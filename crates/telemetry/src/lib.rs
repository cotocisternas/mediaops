//! Deliberately small, local-only telemetry. Existing tracing fields never enter OTLP.
use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime};

use opentelemetry::metrics::{Counter, Histogram, MeterProvider};
use opentelemetry::trace::{Span, Tracer, TracerProvider};
use opentelemetry::{Context, KeyValue};
use opentelemetry_otlp::{WithExportConfig, WithTonicConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::metrics::{PeriodicReader, SdkMeterProvider, Temporality};
use opentelemetry_sdk::trace::{
    BatchConfigBuilder, BatchSpanProcessor, Sampler, SdkTracer, SdkTracerProvider,
};

const EXPORT_TIMEOUT: Duration = Duration::from_millis(500);
const EXPORT_INTERVAL: Duration = Duration::from_secs(15);
const DROP_BUDGET: Duration = Duration::from_millis(600);
const DURATION_BOUNDARIES: &[f64] = &[
    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0, 120.0, 300.0,
    600.0, 1800.0, 3600.0,
];
static SIGNALS: OnceLock<Signals> = OnceLock::new();

#[derive(Debug, PartialEq)]
enum Settings {
    Off,
    Auto(String),
}

impl Settings {
    fn parse(mode: Option<&str>, endpoint: Option<&str>) -> Result<Self, ()> {
        match mode.unwrap_or("auto") {
            "off" => return Ok(Self::Off),
            "auto" => (),
            _ => return Err(()),
        }
        let endpoint = endpoint.unwrap_or("http://127.0.0.1:4317");
        // Parse the authority ourselves: no DNS, credentials, URL normalization,
        // or environment-defined proxy can move collection off this machine.
        let authority = endpoint.strip_prefix("http://").ok_or(())?;
        let authority = authority.strip_suffix('/').unwrap_or(authority);
        let (host, port) = authority.rsplit_once(':').ok_or(())?;
        let port: u16 = port.parse().map_err(|_| ())?;
        if port == 0 || authority.contains(['@', '?', '#', '/']) {
            return Err(());
        }
        let host = host
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(host);
        let ip: IpAddr = if host == "localhost" {
            "127.0.0.1".parse().unwrap()
        } else {
            host.parse().map_err(|_| ())?
        };
        if !ip.is_loopback() {
            return Err(());
        }
        let socket = std::net::SocketAddr::new(ip, port);
        Ok(Self::Auto(format!("http://{socket}")))
    }
}

/// Retain this guard for the whole serve execution. Drop spends at most 600ms
/// waiting for best-effort shutdown; SDK cleanup may finish on a detached thread.
/// Default SIGTERM termination does not run Drop and can lose the final batch.
#[derive(Default)]
pub struct Telemetry(Option<(SdkMeterProvider, SdkTracerProvider)>);

impl Drop for Telemetry {
    fn drop(&mut self) {
        if let Some((metrics, traces)) = self.0.take() {
            let (done, wait) = std::sync::mpsc::sync_channel(1);
            let _ = std::thread::Builder::new()
                .name("telemetry-stop".into())
                .spawn(move || {
                    let _ = metrics.shutdown();
                    let _ = traces.shutdown_with_timeout(EXPORT_TIMEOUT);
                    let _ = done.send(());
                });
            let _ = wait.recv_timeout(DROP_BUDGET);
        }
    }
}

/// Call once per process, after clap parsing, inside the service's Tokio runtime.
pub fn init(service: &'static str, version: &'static str) -> Telemetry {
    let mode = std::env::var("MEDIAOPS_TELEMETRY");
    if mode.as_deref() == Ok("off") {
        return Telemetry::default();
    }
    let endpoint = std::env::var("MEDIAOPS_OTLP_ENDPOINT");
    let settings = match (&mode, &endpoint) {
        (Err(std::env::VarError::NotUnicode(_)), _)
        | (_, Err(std::env::VarError::NotUnicode(_))) => Err(()),
        _ => Settings::parse(mode.as_deref().ok(), endpoint.as_deref().ok()),
    };
    match settings {
        Ok(Settings::Off) => Telemetry::default(),
        Ok(Settings::Auto(endpoint)) => {
            match contain_initialization(|| build(service, version, &endpoint, EXPORT_INTERVAL)) {
                Some((guard, signals)) => {
                    let _ = SIGNALS.set(signals);
                    guard
                }
                None => {
                    eprintln!("telemetry disabled: initialization failed");
                    Telemetry::default()
                }
            }
        }
        Err(_) => {
            eprintln!("telemetry disabled: invalid settings");
            Telemetry::default()
        }
    }
}

// SDK worker-thread construction can panic (for example on thread exhaustion).
// Keep such failures nonfatal without replacing the process-wide panic hook.
// The existing hook may still print its diagnostic before unwinding is caught.
fn contain_initialization<T>(
    build: impl FnOnce() -> Result<T, Box<dyn std::error::Error>> + std::panic::UnwindSafe,
) -> Option<T> {
    std::panic::catch_unwind(build).ok().and_then(Result::ok)
}

fn clear_metadata(mut request: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
    request.metadata_mut().clear();
    Ok(request)
}

fn build(
    service: &'static str,
    version: &'static str,
    endpoint: &str,
    interval: Duration,
) -> Result<(Telemetry, Signals), Box<dyn std::error::Error>> {
    let channel = tonic::transport::Endpoint::from_shared(endpoint.to_owned())?
        .connect_timeout(EXPORT_TIMEOUT)
        .timeout(EXPORT_TIMEOUT)
        .buffer_size(16)
        .connect_lazy();
    // Explicit channels bypass OTEL endpoint/timeout overrides. The SDK merges
    // ambient headers before invoking our interceptor, so clear them there.
    let metrics = opentelemetry_otlp::MetricExporter::builder()
        .with_tonic()
        .with_channel(channel.clone())
        .with_interceptor(clear_metadata)
        .with_timeout(EXPORT_TIMEOUT)
        .with_temporality(Temporality::Cumulative)
        .build()?;
    let traces = opentelemetry_otlp::SpanExporter::builder()
        .with_tonic()
        .with_channel(channel)
        .with_interceptor(clear_metadata)
        .with_timeout(EXPORT_TIMEOUT)
        .build()?;
    let resource = Resource::builder_empty()
        .with_attributes([
            KeyValue::new("service.name", service),
            KeyValue::new("service.version", version),
        ])
        .build();
    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource.clone())
        .with_reader(
            PeriodicReader::builder(metrics)
                .with_interval(interval)
                .build(),
        )
        .build();
    let processor = BatchSpanProcessor::builder(traces)
        .with_batch_config(
            BatchConfigBuilder::default()
                .with_max_queue_size(256)
                .with_max_export_batch_size(64)
                .with_scheduled_delay(interval)
                .build(),
        )
        .build();
    let trace_provider = SdkTracerProvider::builder()
        .with_resource(resource)
        .with_sampler(Sampler::AlwaysOn)
        .with_max_attributes_per_span(2)
        .with_span_processor(processor)
        .build();
    let signals = Signals::new(&meter_provider, &trace_provider);
    Ok((Telemetry(Some((meter_provider, trace_provider))), signals))
}

