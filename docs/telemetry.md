# Local Home telemetry

The six long-running Home processes (`mediaops-home`, `mediaops-api`,
`mediaops-scheduler`, `mediaops-gateway`, `mediaops-inventory`, `mediaops-pull`)
export optional metrics and completed operation traces to a local Grafana Alloy
collector using OTLP/gRPC. Stderr logs and CLI output keep their existing format.
CLI, TUI and seedbox instrumentation, log shipping, cross-process trace
propagation and dashboards are outside this feature.

## Settings

Telemetry settings are **service environment variables**, separate from
`config.toml` and the Cluster object. Children inherit the supervisor environment.

| Variable | Default | Meaning |
| --- | --- | --- |
| `MEDIAOPS_TELEMETRY` | `auto` | `auto` attempts background collection; `off` creates no exporters or telemetry network tasks. |
| `MEDIAOPS_OTLP_ENDPOINT` | `http://127.0.0.1:4317` | Loopback HTTP endpoint with an explicit nonzero port; OTLP/gRPC only. |

Examples of valid endpoints are `http://localhost:4317`,
`http://127.0.0.2:4317/`, and `http://[::1]:4317`. `localhost` is resolved to
`127.0.0.1` without DNS. Non-loopback hosts, HTTPS, credentials, queries,
fragments, and paths other than `/` are rejected. Invalid settings disable
telemetry with one concise stderr warning that does not print the settings.

For a systemd user service, add an override using `systemctl --user edit
mediaops-home.service`, for example:

```ini
[Service]
Environment=MEDIAOPS_TELEMETRY=off
```

Then run `systemctl --user daemon-reload` and restart `mediaops-home.service`
when interrupting active work is acceptable. To select another collector port,
use `Environment=MEDIAOPS_OTLP_ENDPOINT=http://127.0.0.1:14317` instead.
`--help` and `--version` do not initialize telemetry.

Ambient `OTEL_*` endpoint, header and resource overrides are not imported into
exports. Identity and transport limits are set explicitly; ambient headers are
cleared before requests. Unsupported SDK compression environment settings can
cause initialization to disable telemetry rather than fail the service. SDK
construction panics are caught during initialization and disable telemetry; the existing process panic hook may print
its diagnostic before the panic is caught. Telemetry does not replace that hook.
Initialization is intended once per serve process.

## Signals and interpretation

Each signal has only the resource attributes `service.name` (the executable
name) and `service.version` (the application version). No hostname, process ID,
paths, title/job IDs, secret values, error messages or arbitrary existing tracing
fields are exported. There is no tracing-subscriber bridge.

| OTLP metric name | Unit | Labels | Meaning |
| --- | --- | --- | --- |
| `mediaops.process.uptime` | `s` | none | Observable gauge, elapsed time since telemetry initialization; emitted even when idle. |
| `mediaops.operation.count` | `{operation}` | `operation`, `outcome` | Completed scheduler passes, inventory scans, and pull passes. |
| `mediaops.operation.duration` | `s` | `operation`, `outcome` | Histogram of elapsed operation time, with explicit boundaries from 1 ms through 1 hour. |
| `mediaops.pull.terminal` | `{job}` | `outcome` | Confirmed changed terminal status writes: `installed`, `failed`, `refused`. |
| `mediaops.pull.installed` | `By` | none | File length observed after confirmed Installed persistence. **Installed bytes**, not network bytes. |
| `mediaops.supervisor.restarts` | `{restart}` | `role`, `outcome` | Child respawn attempts after exit, including failed attempts. Initial spawns are excluded. |

Operations are restricted to `scheduler.pass`, `inventory.scan`, `pull.pass`;
operation/restart outcomes are `ok` and `error`. Restart roles are `api`,
`scheduler`, `gateway`, `inventory`, `pull`. Completed spans use the operation
name, these same operation/outcome attributes, timestamps and status; they have
no error text, events or links. Each is a standalone trace. Dropping an unfinished
operation emits no completed span or operation metric.

A pull pass can return `ok` after persisting a Failed or Refused Job. Use
`mediaops.pull.terminal` to count those outcomes. Retries that do not confirm a
status write are not counted. Installed bytes include resumed/recovered files
once their Installed transition is confirmed; progress is never summed.
Terminal counters describe observations in this process, not a durable audit
or an exactly-once accounting ledger.

Counters and histograms use **cumulative temporality**, suitable for Alloy's
Prometheus conversion. They reset on process restart. Alloy translates metric
names/units to Prometheus conventions and maps `service.name` to `job`.
The bounded labels keep series cardinality fixed. Multiple Home installations
forwarding into one backend should add an operator-chosen host label in Alloy
to distinguish their series.

## Delivery and shutdown

Initialization never probes the collector synchronously. Metrics are collected
and spans are exported in the background every 15 seconds; span batches also
export when full. Connection and request timeouts are 500 ms. The span queue
holds at most 256 spans, with batches of at most 64 and a channel buffer of 16.
Internal SDK logging is disabled to avoid repeated collector warnings.

Work continues when Alloy is missing. Later exports retry the connection, so
starting Alloy later needs no Home restart. Failed span batches and queue
overflow are lost; there is no disk spool. Cumulative metrics retain in-process
counts for the next successful collection, but restarts lose unexported changes.

Normal return attempts best-effort shutdown on a separate thread, waiting at
most 600 ms. The supervisor still forwards SIGTERM and gives its children five
seconds before SIGKILL. Default child SIGTERM termination, SIGKILL, crashes,
and runtime teardown can lose the last batch; Drop is **not** a SIGTERM flush
promise. This feature does not change service shutdown handling.

## Alloy example

The following Alloy configuration accepts local OTLP metrics and converts them
to Prometheus remote write. Replace the example URL with your metrics backend.
For an authenticated backend, configure credentials in Alloy, not Home.

```alloy
otelcol.receiver.otlp "mediaops" {
  grpc {
    endpoint = "127.0.0.1:4317"
  }
  output {
    metrics = [otelcol.exporter.prometheus.mediaops.input]
  }
}

otelcol.exporter.prometheus "mediaops" {
  forward_to = [prometheus.remote_write.mediaops.receiver]
}

prometheus.remote_write "mediaops" {
  endpoint {
    url = "https://prometheus.example.com/api/v1/write"
  }
}
```

For optional trace forwarding, add this line inside the receiver's `output`
block:

```alloy
traces = [otelcol.exporter.otlp.traces.input]
```

Then add this component, replacing the endpoint with your trace backend:

```alloy
otelcol.exporter.otlp "traces" {
  client {
    endpoint = "tempo.example.com:443"
  }
}
```

Validate your full configuration with `alloy validate /path/to/config.alloy`
before applying it. Bind the receiver to loopback. Install or reconfigure Alloy
as an explicit operator action; Home does neither. With no trace output, the
metrics-only example does not retain operation traces.

References: [Alloy OTLP receiver](https://grafana.com/docs/alloy/latest/reference/components/otelcol/otelcol.receiver.otlp/),
[Prometheus conversion](https://grafana.com/docs/alloy/latest/reference/components/otelcol/otelcol.exporter.prometheus/),
[OTLP trace exporter](https://grafana.com/docs/alloy/latest/reference/components/otelcol/otelcol.exporter.otlp/).

Tests use an isolated local OTLP receiver and require no Alloy, seedbox, SSH or
GPU. Actual Grafana ingestion remains an operator integration check.
