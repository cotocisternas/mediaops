---
title: Local Alloy telemetry for the Home control plane
type: feature
created: 2026-09-15
status: done
baseline_commit: 7b4c9045f67c33d78609db0b168d2a122020b8f9
review_loop_iteration: 0
context:
  - /home/coto/work/dev/mediaops/AGENTS.md
  - /home/coto/work/dev/mediaops/docs/architecture.md
  - /home/coto/work/dev/mediaops/docs/development.md
---

<frozen-after-approval reason="implementation authorized by user request">

## Intent

**Problem:** Home services have stderr logs but no metrics or operation traces for a local Grafana Alloy collector. Operators cannot graph work rates, failures, or latency.

**Approach:** Add shared optional OpenTelemetry metrics and deliberately instrumented operation traces to the six long-running Home processes. Push OTLP/gRPC to loopback Alloy; document conversion to Prometheus in Alloy. Default to attempting localhost collection in the background, with explicit disable and endpoint override. An absent collector must never prevent useful work.

## Boundaries & Constraints

**Always:** Use bounded background export, short network timeouts, bounded span queues, cumulative metrics compatible with Prometheus, service name/version identity, and fixed operation/outcome labels. Preserve stderr logging and CLI stdout. Keep initialization free of synchronous collector probes. Retry on later exports so an Alloy started later works without restarting Home. Use official Rust SDK crates with default features disabled and tonic transport, avoiding reqwest/native-tls. Allowlist workspace edges before adding them.

**Ask First:** Installing or reconfiguring system Alloy, or changing unrelated operator configuration.

**Never:** Export paths, IDs, secret values, error messages, or existing arbitrary tracing fields; add database access to workers; dial seedbox from CLI; change Pull semantics or snapshot configuration. This scope is Home telemetry; CLI/TUI and seedbox instrumentation, log shipping, end-to-end RPC trace propagation and dashboards are outside it.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
| --- | --- | --- | --- |
| Collector available | Local OTLP receiver | Service resource, cumulative metrics and completed operation spans arrive | Export runs off work path |
| Collector absent or starts later | Refused connection, then receiver available | Work continues, later export succeeds | Bounded timeout and queues; no repeated warning flood |
| Disabled | MEDIAOPS_TELEMETRY=off | No exporter or network tasks | Local logs remain |
| Invalid settings | Invalid mode or endpoint | Telemetry disabled with one concise stderr warning | Never fail service startup or echo raw settings |
| Operation failure | Instrumented operation returns error | Fixed error outcome and elapsed duration | No raw error string exported |

</frozen-after-approval>

## Code Map

- `Cargo.toml`, `Cargo.lock`: pinned workspace dependencies. Current tonic is 0.14.6. crates.io has opentelemetry-otlp 0.32.0; verify compatible SDK versions locally.
- `crates/arch-tests/src/lib.rs`: ALLOWED_WORKSPACE_EDGES must be edited first; reqwest direct dependency is only legal in arr. Six new binary-to-telemetry edges suffice.
- `bins/mediaops-{home,api,scheduler,gateway,inventory,pull}/src/main.rs`: each has init_tracing and tokio main. Preserve logging. Supervisor spawn_role/restart loop is a fixed-role restart metric point.
- `bins/mediaops-scheduler/src/main.rs`: heartbeat_loop invokes bind_pending; record pass count/outcome/duration around the invocation.
- `bins/mediaops-inventory/src/main.rs`: run invokes refresh; record scan count/outcome/duration around that invocation.
- `bins/mediaops-pull/src/main.rs`: run invokes claim_and_run (pass result only). run_job returns Ok for persisted failures/refusals, so save_job is the correct point for successful terminal status-write metrics. Count Installed file length only after confirmed persistence, clearly described as installed bytes (not network bytes). Progress includes resumed bytes and must not be summed.
- `bins/mediaops-home/src/main.rs`: supervisor forwards SIGTERM and grants 5 seconds. Child services currently use default signal termination. Exporter Drop cannot promise SIGTERM flush; any lifecycle changes must preserve supervisor cleanup and cap shutdown time. At minimum document last-batch loss on abrupt exit.
- `docs/setup.md`, `docs/config.md`, `docs/README.md`: operator documentation entry points. Telemetry uses service environment, not config.toml/Cluster mutation.

## Tasks & Acceptance