struct Signals {
    operations: Counter<u64>,
    duration: Histogram<f64>,
    terminal: Counter<u64>,
    installed: Counter<u64>,
    restarts: Counter<u64>,
    tracer: SdkTracer,
}

impl Signals {
    fn new(metrics: &SdkMeterProvider, traces: &SdkTracerProvider) -> Self {
        let meter = metrics.meter("mediaops.telemetry");
        let start = Instant::now();
        meter
            .f64_observable_gauge("mediaops.process.uptime")
            .with_unit("s")
            .with_callback(move |observer| observer.observe(start.elapsed().as_secs_f64(), &[]))
            .build();
        Self {
            operations: meter
                .u64_counter("mediaops.operation.count")
                .with_unit("{operation}")
                .build(),
            duration: meter
                .f64_histogram("mediaops.operation.duration")
                .with_unit("s")
                .with_boundaries(DURATION_BOUNDARIES.to_vec())
                .build(),
            terminal: meter
                .u64_counter("mediaops.pull.terminal")
                .with_unit("{job}")
                .build(),
            installed: meter
                .u64_counter("mediaops.pull.installed")
                .with_unit("By")
                .build(),
            restarts: meter
                .u64_counter("mediaops.supervisor.restarts")
                .with_unit("{restart}")
                .build(),
            tracer: traces.tracer("mediaops.telemetry"),
        }
    }
}

