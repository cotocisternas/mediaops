# One-shot sync implementation contract

Approved scope: `mediaops sync` copies all completed eligible files in allowlisted
seedbox roots to home, independently of grabber monitoring. No persistent Wants
are created. `sync --dry-run` previews the same decisions without writing objects.
Continuous automatic sync is explicitly deferred in [backlog.md](backlog.md).

## Boundaries

Home API only from CLI/TUI; inventory alone talks to the seedbox through the
gateway. No new crate, seedbox protocol, worker, SSH/HTTP path, timer, deletion,
overwrite, Hold auto-approval, or global maintenance flock. Existing budgets,
per-file proofs, Job binding/recovery, Want/Hold semantics and JSON modes remain.
Do not deploy this development branch or run a real sync on the operator's
library during implementation. Use isolated local API fixtures for validation.

## Shared data and RPC contracts

Add `Kind::Sync`, `Spec::Sync(SyncSpec)`, `StatusBody::Sync(SyncStatus)` with types
in `crates/core/src/home_sync.rs`, re-exported by core. Preserve existing enum
tokens, serde defaults and protobuf field numbers. New JobSpec field `sync_name:
String` defaults empty and is omitted from JSON when empty. Existing jobs retain
their old authorization when empty.

Exact new types (all public, Clone/Debug/PartialEq/Eq/Serialize/Deserialize;
structs camelCase + deny_unknown_fields, defaults for persisted compatibility):
- `SyncScope`: `AllEligibleCompleted`, default, serde `allEligibleCompleted`.
- `SyncSpec { scope: SyncScope }`.
- `SyncPhase`: WaitingInventory (default), Captured, Scheduled, Failed; camelCase
  serde, `as_str()`/`parse(&str)->Result<Self,HomeError>` for wire conversions.
- `SyncDisposition`: WouldQueue (default), Queued, AlreadyQueued, Present,
  Blocked, Ineligible; camelCase serde and as_str/parse.
- `SyncEntry { remote_root:String, remote_path:String, file_len:u64,
  title_id:String, placement:Option<Placement>, job:Option<JobSpec>,
  disposition:SyncDisposition, reason:String, job_name:String, job_uid:String }`.
- `SyncStatus { phase:SyncPhase, accepted_unix:i64, deadline_unix:i64,
  baseline_generation:i64, acceptance_rv:i64, list_generation:i64,
  cluster:Option<ClusterSpec>, cluster_generation:i64,
  secret_resource_version:i64, entries:Vec<SyncEntry>, message:String }`.

Sync spec is empty intent beyond the fixed scope. Manifest/authority is entirely
server-owned status. Generic Apply/Patch of Sync is denied for all API actors;
Get/List/Watch work; Delete allowed for Cli only after all owned Jobs are terminal
or absent. A Sync never resumes planning after Scheduled, even if its Jobs are
deleted. Job identities include UID to prevent deleted/recreated Jobs reusing
old authorization.

Extend HomeService (home protocol only):
- `Sync(SyncRequest { request_id:string, dry_run:bool }) -> SyncResponse { object:Object }`.
  Actor Cli only. A real request uses a client-generated stable request ID;
  repeating it returns the same stored request, without changing its baseline.
  Dry-run uses an in-memory request and must not write any object/event.
- `BeginInventory(BeginInventoryRequest {}) -> BeginInventoryResponse { object:Object }`.
  Actor Inventory only. API mutation lock records a server-owned start token.

HomeApi methods `sync(&self, request_id:&str, dry_run:bool)->Result<HomeObject,
ClientError>` and `begin_inventory(&self)->Result<HomeObject,ClientError>`.
Add public home-client helper `new_sync_request_id()->String` using time/process
plus atomic counter. Preserve existing raw watch/delete methods.

New NodeStatus fields (default zero) `scan_started_rv:i64`,
`scan_cluster_generation:i64`, `scan_secret_resource_version:i64`. BeginInventory
sets ready=false and captures current global store revision as scan_started_rv
plus current Cluster generation and Secret RV under mutation serialization.
The start token must be newer than Sync acceptance_rv. Inventory calls this once
immediately before remote listing/hold fetch. Remove redundant ready=false writes
for that same scan; heartbeat touches preserve the scan metadata, and publication
must not overwrite it or commit a superseded scan token. Old inventory clients
cannot accidentally satisfy a fresh-sync request with zero start metadata.

## API algorithm

Real Sync RPC atomically creates/returns a WaitingInventory Sync with acceptance
store revision, inventory generation, Cluster spec/generation, Secret RV and
60-second planning deadline. Wake the existing controller. RPC waits boundedly
for Scheduled/Failed and returns immediately after planning, not copying.
Accepted work survives client disconnect/API restart. Expired waiting/captured
requests become Failed without scheduling. If the RPC boundary races the last
controller pass, return explicit pending state, not success.

The controller captures only a fresh successfully committed inventory whose
generation exceeds baseline and whose scan_started_rv exceeds acceptance_rv.
Require ready/fresh heartbeat and completion timestamp, and matching current
Cluster/Secret snapshot. A pre-request in-flight scan, partial/failed scan, or
changed configuration must not qualify. Waiting for the periodic inventory
cycle is sufficient; do not invent a second inventory worker.

Persist a finite Captured manifest once. Subsequent recovery re-evaluates only
those entries, never adds files from later inventories. Preview uses the same
read-only planner and freshness predicate but persists no Sync/Jobs/Titles/Events,
does not mark drift, and never calls BeginInventory/reconcile.