**Execution:**
- [x] `crates/arch-tests/src/lib.rs`, workspace and binary Cargo manifests: allowlist and add a leaf `crates/telemetry` crate with pinned minimal OTel dependencies.
- [x] `crates/telemetry/src/lib.rs`: parse MEDIAOPS_TELEMETRY=auto/off (default auto) and MEDIAOPS_OTLP_ENDPOINT (default http://127.0.0.1:4317). Limit accepted endpoints to loopback HTTP/gRPC with explicit port; reject credentials, query, fragment and non-root paths. Build providers, resource identity, process uptime metric, fixed-label operation counters/histograms/spans, and bounded best-effort lifecycle. Avoid importing ambient OTEL endpoint/header/resource overrides that undermine the local-only contract; inspect SDK precedence. Use explicit channel if needed.
- [x] Six Home binary main files: initialize telemetry for serve execution and instrument scheduler/inventory/pull passes, persisted terminal pull outcomes/installed bytes, and supervisor restarts. Retain provider lifetime. No init/network for --help/version.
- [x] `crates/telemetry` tests: cover matrix with isolated in-memory providers or fake local OTLP receiver, including unavailable-to-available recovery and bounded shutdown. Do not depend on installed Alloy or mutate shared process environment unsafely.
- [x] `docs/telemetry.md` plus docs links: describe settings, emitted signals and units, resource/label cardinality, data-loss semantics, and copyable Alloy OTLP receiver → Prometheus remote_write and optional trace forwarding example. No live system edits.

**Acceptance Criteria:**
- Given the Home supervisor starts, when its children execute work, then telemetry identifies each service separately without changing existing command output or maintenance rules.
- Given a persisted Failed or Refused Pull Job, when run_job returns Ok, then terminal metrics still record the actual persisted phase.
- Given the default Cargo test suite, when the change is verified, then tests require no Alloy, seedbox, SSH or GPU and architecture laws remain valid.

## Spec Change Log

## Design Notes

OTLP is the push transport; Alloy converts cumulative OTel metrics into Prometheus remote write. Keep current tracing logs separate to avoid exporting sensitive fields. Explicit operation spans need no tracing-subscriber bridge or remote context propagation. Process uptime distinguishes a running idle service from a missing service; persisted terminal counts describe observations and reset on process restart.

## Verification

- `cargo test -p mediaops-telemetry --locked`
- `make test` (includes sibling binary build and architecture tests)
- `make fmt`, `make clippy`

Use an isolated local receiver for OTLP verification. Actual Grafana ingestion is an operator integration step, not asserted by unit tests.


### Implementation verification (2026-09-15)

- `cargo test -p mediaops-telemetry --locked`: passed, 11 tests. Isolated OTLP
  receiver checks cumulative metrics, resource identity, bounded labels, terminal
  outcome/installed byte values, completed spans, collector recovery and bounded
  shutdown. Child-process tests check hostile ambient OTEL settings, explicit off,
  and sanitized nonfatal invalid-setting warnings without shared env mutation.
- `make test`: passed, 889 tests across 64 targets, including the required sibling
  binary build and architecture tests. No Alloy, seedbox, SSH or GPU required by the suite.
- `make fmt`: passed. `make clippy`: passed with existing repository warnings;
  no warnings reported for the telemetry crate.
- Independent isolated Alloy v1.19.2 smoke: API and scheduler exported uptime,
  operation count and a scheduler span. Both documented Alloy configurations
  validated successfully. Temporary processes were stopped; no system Alloy or
  operator configuration was changed.
- Actual Grafana backend ingestion remains an operator integration step. SIGTERM
  last-batch loss and failed span export loss are documented. SDK metrics shutdown
  ignores its supplied timeout in version 0.32.0, so guard Drop uses a separate
  cleanup thread and an outer 600 ms wait budget.


### Review fixes verified

- Duration histograms now explicitly cover 1 ms through 1 hour; two span attributes
  are reserved explicitly even with `OTEL_SPAN_ATTRIBUTE_COUNT_LIMIT=0`.
- Measurements create spans only when finished, retaining the original start
  timestamp. Cancelled measurements emit neither spans nor operation metrics.
- SDK construction panics disable telemetry without replacing the process panic
  hook. Explicit off ignores even a non-Unicode endpoint.
- The isolated public enabled-path test verifies init, start/finish, terminal and
  restart exports, exact resource/version and label sets, counters, histogram
  boundaries, installed bytes, unknown-role exclusion and cancellation. Hostile
  OTEL overrides run in a child process.
- The Pull suite passes 13 tests, including a fake-API persistence-boundary test:
  successful Failed/Refused/Installed writes observe the confirmed phase, stale
  writes and unchanged phases emit nothing, and Installed bytes count once.
- Recovery tests retain a bound listener before serving, avoiding a port-rebind
  race. A separate accepting-but-stalled TCP collector verifies the outer Drop
  budget. Telemetry's 11 focused tests, workspace tests, fmt and Clippy pass after
  these fixes; no new telemetry warnings.
- All six Home executables return successfully with empty stderr for help/version
  despite invalid telemetry settings. Rebuilt API/scheduler binaries also pass the
  isolated Alloy smoke after the Measurement changes.

## Suggested Review Order

**Local export and privacy**

- Validate loopback settings and isolate optional telemetry from service startup.
  [telemetry/lib.rs:88](../../crates/telemetry/src/lib.rs#L88)

- Bound collection and export while preserving cumulative metrics and fixed resource attributes.
  [telemetry/lib.rs:134](../../crates/telemetry/src/lib.rs#L134)

**Operation and persistence semantics**

- Publish completed measurements without inventing spans for cancelled operations.
  [telemetry/lib.rs:255](../../crates/telemetry/src/lib.rs#L255)

- Record actual terminal outcomes only after confirmed changed status writes.
  [main.rs:594](../../bins/mediaops-pull/src/main.rs#L594)

**Verification and operation**

- Verify exported metrics and traces through the public API.
  [telemetry/lib.rs:756](../../crates/telemetry/src/lib.rs#L756)

- Protect persisted outcome counts against retries and failed writes.
  [tests.rs:607](../../bins/mediaops-pull/src/tests.rs#L607)

- Configure Alloy and understand the signals and delivery limits.
  [telemetry.md:10](../telemetry.md#L10)
