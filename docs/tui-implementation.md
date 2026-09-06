---
title: Additive home TUI
type: feature
created: 2026-09-06
status: done
baseline_commit: 367e78ff6b82fb21c71b48f33c9ef8cf6c964c8e
context:
  - AGENTS.md
  - docs/architecture.md
  - docs/usage.md
  - docs/development.md
---

# Additive home TUI implementation

The user approved the full hyperplan and requested unattended implementation.
No commits, pushes, installations into the operator's active environment, or live
seedbox operations are authorized. Preserve the pre-existing untracked `.omo/`.

<frozen-after-approval>

## Intent

Implement a complete separate `mediaops-tui` executable, not a replacement or
subcommand of `mediaops`. Preserve the existing CLI source, help, formatters,
exact-screen tests, raw `-o json`, envelope `--json`, stdout/stderr and exit codes.
The approved plan is a cohesive home-side visual management feature, including
all seven read screens and exactly four scoped mutations, not a read-only draft.

## Boundaries

Register architecture edges before Cargo edges. TUI direct workspace dependencies
are core and home-client only. Use HomeApi over its default/configured Unix socket
as Actor::Cli, never raw wire conversion, proto/tonic direct dependencies, API
server/store, CLI imports/subprocesses, gateway/ControlPort, Range/transfer, SSH,
arr/reqwest, encode, sync, PEMs, seedbox dialing, or destination-path rendering.
Never acquire maintenance/role locks or modify config, systemd, supervisor,
controller behavior, `.proto`, admission or daemon musl packaging. Keep
grabber=none valid. No prompts, modals, free-form input, batch/auto-approval,
arbitrary editors, mutation retries or subprocess launches in production TUI.

## Product and lifecycle

Create bins/mediaops-tui with a library target for API-side integration tests.
Pin ratatui 0.30.2 and crossterm 0.29.0 (event-stream), retaining compatible
versions; use existing tokio/tokio-stream, clap, thiserror/anyhow conventions.
Verify library syntax against current docs. Small focused modules, no unsafe.

Invocation: mediaops-tui [--api-socket PATH] [--color auto|always|never]. Help and
version never need a socket or TTY. Interactive stdin/stdout must be terminals;
TERM=dumb or redirection fails exit 2 with no escape sequences. q/Ctrl-C/SIGTERM
exit cleanly 0; fatal terminal/event errors exit 1 after restoration. API absence
shows reconnecting, never fallback to local DB. Restore partial initialization,
raw/alternate modes, cursor/paste state on normal return, error and panic.

Seven screens: 1 Overview (Wants, non-installed Jobs, failures, worker readiness,
disk), 2 Wants, 3 Jobs (phase/bytes/attempts/binding/failure), 4 open Holds,
5 Titles/why facts, 6 Nodes/readiness, 7 Box/current RemoteFiles. Build known title
identities from valid TitleIds in these objects, never paths/display names.
Show observed facts only, no invented ETA/throughput. Reuse observed_files proofs.
Read disk through core::free_bytes outside draw, every 5 seconds/root change,
showing observation age/unavailable; this is not API List polling.

Navigation: 1-7 and Tab/Shift-Tab screens; arrows/j/k, PageUp/Down, Home/End rows;
Enter details, Esc back, ? read-only help, q quit. In selected detail only:
W apply/reapply one Want, D delete one Want, A approve one Hold, X reject one
Hold. Enter never writes. Caption: Approve records a decision; it does not install.
Want has only title_id: no editor/priority/pause/reset. Want-only apply is NOT
equivalent to CLI watch (which also writes Title); describe precise apply context.
Hold target is exact release/object, not merely its shared TitleId.

Explicitly forbid Reconcile, Job create/bind/status/delete/cancel/retry,
Cluster/Secret/Title/Node/Event writes, Hold/RemoteFile Apply, maintenance,
select-all, confirmation/search prompts and external editors. Closed action enum.

## State and safety

Cache identity is (kind,name), retaining UID, resourceVersion, tombstones and
connection epoch. States Connecting/Synchronizing/Current/Stale. Initial Watch
at zero provides snapshot and events; List establishes a known-empty/populated
baseline because protocol has no snapshot-end marker. Never infer completion
from silence. On failure/EOF/decode/Expired show NOT CURRENT, disable writes,
retain data only as stale and replace cache from fresh bootstrap. Backoff
1/2/4/8 seconds capped; no periodic Lists when healthy. Bounded channels with
backpressure; reject old epochs and revisions. Local 1-second tick handles
freshness, redraw on changes at most 10Hz. Current means successful baseline plus
connected stream, not linearizable/exactly caught-up revision.