#[derive(Clone, Copy)]
pub enum Operation {
    SchedulerPass,
    InventoryScan,
    PullPass,
}
impl Operation {
    fn name(self) -> &'static str {
        match self {
            Self::SchedulerPass => "scheduler.pass",
            Self::InventoryScan => "inventory.scan",
            Self::PullPass => "pull.pass",
        }
    }
}

/// A standalone span with only fixed attributes; no ambient context or fields.
pub struct Measurement {
    operation: Operation,
    started: Instant,
    started_at: SystemTime,
}
pub fn start(operation: Operation) -> Measurement {
    Measurement {
        operation,
        started: Instant::now(),
        started_at: SystemTime::now(),
    }
}
impl Measurement {
    pub fn finish(self, success: bool) {
        if let Some(signals) = SIGNALS.get() {
            self.record(signals, success);
        }
    }
    fn record(self, signals: &Signals, success: bool) {
        let outcome = if success { "ok" } else { "error" };
        let attributes = [
            KeyValue::new("operation", self.operation.name()),
            KeyValue::new("outcome", outcome),
        ];
        signals.operations.add(1, &attributes);
        signals
            .duration
            .record(self.started.elapsed().as_secs_f64(), &attributes);
        let mut span = signals.tracer.build_with_context(
            signals
                .tracer
                .span_builder(self.operation.name())
                .with_start_time(self.started_at)
                .with_attributes(attributes),
            &Context::new(),
        );
        span.set_status(if success {
            opentelemetry::trace::Status::Ok
        } else {
            opentelemetry::trace::Status::error("")
        });
        span.end();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Terminal {
    Installed,
    Failed,
    Refused,
}
/// Call only after a changed terminal phase was confirmed by the API. Installed
/// bytes are the file length, never progress (which includes resumed bytes).
pub fn terminal(phase: Terminal, file_len: u64) {
    if let Some(s) = SIGNALS.get() {
        s.terminal(phase, file_len);
    }
}
impl Signals {
    fn terminal(&self, phase: Terminal, file_len: u64) {
        let outcome = match phase {
            Terminal::Installed => "installed",
            Terminal::Failed => "failed",
            Terminal::Refused => "refused",
        };
        self.terminal.add(1, &[KeyValue::new("outcome", outcome)]);
        if matches!(phase, Terminal::Installed) {
            self.installed.add(file_len, &[]);
        }
    }
}

/// Unknown role names are ignored, keeping exported cardinality fixed.
pub fn restart(role: &str, success: bool) {
    let role = match role {
        "mediaops-api" => "api",
        "mediaops-scheduler" => "scheduler",
        "mediaops-gateway" => "gateway",
        "mediaops-inventory" => "inventory",
        "mediaops-pull" => "pull",
        _ => return,
    };
    if let Some(s) = SIGNALS.get() {
        s.restarts.add(
            1,
            &[
                KeyValue::new("role", role),
                KeyValue::new("outcome", if success { "ok" } else { "error" }),
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry_proto::tonic::collector::metrics::v1::{
        ExportMetricsServiceRequest, ExportMetricsServiceResponse,
        metrics_service_server::{MetricsService, MetricsServiceServer},
    };
    use opentelemetry_proto::tonic::collector::trace::v1::{
        ExportTraceServiceRequest, ExportTraceServiceResponse,
        trace_service_server::{TraceService, TraceServiceServer},
    };
    use opentelemetry_proto::tonic::metrics::v1::{metric::Data, number_data_point::Value};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Receiver {
        metrics: Arc<Mutex<Vec<ExportMetricsServiceRequest>>>,
        traces: Arc<Mutex<Vec<ExportTraceServiceRequest>>>,
    }
    #[tonic::async_trait]
    impl MetricsService for Receiver {
        async fn export(
            &self,
            request: tonic::Request<ExportMetricsServiceRequest>,
        ) -> Result<tonic::Response<ExportMetricsServiceResponse>, tonic::Status> {
            assert!(request.metadata().get("authorization").is_none());
            assert!(request.metadata().get("secret-header").is_none());
            self.metrics.lock().unwrap().push(request.into_inner());
            Ok(tonic::Response::new(ExportMetricsServiceResponse::default()))
        }
    }
    #[tonic::async_trait]
    impl TraceService for Receiver {
        async fn export(
            &self,
            request: tonic::Request<ExportTraceServiceRequest>,
        ) -> Result<tonic::Response<ExportTraceServiceResponse>, tonic::Status> {
            assert!(request.metadata().get("authorization").is_none());
            assert!(request.metadata().get("secret-header").is_none());
            self.traces.lock().unwrap().push(request.into_inner());
            Ok(tonic::Response::new(ExportTraceServiceResponse::default()))
        }
    }
    fn record(signals: &Signals, operation: Operation, success: bool) {
        Measurement {
            operation,
            started: Instant::now(),
            started_at: SystemTime::now(),
        }
        .record(signals, success);
    }
    #[test]
    fn settings_matrix() {
        assert_eq!(
            Settings::parse(None, None),
            Ok(Settings::Auto("http://127.0.0.1:4317".into()))
        );
        assert_eq!(Settings::parse(Some("off"), Some("bad")), Ok(Settings::Off));
        assert!(Settings::parse(Some("on"), None).is_err());
        for endpoint in [
            "https://127.0.0.1:4317",
            "http://example.com:4317",
            "http://192.168.1.1:4317",
            "http://localhost",
            "http://127.0.0.1:0",
            "http://localhost:65536",
            "http://user:secret@localhost:4317",
            "http://localhost:4317/path",
            "http://localhost:4317?secret",
            "http://localhost:4317#fragment",
            "http://[::]:4317",
            "http://localhost:4317//",
        ] {
            assert!(Settings::parse(None, Some(endpoint)).is_err(), "{endpoint}");
        }
        for endpoint in [
            "http://localhost:4317",
            "http://127.0.0.2:4317/",
            "http://[::1]:4317",
        ] {
            assert!(Settings::parse(None, Some(endpoint)).is_ok(), "{endpoint}");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn collector_recovery_signals_and_bounded_shutdown() {
        // Retain the bound listener to avoid a drop/rebind port race. Delay
        // accepting HTTP/2 until the first export has exceeded its timeout.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let started = Instant::now();
        let (guard, signals) = build(
            "mediaops-test",
            "test-version",
            &format!("http://{address}"),
            Duration::from_millis(50),
        )
        .unwrap();
        assert!(started.elapsed() < Duration::from_secs(1));
        record(&signals, Operation::SchedulerPass, false);
        signals.terminal(Terminal::Failed, 999);
        signals.terminal(Terminal::Refused, 999);
        signals.terminal(Terminal::Installed, 123);
        tokio::time::sleep(EXPORT_TIMEOUT + Duration::from_millis(200)).await;
        let receiver = Receiver::default();
        let server = tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(MetricsServiceServer::new(receiver.clone()))
                .add_service(TraceServiceServer::new(receiver.clone()))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
        );
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                // A failed span export is dropped; new operations export after recovery.
                record(&signals, Operation::InventoryScan, true);
                if !receiver.metrics.lock().unwrap().is_empty()
                    && !receiver.traces.lock().unwrap().is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .unwrap();
        // Verify a failure span after recovery, too.
        record(&signals, Operation::PullPass, false);
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if receiver
                    .traces
                    .lock()
                    .unwrap()
                    .iter()
                    .flat_map(|r| &r.resource_spans)
                    .flat_map(|r| &r.scope_spans)
                    .flat_map(|s| &s.spans)
                    .any(|s| s.name == "pull.pass")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
        let metrics = receiver.metrics.lock().unwrap();
        let mut found_installed = false;
        let mut found_uptime = false;
        let mut terminal_outcomes = std::collections::BTreeSet::new();
        for resource in metrics.iter().flat_map(|r| &r.resource_metrics) {
            let attributes = &resource.resource.as_ref().unwrap().attributes;
            assert_eq!(attributes.len(), 2);
            assert!(
                attributes.iter().any(|a| a.key == "service.name"
                    && format!("{:?}", a.value).contains("mediaops-test"))
            );
            for metric in resource.scope_metrics.iter().flat_map(|s| &s.metrics) {
                match metric.data.as_ref().unwrap() {
                    Data::Sum(sum) => {
                        assert_eq!(sum.aggregation_temporality, 2); // cumulative
                        assert!(sum.is_monotonic);
                        if metric.name == "mediaops.pull.installed" {
                            assert_eq!(metric.unit, "By");
                            assert_eq!(sum.data_points[0].value, Some(Value::AsInt(123)));
                            found_installed = true;
                        }
                        if metric.name == "mediaops.pull.terminal" {
                            for p in &sum.data_points {
                                assert_eq!(p.value, Some(Value::AsInt(1)));
                                assert_eq!(p.attributes.len(), 1);
                                terminal_outcomes.insert(format!("{:?}", p.attributes[0].value));
                            }
                        }
                    }
                    Data::Histogram(h) => {
                        assert_eq!(h.aggregation_temporality, 2);
                        assert_eq!(metric.unit, "s");
                    }
                    Data::Gauge(g) => {
                        assert_eq!(metric.name, "mediaops.process.uptime");
                        assert_eq!(metric.unit, "s");
                        assert!(!g.data_points.is_empty());
                        found_uptime = true;
                    }
                    _ => panic!("unexpected metric"),
                }
            }
        }
        assert!(found_installed && found_uptime);
        assert_eq!(terminal_outcomes.len(), 3);
        drop(metrics);
        for span in receiver
            .traces
            .lock()
            .unwrap()
            .iter()
            .flat_map(|r| &r.resource_spans)
            .flat_map(|r| &r.scope_spans)
            .flat_map(|s| &s.spans)
        {
            assert_eq!(span.attributes.len(), 2);
            assert!(
                span.attributes
                    .iter()
                    .all(|a| ["operation", "outcome"].contains(&a.key.as_str()))
            );
            assert!(span.events.is_empty() && span.links.is_empty());
            assert!(span.end_time_unix_nano >= span.start_time_unix_nano);
            if span.name == "pull.pass" {
                assert_eq!(span.status.as_ref().unwrap().code, 2);
                assert!(span.status.as_ref().unwrap().message.is_empty());
            }
        }
        server.abort();
        let started = Instant::now();
        drop(guard);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn interceptor_removes_ambient_metadata() {
        let mut request = tonic::Request::new(());
        request
            .metadata_mut()
            .insert("authorization", "Bearer-secret".parse().unwrap());
        assert!(clear_metadata(request).unwrap().metadata().is_empty());
    }
    #[test]
    fn ambient_overrides_cannot_change_exports() {
        // Give only the child a hostile environment; never mutate shared test env.
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::public_enabled_path", "--nocapture"])
            .env("OTEL_EXPORTER_OTLP_ENDPOINT", "http://192.0.2.1:1")
            .env("OTEL_EXPORTER_OTLP_METRICS_ENDPOINT", "http://192.0.2.2:1")
            .env("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "http://192.0.2.3:1")
            .env(
                "OTEL_EXPORTER_OTLP_HEADERS",
                "authorization=Bearer-secret,secret-header=secret",
            )
            .env(
                "OTEL_EXPORTER_OTLP_METRICS_HEADERS",
                "authorization=metrics-secret",
            )
            .env(
                "OTEL_EXPORTER_OTLP_TRACES_HEADERS",
                "secret-header=trace-secret",
            )
            .env(
                "OTEL_RESOURCE_ATTRIBUTES",
                "secret.attribute=secret,service.name=wrong",
            )
            .env("OTEL_SERVICE_NAME", "wrong")
            .env("OTEL_EXPORTER_OTLP_METRICS_TEMPORALITY_PREFERENCE", "delta")
            .env("OTEL_METRIC_EXPORT_INTERVAL", "99999999")
            .env("OTEL_BSP_SCHEDULE_DELAY", "99999999")
            .env("OTEL_TRACES_SAMPLER", "always_off")
            .env("OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT", "0")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[tokio::test]
    async fn init_child() {
        if std::env::var_os("MEDIAOPS_TEST_INIT").is_some() {
            let telemetry = init("mediaops-test", "test-version");
            assert!(telemetry.0.is_none());
        }
    }

    #[test]
    fn disabled_and_invalid_settings_are_nonfatal() {
        for (mode, endpoint, warning) in [
            ("off", "invalid secret endpoint", ""),
            (
                "invalid secret mode",
                "http://127.0.0.1:4317",
                "telemetry disabled: invalid settings\n",
            ),
            (
                "auto",
                "http://secret@127.0.0.1:4317",
                "telemetry disabled: invalid settings\n",
            ),
        ] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tests::init_child", "--nocapture"])
                .env("MEDIAOPS_TEST_INIT", "1")
                .env("MEDIAOPS_TELEMETRY", mode)
                .env("MEDIAOPS_OTLP_ENDPOINT", endpoint)
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(String::from_utf8(output.stderr).unwrap(), warning);
        }
    }
    #[test]
    fn construction_panics_are_contained() {
        let result: Option<()> =
            contain_initialization(|| panic!("simulated SDK thread spawn failure"));
        assert!(result.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn off_ignores_non_unicode_endpoint() {
        use std::os::unix::ffi::OsStringExt;
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "tests::init_child", "--nocapture"])
            .env("MEDIAOPS_TEST_INIT", "1")
            .env("MEDIAOPS_TELEMETRY", "off")
            .env(
                "MEDIAOPS_OTLP_ENDPOINT",
                std::ffi::OsString::from_vec(vec![0xff]),
            )
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(output.stderr.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_with_stalled_collector_is_bounded() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let (guard, signals) = build(
            "mediaops-test",
            "test-version",
            &format!("http://{}", listener.local_addr().unwrap()),
            Duration::from_millis(10),
        )
        .unwrap();
        record(&signals, Operation::PullPass, true);
        let (connection, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .unwrap()
            .unwrap();
        // Keep the TCP stream alive without ever answering the HTTP/2 handshake.
        let started = Instant::now();
        tokio::task::spawn_blocking(move || drop(guard))
            .await
            .unwrap();
        assert!(started.elapsed() < DROP_BUDGET + Duration::from_millis(300));
        drop(connection);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn public_enabled_child() {
        if std::env::var_os("MEDIAOPS_TEST_ENABLED").is_none() {
            return;
        }
        let guard = init("mediaops-public-test", "exact-version");
        assert!(guard.0.is_some());
        let operation = start(Operation::SchedulerPass);
        tokio::time::sleep(Duration::from_millis(20)).await;
        operation.finish(true);
        start(Operation::InventoryScan).finish(false);
        // Cancellation must not create a completed span (nor a counter entry).
        {
            let _unfinished = start(Operation::PullPass);
        }
        terminal(Terminal::Failed, 999);
        terminal(Terminal::Refused, 999);
        terminal(Terminal::Installed, 123);
        for role in [
            "mediaops-api",
            "mediaops-scheduler",
            "mediaops-gateway",
            "mediaops-inventory",
            "mediaops-pull",
        ] {
            restart(role, true);
            restart(role, false);
        }
        restart("unknown-private-role", true);
        let (metrics, traces) = guard.0.as_ref().unwrap().clone();
        tokio::task::spawn_blocking(move || {
            metrics.force_flush().unwrap();
            traces.force_flush().unwrap();
        })
        .await
        .unwrap();
        drop(guard);
    }

    fn attributes(
        items: &[opentelemetry_proto::tonic::common::v1::KeyValue],
    ) -> std::collections::BTreeMap<String, String> {
        use opentelemetry_proto::tonic::common::v1::any_value;
        items
            .iter()
            .map(|a| {
                let Some(any_value::Value::StringValue(value)) =
                    a.value.as_ref().and_then(|v| v.value.as_ref())
                else {
                    panic!("string attribute required");
                };
                (a.key.clone(), value.clone())
            })
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn public_enabled_path() {
        let receiver = Receiver::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(MetricsServiceServer::new(receiver.clone()))
                .add_service(TraceServiceServer::new(receiver.clone()))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener)),
        );
        let output = tokio::task::spawn_blocking(move || {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tests::public_enabled_child", "--nocapture"])
                .env("MEDIAOPS_TEST_ENABLED", "1")
                .env("MEDIAOPS_TELEMETRY", "auto")
                .env("MEDIAOPS_OTLP_ENDPOINT", endpoint)
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let expected_resource = std::collections::BTreeMap::from([
            (
                "service.name".to_string(),
                "mediaops-public-test".to_string(),
            ),
            ("service.version".to_string(), "exact-version".to_string()),
        ]);
        let expected_operations = std::collections::BTreeSet::from([
            ("scheduler.pass".to_string(), "ok".to_string()),
            ("inventory.scan".to_string(), "error".to_string()),
        ]);
        let mut metric_names = std::collections::BTreeSet::new();
        for resource in receiver
            .metrics
            .lock()
            .unwrap()
            .iter()
            .flat_map(|r| &r.resource_metrics)
        {
            assert_eq!(
                attributes(&resource.resource.as_ref().unwrap().attributes),
                expected_resource
            );
            for metric in resource.scope_metrics.iter().flat_map(|s| &s.metrics) {
                metric_names.insert(metric.name.clone());
                match metric.data.as_ref().unwrap() {
                    Data::Sum(sum) => {
                        assert_eq!(sum.aggregation_temporality, 2);
                        assert!(sum.is_monotonic);
                        match metric.name.as_str() {
                            "mediaops.operation.count" => {
                                assert_eq!(sum.data_points.len(), 2);
                                let labels = sum
                                    .data_points
                                    .iter()
                                    .map(|p| {
                                        assert_eq!(p.value, Some(Value::AsInt(1)));
                                        assert_eq!(p.attributes.len(), 2);
                                        let a = attributes(&p.attributes);
                                        (a["operation"].clone(), a["outcome"].clone())
                                    })
                                    .collect::<std::collections::BTreeSet<_>>();
                                assert_eq!(labels, expected_operations);
                            }
                            "mediaops.pull.terminal" => {
                                assert_eq!(sum.data_points.len(), 3);
                                let outcomes = sum
                                    .data_points
                                    .iter()
                                    .map(|p| {
                                        assert_eq!(p.value, Some(Value::AsInt(1)));
                                        assert_eq!(p.attributes.len(), 1);
                                        attributes(&p.attributes)["outcome"].clone()
                                    })
                                    .collect::<std::collections::BTreeSet<_>>();
                                assert_eq!(
                                    outcomes,
                                    ["installed", "failed", "refused"]
                                        .map(str::to_string)
                                        .into()
                                );
                            }
                            "mediaops.pull.installed" => {
                                assert_eq!(metric.unit, "By");
                                assert_eq!(sum.data_points.len(), 1);
                                assert!(sum.data_points[0].attributes.is_empty());
                                assert_eq!(sum.data_points[0].value, Some(Value::AsInt(123)));
                            }
                            "mediaops.supervisor.restarts" => {
                                assert_eq!(sum.data_points.len(), 10);
                                let labels = sum
                                    .data_points
                                    .iter()
                                    .map(|p| {
                                        assert_eq!(p.value, Some(Value::AsInt(1)));
                                        assert_eq!(p.attributes.len(), 2);
                                        let a = attributes(&p.attributes);
                                        (a["role"].clone(), a["outcome"].clone())
                                    })
                                    .collect::<std::collections::BTreeSet<_>>();
                                let expected = ["api", "scheduler", "gateway", "inventory", "pull"]
                                    .into_iter()
                                    .flat_map(|role| {
                                        ["ok", "error"]
                                            .map(|outcome| (role.to_string(), outcome.to_string()))
                                    })
                                    .collect::<std::collections::BTreeSet<_>>();
                                assert_eq!(labels, expected);
                            }
                            _ => panic!("unexpected counter"),
                        }
                    }
                    Data::Histogram(histogram) => {
                        assert_eq!(metric.name, "mediaops.operation.duration");
                        assert_eq!(metric.unit, "s");
                        assert_eq!(histogram.aggregation_temporality, 2);
                        assert_eq!(histogram.data_points.len(), 2);
                        let labels = histogram
                            .data_points
                            .iter()
                            .map(|p| {
                                assert_eq!(p.count, 1);
                                assert_eq!(p.explicit_bounds, DURATION_BOUNDARIES);
                                assert_eq!(p.attributes.len(), 2);
                                let a = attributes(&p.attributes);
                                if a["operation"] == "scheduler.pass" {
                                    assert!(p.sum.unwrap() >= 0.02);
                                }
                                (a["operation"].clone(), a["outcome"].clone())
                            })
                            .collect::<std::collections::BTreeSet<_>>();
                        assert_eq!(labels, expected_operations);
                    }
                    Data::Gauge(gauge) => {
                        assert_eq!(metric.name, "mediaops.process.uptime");
                        assert_eq!(metric.unit, "s");
                        assert_eq!(gauge.data_points.len(), 1);
                        assert!(gauge.data_points[0].attributes.is_empty());
                    }
                    _ => panic!("unexpected metric"),
                }
            }
        }
        assert_eq!(
            metric_names,
            [
                "mediaops.operation.count",
                "mediaops.operation.duration",
                "mediaops.process.uptime",
                "mediaops.pull.terminal",
                "mediaops.pull.installed",
                "mediaops.supervisor.restarts"
            ]
            .map(str::to_string)
            .into()
        );
        let mut spans = Vec::new();
        for resource in receiver
            .traces
            .lock()
            .unwrap()
            .iter()
            .flat_map(|r| &r.resource_spans)
        {
            assert_eq!(
                attributes(&resource.resource.as_ref().unwrap().attributes),
                expected_resource
            );
            for span in resource.scope_spans.iter().flat_map(|s| &s.spans) {
                assert_eq!(span.attributes.len(), 2);
                let labels = attributes(&span.attributes);
                assert_eq!(span.name, labels["operation"]);
                assert!(span.events.is_empty() && span.links.is_empty());
                if span.name == "scheduler.pass" {
                    assert!(span.end_time_unix_nano - span.start_time_unix_nano >= 20_000_000);
                    assert_eq!(span.status.as_ref().unwrap().code, 1);
                } else {
                    assert_eq!(span.status.as_ref().unwrap().code, 2);
                    assert!(span.status.as_ref().unwrap().message.is_empty());
                }
                spans.push((labels["operation"].clone(), labels["outcome"].clone()));
            }
        }
        assert_eq!(
            spans.len(),
            2,
            "unfinished measurement must not export a span"
        );
        assert_eq!(
            spans.into_iter().collect::<std::collections::BTreeSet<_>>(),
            expected_operations
        );
        server.abort();
    }
}