Plan decisions:
- Nonzero completed media in configured roots, canonical PathSchema placement,
  or an exact already-approved current Hold with authoritative placement.
- Respect undecided/rejected Holds for exact file/placement; no decisions changed.
- Compare canonical TitleId + FileKey with all recorded Title proofs (including
  imported authority-ID proofs and alternate extensions). Healthy proof plus
  regular on-disk file = Present. Missing/drifted recorded placement = Blocked.
- Existing unproved destination, symlink, unreadable path or unsafe parent =
  Blocked, never overwrite. Only real NotFound establishes absence.
- Multiple sources for one destination placement = Blocked ambiguous source.
- Matching nonterminal Job = AlreadyQueued, preserving its original authorizer;
  conflicting source or existing terminal Job = Blocked. Retry requires explicit
  terminal Job deletion and a NEW sync; never reset/recreate implicitly.

Observe filesystem outside the API mutation lock; before scheduling revalidate
configuration, committed source/Hold eligibility, proofs and Job conflicts.
Never widen captured scope. Atomically create missing Title shells + new Jobs,
record exact Job names/UIDs in Sync entries and mark Scheduled in one store
transaction with normal RV/history semantics. No scheduler may bind before this
transaction commits.

At Job creation and bind, a nonempty sync_name requires a Scheduled Sync entry
matching the exact Job UID/name and all immutable JobSpec fields except binding
fields. Existing source/listing/placement/Hold/Cluster/proof/budget checks still
apply. Sync Jobs require no Want. For unbound sync Jobs, temporary inventory
failure leaves Pending; a successful listing proving a changed/missing source
refuses it permanently. Revoked exact Hold authorization refuses it too.

## CLI and TUI

CLI: `mediaops sync [--dry-run] [--request-id ID] [--socket PATH]`.
Validate conflicting output flags before RPC. Default readable summary includes
request ID, captured generation, copy/reuse/present/blocked/ineligible counts,
each source and destination/reason. `-o json` emits the raw Sync object. The retired `--json` interface is not supported. Transport error names request ID for inspection/retry; dry-run/no-queue
must never print scheduled success. `mediaops get Sync ID` inspects durable result.
Keep implementation in a new sync_cmd.rs, not growing api_cmd.rs.

TUI: add explicit global `p` preview and `S` sync commands/effects, NOT variants
of existing four-object Mutation enum. Same HomeApi::sync RPC and request IDs.
Require Current, normal size, no help/pending operation; ignore repeat/paste.
Keep UI responsive; store and render report in a read-only preview/report pane
with counts and scrollable source/destination/reason rows, Esc returns. Help and
footer describe keys and clarify S schedules a fresh request, not permanent
watching. No confirmation prompts or automatic Hold decisions. No CLI formatter
imports; only core/home-client workspace dependencies.

## Verification and scope ownership

Core/wire unit: core/proto/home-client, inventory scan handshake and mechanical
compatibility changes to struct literals/exhaustive matches. Wire conversions
only in proto. Parent owns API/store controller, planning/authorization and tests.
UI unit: CLI sync_cmd/main dispatch, TUI global action/report view/tests, docs
usage/tui/DESIGN. Do not touch existing four mutation semantics.

Tests must cover: exact fresh-scan boundary; no-zero token; partial publication;
zero-write preview; no permanent Wants; new files after capture not selected;
atomic rollback; restart Captured; concurrent sync dedup; existing proof, drift,
unproved destination/symlink, duplicate sources; Hold blocking/revocation;
deleted/recreatedJob UID rejection; old serialized Job/Node shapes; JSON modes,
explicit globalTUI action and no Enter/paste/repeat writes. Real local API and
terminal QA only, no production sync/deployment or seedbox copy.

Run make proto, fmt-check, test-arch, targeted tests, full make test with sibling
build, Clippy under current policy, and a local CLI/TUI fixture. Record actual
verification; do not mark work complete from compilation alone.

## Implementation verification

Implemented on `feat/one-shot-sync` and verified on 2026-09-06:
- Full `make test OFFLINE=1`, formatting, protobuf lint/format and protobuf
  breaking checks against `main` passed.
- Strict package-only Clippy passed for the API and TUI; static musl daemon
  build passed with the seedbox wire unchanged.
- New regressions cover fresh scan boundaries and superseded publication,
  atomic rollback and exact Job UIDs, database-v3 migration preservation,
  captured-request restart, deadline failure, concurrent/repeated sync,
  zero-write previews, Hold conflicts, installed/unproved/symlinked placements,
  source-configuration changes, and deletion without implicit recreation.
- The real CLI/supervisor/loopback Range end-to-end test installed verified
  bytes without a Want and left later remote files outside the one-shot scope.
- Actual local CLI and TUI preview/scheduling, JSON/envelope output, Unicode
  wrapping, report indentation, End/Up scrolling and repeat-key protection were
  exercised. Independent server, input-safety and visual reviews passed.
- Rust LSP remained unavailable (daemon timeouts); compiler, tests and Clippy
  provided diagnostics. No production sync, deployment, commit or push occurred.

Deployment requires a coordinated Home API/inventory/client update. The API
database uses marker 4 to reject rollback to pre-Sync binaries; existing objects
and watch history are preserved on upgrade. Back up Home state before deployment.
On terminals without reliable key-release reporting, TUI `S` is limited to one
request per launch; use the CLI or relaunch for another sync. Automatic continuous
sync remains deferred in `docs/backlog.md`.