Hold inbox requires ready inventory Node, positive committed generation, fresh
heartbeat AND completed-list timestamp via node_is_ready, matching Hold
list_generation, Empty decision. Apply same generation/freshness to RemoteFiles.
Missing/future/expired/not-ready inventory is unavailable, not empty.

Before writes capture selected full identity/UID/revision/epoch. Fresh GET Want;
existing UID/revision must still match; absent creates with version zero only
after NotFound. Hold actions fresh unfiltered List verifies exact Hold and Node
generation together and displayed UID/revision. Exactly one versioned write,
never blind retry; server admission authoritative. Conflict refreshes with visible
message; post-submit transport uncertainty reports outcome unknown and refreshes
without resend. One mutation in flight, keys never queue; clear action selection
after every attempt, never retarget the next row. Ignore repeat/release/pasted
mutation input. Disable mutations while stale/undersized/identity clipped.

## Visual contract and QA

Write root DESIGN.md before widgets, with terminal research log and wireframes
140x40/80x24/60x16; browser/React/image/Lighthouse lanes inapplicable. Editorial
operations ledger: masthead, dominant table, restrained detail, aligned numeric
columns, scoped persistent footer. Terminal-native monospace, default background,
cyan focus, green success, yellow stale, red failure. Text labels/ASCII structure,
Unicode content width-aware and all untrusted control sequences sanitized.
NO_COLOR/--color never preserve meaning via text/reverse/bold. No animations or
blink, nested boxes. >=120 columns split detail; 60-119 single pane; minimum
60x16 otherwise resize notice and quit, mutations off. Independent detail scroll.
Reuse exact English empty-state strings only for genuinely known empty states.
Document terminal accessibility limitations and unchanged CLI alternative.

Headless deterministic TestBackend tests in default Cargo suite; styles AND text
at wide/narrow/min/undersize. State, key, conflict, generation, stale, epoch,
delete/recreate and revision-overlap tests. API-side real UDS integration tests
and local-only rich/empty/not-ready fixture; no GPU/SSH/seedbox/live-box. Actual
PTY manual QA outside make test must exercise seven screens, keys, mutations,
bad socket, loss/reconnect, freshness lapse, resize/monochrome/Unicode, no TTY,
help, error/panic/signal cleanup. Never claim manual QA from snapshots alone.

## I/O matrix

| Given | When | Then |
|---|---|---|
| Empty successful baseline | Connected watch starts | Known empty states render without waiting for events |
| Cached data | Stream fails/expires | NOT CURRENT, mutations disabled, reconnect from zero |
| Failed List | Bootstrap completes unsuccessfully | Unavailable, not empty |
| Revision overlap/delete/recreate | Events replay | No regression/resurrection/old UID selection |
| Ready committed generation | Open Holds projected | Only matching undecided releases visible |
| Clock advances past freshness | No new event | Inbox/listing unavailable |
| Selected object changed | Mutation requested | Refresh/message, no stale write/retry |
| Two Holds share TitleId | A/X on one detail | Only selected release changes |
| Mutation pending/repeat/paste | More mutation keys | No queued/batch write |
| Non-TTY or help | Launch | Plain refusal/help with no escapes |
| Normal/error/panic/SIGTERM | Exit | Terminal restored |

</frozen-after-approval>

## Code map and implementation notes

- crates/home-client/src/lib.rs: HomeApi::connect/get/list/apply/patch/delete/watch;
  default_api_socket(). watch returns raw tonic stream; existing delete GETs first.
- crates/proto/src/home_convert.rs: home_object_from_wire/to_wire; add typed watch
  decoding here or sibling home_watch.rs, keep all wire conversion in proto.
- proto/mediaops/home/v1/home.proto: List lacks collection RV, Watch lacks snapshot
  completion; Watch(0) snapshot then durable events, >0 replays history only.
- crates/api/src/serve.rs: serve_api(ApiConfig{socket,api_db}), abort handle to
  stop. No public in-process HomeSvc harness. Actual watch snapshots have cursor.
- crates/store/src/api.rs: retained history 4096; object revision can predate floor.
- crates/core/src/home.rs: HomeObject::new, specs/status, Actor matrix; WorkerKind
  node names, node_is_ready, TitleStatus::observed_files, CLUSTER_NAME.
- bins/mediaops/src/api_cmd.rs: read-only reference published_holds/status_pretty/
  watch_title/hold_decide. NEVER edit CLI files or import its renderer.
- crates/arch-tests/src/lib.rs: ALLOWED_WORKSPACE_EDGES, SEED_PACKAGES,
  live_metadata/add_direct_dep. Dev edges count too.
- Makefile: musl is daemon-only, install currently eight binaries.

Fresh investigation found max(List object RV) may predate retained Watch history,
so the plan's suggested bootstrap cursor can reconnect forever in a quiet store.
Preserve approved observable semantics without deriving resume cursor from old
objects: retain Watch(0) established before List and merge overlap with revisions/
tombstones (including deleted snapshot objects), or another tested gap-free
implementation. No wire schema/server behavior changes. Include regression for
old live objects after compaction and empty baseline. Library code details may
adapt to verified actual APIs without weakening any behavior above.

## Tasks & Acceptance

- [x] Architecture: register TUI->core/home-client and apiserver development-only
  ->TUI/home-client before manifests; assert forbidden production closures and
  ratatui/crossterm absent from CLI/daemon; include negative tests.
- [x] Client: additive typed HomeWatch::message and decoded Added/Modified/Deleted
  events, explicit-version delete_at_version; preserve old signatures/behavior.
- [x] UI: DESIGN.md then bins/mediaops-tui modules main/lib/args/terminal/runtime/
  session/model/update/actions/projection/disk/view; complete all screens/actions.
- [x] Tests: state/actions/render/arguments fixtures, real API socket integration
  in crates/api/tests/tui_session.rs and tui_actions.rs; no TUI->server dev edge.
- [x] QA support: crates/api/examples/tui_fixture/main.rs rich/empty/not-ready with
  local heartbeats and no workers; terminal_probe example for restoration.
- [x] Packaging: Makefile install and make tui; workspace members/pins/lock via
  Cargo only. Docs tui.md/tui-qa.md, README/index/usage/architecture/development,
  narrow AGENTS no-TUI exception. No changes to existing CLI.
- [x] Verification: targeted and workspace tests, fmt/clippy/proto/arch, normal
  dependency trees, real local PTY visual/interaction/restoration acceptance.

## Verification

cargo test -p mediaops-tui --locked; targeted proto/home-client/API tests;
make test OFFLINE=1; make test-arch OFFLINE=1; make fmt-check;
make clippy OFFLINE=1; make proto; cargo tree normal closures. make musl OFFLINE=1
only if toolchain available, separate from default tests. Do not rerun identical
green suites unless inputs change. Capture actual red/green evidence.

Local fixture CLI: cargo run -p mediaops-apiserver --example tui_fixture --locked
--offline -- /tmp/opencode/mediaops-tui-qa-UNIQUE rich; prints api.sock path.
Drive real TUI in tmux against it; document actual verification and gaps.

## Spec change log

2026-09-06: User approved full hyperplan unattended. Persisted active execution
contract in current docs rather than historical BMAD queue. Corrected bootstrap
implementation note using freshly verified retained-history semantics; observable
requirements unchanged. No automatic commits.

## Completion evidence

Implemented and independently reviewed on 2026-09-06. The full offline workspace
suite and 17 architecture tests passed. The new TUI has 58 passing unit,
interaction, argument and rendering tests; API-side TUI/fixture suites add 19
tests. Formatting, proto lint/format, strict package-only Clippy and the static
musl daemon build passed. Workspace Clippy exits successfully but retains
pre-existing warnings elsewhere; a workspace-wide `-D warnings` run is blocked
by existing core warnings. Rust LSP requests timed out; Cargo compilation and
Clippy were the available diagnostic gates.

Real PTY use verified Want creation/deletion, per-release Hold approval and
rejection, continued navigation during disconnect, reconnection, inventory
freshness lapse, all seven screens at 140x40/80x24/60x16, minimum-size refusal,
monochrome, Unicode/control-text rendering, and terminal restoration on normal
return, error, panic, Ctrl-C and SIGTERM. Independent runtime review passed;
independent visual review passed after narrow-footer and heading fixes.
Temporary captures and width reports are in `/tmp/opencode/heph-tui-evidence`.

Residual nonblocking verification gaps: no deterministic fault injection of a
successful write whose response is lost, nor of the exact Watch-snapshot/List
deletion race. Version, identity, epoch and no-retry behavior have focused tests
and source review. No live seedbox, GPU, deployment, install, commit or push was
performed. Existing CLI/daemon source and wire schemas are unchanged.

Following that implementation verification, the optimized TUI was installed for
operator testing. Holds and Titles were then updated to show readable metadata
labels while retaining ID-based selection and mutation targeting. Five label
regressions, strict TUI Clippy, live terminal checks, and independent visual and
identity-safety reviews passed. The operator accepted the updated installed
build and authorized committing the work to the existing branch.
