---
stepsCompleted:
  - step-01-validate-prerequisites
  - step-02-design-epics
  - step-03-create-stories
  - step-04-final-validation
inputDocuments:
  - _bmad-output/specs/spec-mediaops/SPEC.md
  - _bmad-output/specs/spec-mediaops/module-map.md
  - _bmad-output/specs/spec-mediaops/grabber-inventory.md
  - _bmad-output/specs/spec-mediaops/bootstrap-surfaces.md
  - _bmad-output/specs/spec-mediaops/failure-history-tests.md
  - _bmad-output/specs/spec-mediaops/increments.md
  - _bmad-output/planning-artifacts/architecture/architecture-mediaops-2026-08-29/ARCHITECTURE-SPINE.md
---

# mediaops — work breakdown

## Overview

This document provides the complete epic and story breakdown for mediaops, decomposing SPEC-mediaops (the canonical contract), its companions, and the Architecture Spine into implementable stories.

Epics 1–4 are the first demo on this box (`increments.md`): scaffold → seedbox answers → home can pull and resume → unattended plan/apply plus Chrome-playable encode. `grabber=None` is a valid demo path. Epics 5–8 are remaining v1 (quiet-box apply, holds, reclaim, relocate/docs). CAP-11 LLM verbs, TUI, `ui <app>`, bearer-token 2FA, and a generalized wants queue stay deferred: v1 only reserves the capability-token enum in `core`.

## Requirements Inventory

### Functional Requirements

FR1 (CAP-1): `watch TITLE` records a per-title want and, when a grabber is configured, ensures *arr monitoring; it exits 0 when the want is recorded — not when the file is already playable.

FR2 (CAP-1): An unattended timer/`run` later delivers a schema-valid playable file on the home disk or an open hold, without occupying a console.

FR3 (CAP-1): Pull and encode honor remaining-home-disk and max-copy budgets; upgrade class defaults to never.

FR4 (CAP-2): Edge, Grab, and Paths desired state applies from a git-readable file; a second apply is a no-op when reality already matches.

FR5 (CAP-2): Unified diffs of ini, xml, and nginx render before any write.

FR6 (CAP-3): `why TITLE` / `status` show the grab → import → hold → pull → encode → library chain including stuck states (hold, watermark, lock, encode queue), with local FS as truth.

FR7 (CAP-3): When *arr believes a file is missing that exists locally, reconcile tells *arr to unmonitor.

FR8 (CAP-3): Disk-full is answered by seedbox df plus a reclaim preview ranked by ratio, private, and age.

FR9 (CAP-4): A copy killed mid-transfer resumes from `.partial`: resume lists completed ranges and continues; GC and empty-dir prune never delete partials.

FR10 (CAP-5): `seedbox bootstrap` brings a SwizzinBox or AlreadyThere box to desired state: mediaopsd installed and answering gRPC under bootstrap-minted mTLS, EdgeInvariant holds, indexer and client sets match desired state, packages and pins applied; Jellyfin/Plex untouched.

FR11 (CAP-5): API keys are discovered from remote `config.xml` / `sabnzbd.ini` at runtime — never pasted, stored masked, or echoed; UI is a presence boolean.

FR12 (CAP-5): Range-RPC concurrency N is probed until throughput plateaus, persisted, and re-probed only on endpoint/underlay change.

FR13 (CAP-6): `library bootstrap` creates schema dirs (`movies`/`series`/`music`) plus app-managed `_ops`/`_incoming` (never media-server libraries), watermarks, lock, systemd-user oneshot + `OnUnitInactiveSec` + flock, title-index sqlite, and NVENC cap probe; it refuses if the disk is below watermark and configures no media-server libraries, users, or clients.

FR14 (CAP-6): `library relocate` rewrites schema roots, systemd units, and title-index paths; `new-machine` exports/imports desired-state + tls dir + title index (bootstraps layout even before files exist).

FR15 (CAP-7): Plan is a first-class artifact with at least Copy, Skip, Review, Unmonitor, DeleteRemote, Encode, Reclaim plus EdgeApply/GrabApply reconcile steps; `run` = plan + apply; config is snapshotted at plan start.

FR16 (CAP-7): Lock conflict is a distinct exit code plus skip-with-reason, never silent 0; `status` shows who holds the lock (pid, started_at, command).

FR17 (CAP-8): The holds inbox lists import-blocked releases with age, size, and *arr reason; Approve promotes through the schema path; Reject means never-this-release and lets *arr try another; auto-approve is impossible; there is no agent-approve path in v1; blocked NZBs are never library.

FR18 (CAP-9): Reclaim preview is ranked and dry-run exists (or reclaim does not); before any remote library unlink qBit is queried and seeding skips; private-under-goal is untouched; usenet-complete is deletable after Copy; torrent delete belongs to reclaim only, never sync-after-copy.

FR19 (CAP-10): EncodePolicy maps (codec, depth, container, hdr) → Keep | NvencH264 | Refuse: HEVC-MP4 movies that break Chrome encode to H.264 8-bit under the probed NVENC cap; series-skip of HEVC-MP4 is an explicit named rule; HDR/DV and 2160p remux refuse (Keep-forever).

FR20 (CAP-10): Encode is a reversible transaction: write `.converting`, replace, move original to backup; the original is never deleted before replace succeeds; the encode queue is visible and pausable.

FR21 (CAP-11 — deferred): Agent research/debug ships post-v1. v1 reserves the capability-token enum in `core` only; no LLM runtime dependency, no agent-approve path.

FR22 (CAP-12): Scheduled doctor is read-only and detects panel edge rewrites; EdgeInvariant drift fails reconcile; a panel fingerprint change freezes apply until repair-edge is explicit.

FR23 (CAP-12): `repair edge` is one confirmed transaction (diff, then nginx + stack apply) gated by `--repair` plus a local confirm flag or pin; after install/upgrade an edge check is queued before success is reported.

FR24 (cross-cutting): `docs render` generates operator docs from PathSchema so generated docs cannot lie (replaces hand-edited SEEDBOX.md/AGENTS.md).

FR25 (cross-cutting): Every capability is a CLI subcommand with `--json`; timers never require a TUI; the CLI talks only to local mediaopsd over a unix socket.

FR26 (CAP-5): `seedbox upgrade` redeploys the daemon (copy binary + restart unit over ssh) — never a panel or package path — and warns/refuses on proto version skew.

### NonFunctional Requirements

NFR1 (Security — transport): The formal API is mediaopsd gRPC with mTLS only (v1); self-signed CA + server + client certs minted at bootstrap (rcgen, ECDSA P-256); rustls everywhere, native-tls forbidden; cert PEMs are gitignored files next to desired-state; desired-state stores SHA-256-of-DER fingerprints and paths, never PEMs; doctor refuses PEMs inside a git work tree.

NFR2 (Security — secrets): Zero secrets in git; API keys discovered at runtime, never stored or echoed; masked `********` values are never accepted.

NFR3 (Security — exposure): *arr HTTP never leaves localhost; mediaopsd is the only remote surface; the home CLI process never contains a seedbox address; no public WebUI or extra public status daemon.

NFR4 (Integrity): Verification is BLAKE3 per-range (recorded in the `.partial` sidecar) plus whole-file BLAKE3 at schema install; size/mtime is never proof; reclaim local-proof uses only the install digest.

NFR5 (Idempotency): Every mutation is an action in a Plan applied as idempotent transactions; running twice is a no-op when reality matches; wizard steps are reconciles, never once-only.

NFR6 (Safety): One-way pull; remote delete only for surplus after local hash proof; never two-way sync, never a third cloud; never torrent save paths or `torrents/incomplete`; remote walks use an allowlist, error on unknown paths, and never follow symlinks off it; encode never deletes an original before replace succeeds.

NFR7 (Resource budgets): `max_copy_gib`, `min_free_gib`, `max_nvenc`, and lock live in config — no magic numbers; preflight fails if a copy would breach the watermark; music-first then videos is a planner law.

NFR8 (Scheduling): systemd-user oneshot + `OnUnitInactiveSec` + flock, all three; no overlapping `OnCalendar` cron; config is never hot-reloaded mid-copy; jobs have state machines and timers only advance ready jobs.

NFR9 (Testability): Unit tests never require the live box or a GPU; failure history is the test suite — every named failure maps to at least one test citing it; live-box integration is behind an explicit cargo feature + env var, never default CI.

NFR10 (Performance): Parallel Range RPCs must beat the live FTP-PASV ~30 MiB/s ceiling; probed N means N independent TCP+TLS channels, never streams multiplexed onto one TCP connection.

NFR11 (Observability): `--json` on every command with a single `{ok, data, error}` envelope on stdout; tracing events on stderr (JSON lines when not a tty); logs land in journald via systemd-user; `status` surfaces lock holder and failed runs.

NFR12 (Placement): Encode runs on the home GPU only; the seedbox is dumb disk + pipe and never encodes.

NFR13 (Change management): Version pins are first-class with a per-provider OS compatibility matrix; upgrade is a conscious transaction that can refuse; panel click is never an upgrade path.

NFR14 (Identity): Identity is TitleId (kind + TMDB/TVDB/MBID), never a path string; local sqlite maps TitleId → path → inode/BLAKE3 so identity survives rename; music remasters keyed by MBID.

### Additional Requirements

**Greenfield / starter template:** No external starter template. The Architecture Spine prescribes the exact Cargo workspace shape (Structural Seed): virtual workspace manifest, `proto/` sources, crates `core`, `proto`, `store`, `net`, `ssh`, `arr`, `transfer`, `sync`, `encode`, `arch-tests`, bins `mediaops` + `mediaopsd`, `fixtures/`. Epic 1 Story 1 must scaffold this exactly, on Rust 1.98 / edition 2024 with the pinned stack (tonic 0.14.6, prost 0.14.4, rustls 0.23.43, rcgen 0.14.10, blake3 1.8.7, clap 4.6.6, rusqlite 0.40.2, tokio 1.53.1, reqwest 0.13.4, serde 1.0.229, toml 1.1.4, tracing 0.1.44/0.3.23, thiserror 2.0.20, anyhow 1.0.104, similar 3.2.0, serde_json 1.0.151, cargo_metadata 0.23.1, directories 6.0.0, fs4 1.1.0).

- AD-1: Binaries are composition roots only; all logic a test would want lives in library crates.
- AD-2: Crate dependency direction enforced in CI by the `arch-tests` member crate (cargo_metadata subgraph check + external bans: reqwest only under arr, rusqlite only under store, encode/store never in the mediaopsd tree, rsync/rclone/ftp/ssh2/russh/ffmpeg-next/native-tls nowhere).
- AD-3: One wire contract in the `proto` crate (`mediaops.v1`, tonic-prost-build); additive-only evolution; `proto` owns wire↔domain conversions, the canonical `ControlPort` implementation (trait defined in `core`), and the only `ErrorDetail`↔`tonic::Status` builders.
- AD-4: Executor split — the flock-holding CLI is the only executor of plan/apply/pull/verify/install/encode; home mediaopsd is a gateway only (proxies control + Range streams, never writes staging/library); planning is home-side; the qBit seeding guard lives inside the seedbox DeleteRemote handler; lock classes fixed (exclusive verbs take flock; lock-free verbs do single-transaction row writes only).
- AD-5: One daemon binary, role (seedbox bind vs home unix-socket gateway) from config; reverse-connect stays a designed-unused mode.
- AD-6: Three data tiers — desired-state TOML (user, git-versionable), machine state (sqlite + probes, app-owned), runtime artifacts (lockfile, `.partial` + sidecar, tls/, plan JSONs under `~/.local/state/mediaops/plans/`).
- AD-7: Active config dir `~/.config/mediaops/` (desired-state.toml + tls/); machine state in `~/.local/state/mediaops/`; bootstrap refuses to mint certs into a git work tree; new-machine import populates the active dir, never a work tree.
- AD-8: One home `state.db` touched only by `store` (rusqlite behind spawn_blocking, embedded forward-only migrations via PRAGMA user_version); repository traits in `core`; tables `title_index` (dual digests: immutable `install_b3`, gate-updated `current_b3`), `jobs`, `probes`, `holds_decisions` (keyed by `HoldKey {title_id, release_id}`); seedbox daemon role links no sqlite.
- AD-9: A Plan embeds the exact raw TOML bytes of the snapshotted desired-state plus blake3(bytes); apply re-parses only from embedded bytes and refuses on hash mismatch; `Action` is one exhaustive enum with a `never` default arm.
- AD-10: Every long-running operation (want, pull, encode, hold) is a `jobs` row; `core::jobs` owns state enums and pure `advance()`; readiness predicates evaluated in `core::jobs`; planner links action jobs to wants via `parent_job_id`; crash recovery derives from job state + runtime artifacts.
- AD-11: `.partial` staging format — `<final>.partial` + sidecar `<final>.partial.b3` (versioned JSON: file_len, range_len, ranges with offset/len/blake3, all bytes); a range counts completed only after fsync + hash recorded; staging layout `_incoming/<TitleId>/…` rendered solely by `core::pathschema::staging_path`; any dir under `_incoming/` containing `*.partial*` is sacred to prune/GC.
- AD-12: Parallelism is a channel pool owned by the home gateway — N independent TCP+TLS channels, one GetRange stream per channel, N+1th refused ResourceExhausted; CLI sets N via gateway-only `ConfigurePool` UDS RPC; `probes` keyed by `endpoint_fingerprint`, re-probe on mismatch; range_len default 64 MiB from desired-state.
- AD-13: `core::pathschema` is the only renderer/parser of library paths; install gate has exactly two entry points (`install`, `replace` — replace is encode's path and sole `current_b3` writer); one allowlist walker produces typed `RemoteRef`/`RemoteEntry`; the Transfer service and planner consume the same shapes; `docs render` renders from PathSchema.
- AD-14: TLS mechanics — rcgen ECDSA P-256 CA/server/client at bootstrap into `tls/`; SHA-256-of-DER lowercase-hex fingerprints in desired-state; server requires-and-verifies client certs against the minted CA.
- AD-15: All grabber HTTP through an `HttpTransport` trait; reqwest impl linked only in mediaopsd; tests replay cassettes; every named grabber failure gets a cassette.
- AD-16: ffmpeg/ffprobe/ssh/systemctl invoked through a single exec port with probed absolute paths persisted in machine state; no lib bindings (no ffmpeg-next, ssh2, russh); system ssh honors `Host seedbox`.
- AD-17: `core` owns an exhaustive ExitCode enum (0 ok, 1 runtime, 2 usage, 3 lock conflict, 4 drift/verify, 5 policy refusal); libraries return thiserror and never exit; each binary maps error→ExitCode in one place; ExitCode reflects the command's own contract (refusals inside an apply loop are data, not exit 5).
- AD-18: stdout carries only the result (human or single JSON envelope); stderr carries tracing; progress is tracing events (the deferred TUI attaches there).
- AD-19: tokio is the only executor; blocking work through spawn_blocking; whole-file hashing uses blake3 rayon; subprocesses via tokio::process under the exec port.
- AD-20: Offline tests are the suite — cassettes, tree fixtures, exec-port transcripts, in-memory sqlite, schema round-trip CI; live box behind feature + env only.
- AD-21: `Provider` trait in core; v1 ships SwizzinBox (impl in ssh) + AlreadyThere (core); all other variants return `Unimplemented` errors with tests asserting loud failure.
- AD-22: Seedbox mediaopsd builds statically as `x86_64-unknown-linux-musl`; redeploy is `seedbox upgrade` (re-run of bootstrap's install step); Control responses carry daemon semver + proto package; CLI refuses unknown proto package, warns on minor skew.

**Scope boundaries (increments.md):**

- First demo must run on this box: bootstrap (gRPC/mTLS up, home unix socket) → plan → parallel Range pull on allowlisted paths → `.partial` resume with per-range BLAKE3 → whole-file BLAKE3 + schema install → at least one HEVC-MP4 movie encoded under the probed NVENC cap. `grabber=None` is a valid demo path.
- Designed, unused by default: reverse-connect; Tailscale/WireGuard underlay.
- Deferred (capability kept, not built): TUI, `ui <app>`, CAP-11 LLM agents, bearer-token 2FA, generalized wants queue.
- Forbidden: emergency rsync-ssh (advertised or not), Autobrr/Bazarr (including stubs), agent auto-approve/confidence floor.

### UX Design Requirements

N/A — no UX design contract exists and none is required: the product is CLI-first (`--json` everywhere); the TUI is explicitly deferred and is a skin over the tracing stream, not a second API.

### FR Coverage Map

FR1: Epic 4 — `watch TITLE` records a per-title want
FR2: Epic 3 (timer) + Epic 4 (`run`) — unattended delivery
FR3: Epic 4 — budgets and upgrade-class default never
FR4: Epic 5 — Edge/Grab/Paths apply; second apply is a no-op
FR5: Epic 5 — unified diffs of ini/xml/nginx before write
FR6: Epic 4 (pull/encode/lock/watermark) + Epic 7 (grab/hold/reclaim) — `why` / `status` chain
FR7: Epic 7 — Unmonitor when local exists and *arr thinks missing
FR8: Epic 7 — seedbox df + ranked reclaim preview
FR9: Epic 3 — `.partial` resume; GC never deletes partials
FR10: Epic 2 (daemon, mTLS, probe) + Epic 5 (packages, edge, grabber sets)
FR11: Epic 5 — API key discovery, never paste/store/echo
FR12: Epic 2 — probe N until plateau; persist; re-probe on endpoint change
FR13: Epic 3 — `library bootstrap`
FR14: Epic 8 — `library relocate` and `new-machine`
FR15: Epic 1 (Plan/Action types) + Epic 4 (plan/run Copy/Skip/Encode) + Epics 5–7 (remaining actions)
FR16: Epic 3 (flock) + Epic 4 (`status` lock holder, exit 3)
FR17: Epic 6 — holds inbox
FR18: Epic 7 — reclaim preview/apply with qBit guard
FR19: Epic 4 — EncodePolicy Keep | NvencH264 | Refuse
FR20: Epic 4 — reversible `.converting` replace
FR21: Epic 1 — capability-token enum reserved in `core` only
FR22: Epic 5 — scheduled doctor read-only; panel fingerprint freeze
FR23: Epic 5 — `repair edge` confirmed transaction
FR24: Epic 8 — `docs render` from PathSchema
FR25: Epics 2–8 — every CLI verb has `--json`; CLI talks only to local mediaopsd
FR26: Epic 5 — `seedbox upgrade` + proto skew

## Planned increments

### Epic 1: The laws live in types
The home repo compiles as the Architecture Spine's structural seed. Identity, paths, desired-state, plans, jobs, and the gRPC contract are types plus offline tests — not comments in tribal Python. Nothing talks to the live box yet. Enables every later epic.
**FRs covered:** FR15 (Plan/Action types), FR21, FR25 (envelope + ExitCode), NFR9, NFR11, NFR14, AD-1–AD-3, AD-8–AD-10, AD-13, AD-17–AD-20

### Epic 2: Seedbox answers on this box
Operator runs `seedbox bootstrap` and health is gRPC: mediaopsd binds Control + Transfer under bootstrap-minted mTLS. Range concurrency N is probed and persisted. Grabber apply and edge repair wait for Epic 5; `grabber=None` is legal here.
**FRs covered:** FR10 (daemon/mTLS slice), FR12, FR25, NFR1, NFR3, NFR10, AD-5, AD-7, AD-12, AD-14, AD-16, AD-21, AD-22

### Epic 3: Home disk can receive a file
Home mediaopsd is a unix-socket gateway. `library bootstrap` stands up schema dirs, lock, timer, and title-index. `PullFile` copies allowlisted remotes through `.partial` with per-range BLAKE3 and resumes after a kill. No planner or encode yet.
**FRs covered:** FR9, FR13, FR16 (lock exists), FR25, NFR4, NFR6, NFR8, AD-4 (gateway), AD-11, AD-12

### Epic 4: Tonight-playable on this box
`plan` / `run` / `watch` / `why` / `status` reconcile Copy/Skip/Encode under budgets. Encode makes HEVC-MP4 movies Chrome-playable under the probed NVENC cap. Story 4.3 is the live first demo.
**FRs covered:** FR1, FR2, FR3, FR6 (pull/encode/lock/watermark slice), FR15 (Copy/Skip/Encode apply), FR16, FR19, FR20, FR25, NFR7, NFR12

### Epic 5: Quiet box without the panel
Edge, Grab, and Paths apply from git-readable desired-state with diffs first. Keys are discovered, never pasted. Scheduled doctor is read-only; repair and upgrade are explicit transactions.
**FRs covered:** FR4, FR5, FR10 (packages/edge/grabber slice), FR11, FR22, FR23, FR26, NFR2, NFR5, NFR13, AD-15

### Epic 6: Holds are an inbox
Import-blocked releases are a typed inbox with age, size, and reason. Approve promotes through PathSchema; Reject means never-this-release. Auto-approve is impossible.
**FRs covered:** FR17

### Epic 7: Reclaim after local proof
`why` grows the grab→hold→reclaim chain. Unmonitor tells *arr the truth when the file is already local. Reclaim preview is ranked; apply queries qBit and never deletes seeding or private-under-goal.
**FRs covered:** FR6 (remaining chain), FR7, FR8, FR18, NFR4 (reclaim digest)

### Epic 8: Move the library, render the docs
`library relocate` and `new-machine` move the archive without hand-editing paths. `docs render` generates operator docs from PathSchema so they cannot lie.
**FRs covered:** FR14, FR24

## Epic 1: The laws live in types

The home repo is the source of truth for how mediaops may be built. After this epic a developer can compile the workspace, CI fails illegal crate edges, and identity / paths / plans / the wire contract are tested offline. No live box, no GPU.

### Story 1.1: Workspace scaffold and dependency law

As an operator,
I want the Architecture Spine's Cargo workspace to exist with CI that fails illegal crate edges,
So that later stories cannot smuggle grabber HTTP into the CLI or FTP into transfer.

**Acceptance Criteria:**

**Given** a greenfield checkout
**When** I inspect the tree
**Then** it matches the spine Structural Seed: virtual workspace `Cargo.toml`, `proto/` sources, crates `core`, `proto`, `store`, `net`, `ssh`, `arr`, `transfer`, `sync`, `encode`, `arch-tests`, bins `mediaops` and `mediaopsd`, and `fixtures/`
**And** the toolchain is Rust 1.98 / edition 2024 with the spine's pinned crate versions

**Given** `crates/arch-tests`
**When** CI (or `cargo test -p mediaops-arch-tests`) runs
**Then** the workspace-internal graph is a subgraph of AD-2
**And** the run fails if `reqwest` appears outside `arr`, `rusqlite` outside `store`, `encode`/`store` appear in the mediaopsd tree, or `rsync`/`rclone`/`ftp`/`ssh2`/`russh`/`ffmpeg-next`/`native-tls` appear anywhere

**Given** both binaries
**When** they start
**Then** they are composition roots only (AD-1): parse/serve, snapshot config, lock, call libraries, render output
**And** `core` owns exhaustive `ExitCode` (0 ok, 1 runtime, 2 usage, 3 lock conflict, 4 drift/verify, 5 policy refusal)
**And** stdout is a human result or a single `{ok, data, error}` JSON envelope; stderr is tracing (JSON lines when not a tty)

**Given** the test suite
**When** it runs with default features
**Then** no test requires network, the live box, or a GPU (AD-20)
**And** `core` reserves a capability-token enum for CAP-11 with no LLM runtime dependency (FR21)

### Story 1.2: PathSchema, TitleId, and the walker

As an operator,
I want TitleId identity and PathSchema as the only path renderer/parser,
So that library names cannot lie and remote walks cannot leave the allowlist.

**Acceptance Criteria:**

**Given** a TitleId `kind:source:id` (movie TMDB, series TVDB, album MBID)
**When** PathSchema renders then parses it
**Then** `parse(render(id)) == id`
**And** year lives in the folder and the file the same way; spaces are refused; remasters key by MBID not folder year

**Given** names containing REPACJ, REPACK, or PROPER
**When** the scene-tag strip runs
**Then** those tags are removed
**And** explicit reject bins include `needs-split` and `needs-year`

**Given** a remote root allowlist
**When** the walker lists
**Then** it produces only typed `RemoteRef` / `RemoteEntry`
**And** unknown paths error; it never follows symlinks off the allowlist; torrent save paths and `torrents/incomplete` are not listed

**Given** staging
**When** any crate needs a staging path
**Then** it calls only `core::pathschema::staging_path` (`_incoming/<TitleId>/…`)
**And** the install gate exists as two entry points (`install`, `replace`) even if callers other than tests wait for later stories

### Story 1.3: DesiredState, Plan, jobs, and store

As an operator,
I want desired-state, Plan, and job state machines persisted in one home sqlite,
So that timers advance ready work and a crash does not invent progress.

**Acceptance Criteria:**

**Given** a desired-state TOML
**When** `core` parses it
**Then** `DesiredState` uses `deny_unknown_fields` and requires `schema_version`
**And** size fields are unit-suffixed (`max_copy_gib`, `min_free_gib`, `range_len_mib`) and convert to `Bytes(u64)` at parse — no bare integer size crosses a crate boundary

**Given** a Plan
**When** it is written
**Then** it embeds the exact raw TOML bytes of the snapshotted desired-state plus `blake3(bytes)`
**And** `Action` is one exhaustive enum: Copy, Skip, Review, Unmonitor, DeleteRemote, Encode, Reclaim, EdgeApply, GrabApply, matched with a `never` default

**Given** `core::jobs`
**When** a caller advances state
**Then** `advance(state, event)` is the sole state write and illegal transitions error
**And** repository traits live in `core`; `store` is the adapter (rusqlite behind `spawn_blocking`, forward-only migrations via `PRAGMA user_version`)

**Given** the first story that needs persistence
**When** `store` migrates
**Then** it creates `title_index` (`install_b3` immutable, `current_b3` gate-updated) and `jobs`
**And** `probes` and `holds_decisions` are not created yet — those tables land with the stories that write them
**And** neither daemon role links `store`

### Story 1.4: Wire contract in the proto crate

As an operator,
I want one generated `mediaops.v1` wire contract,
So that daemon and CLI cannot drift on RPC shapes.

**Acceptance Criteria:**

**Given** `.proto` sources under package `mediaops.v1`
**When** the `proto` crate builds
**Then** codegen is `tonic-prost-build` (tonic 0.14.6 / prost 0.14.4)
**And** `proto` is the sole home of wire↔domain `From`/`TryFrom`

**Given** Control and Transfer services
**When** messages are inspected
**Then** `RemoteRef` / `RemoteEntry` mirror `core` field-for-field
**And** `ErrorDetail {exit_code, reason, message}` plus the only two `tonic::Status` build/parse functions live in `proto`

**Given** `core::ControlPort`
**When** the canonical implementation is used
**Then** it lives in `proto` over generated clients
**And** wire evolution inside `mediaops.v1` is additive-only

## Epic 2: Seedbox answers on this box

Operator can bring this SeedIt4Me/Swizzin box (or AlreadyThere) to a daemon that answers gRPC under mTLS. Grabber HTTP apply and nginx repair are Epic 5. `grabber=None` is valid.

### Story 2.1: net crate — TLS identity and channels

As an operator,
I want bootstrap-minted mTLS and a channel-pool primitive,
So that the only remote surface is mediaopsd and Range RPCs cannot collapse onto one TCP.

**Acceptance Criteria:**

**Given** bootstrap minting
**When** `net` runs rcgen
**Then** it produces ECDSA P-256 CA, server, and client certs
**And** rustls server config requires-and-verifies client certs against that CA; `native-tls` is forbidden (arch-tests already bans it)

**Given** UDS and TCP
**When** serve/connect run in tests
**Then** both transports work through the same rustls config
**And** `endpoint_fingerprint` is a hash of seedbox address + underlay mode

**Given** the channel-pool primitive
**When** N slots are configured
**Then** it is N independent TCP+TLS channels, one in-flight stream per channel
**And** unit tests do not need a live box

### Story 2.2: mediaopsd seedbox role

As an operator,
I want one daemon binary whose seedbox role binds Control + Transfer,
So that health after bootstrap is gRPC, not SSH.

**Acceptance Criteria:**

**Given** config role = seedbox
**When** mediaopsd starts
**Then** it binds TCP gRPC+mTLS and serves Transfer (listing, Stat, streaming GetRange) backed by the one walker
**And** Control includes at least `df`, version + proto-package handshake; `grabber=None` means no live *arr calls

**Given** the seedbox role
**When** the binary is linked
**Then** it does not link `store` or `encode`
**And** reverse-connect remains a designed-unused mode of this same binary (no third binary)

**Given** a Control response
**When** the CLI reads it
**Then** it carries daemon semver + proto package name

### Story 2.3: Seedbox bootstrap over ssh

As an operator,
I want `mediaops seedbox bootstrap` to install a musl-static mediaopsd and mint certs,
So that a new box answers gRPC under mTLS without me SSHing daily.

**Acceptance Criteria:**

**Given** `~/.ssh/config` Host `seedbox`
**When** I run `mediaops seedbox bootstrap --json`
**Then** it imports that host (no invented alias format), builds `x86_64-unknown-linux-musl` mediaopsd, copies the binary and a systemd user unit, mints certs into the active config dir (`~/.config/mediaops/tls/`), and refuses to mint into a git work tree
**And** desired-state stores SHA-256-of-DER lowercase-hex fingerprints and paths, never PEMs

**Given** Provider
**When** the target is SwizzinBox or AlreadyThere
**Then** bootstrap completes (AlreadyThere is no-op install)
**And** unimplemented providers (DockerCompose, Ultra.cc, QuickBox) return errors with tests that they fail loudly — never `Ok`

**Given** gRPC is up
**When** bootstrap probes Range concurrency
**Then** it raises N until throughput plateaus, persists N keyed by `endpoint_fingerprint` in `probes` (this story adds the `probes` table)
**And** re-probe is not every run — only on fingerprint mismatch

**Given** unit tests
**When** they cover bootstrap
**Then** they use exec-port transcripts (AD-16); bulk copy over SSH is a test failure
**And** this story's live execution may touch the SeedIt4Me box — destructive steps must be surfaced before they run

## Epic 3: Home disk can receive a file

Home is a gateway plus a library disk plus PullFile. Planner and encode wait for Epic 4.

### Story 3.1: Home gateway

As an operator,
I want home mediaopsd on a unix socket as an overlay gateway,
So that the CLI never contains a seedbox address.

**Acceptance Criteria:**

**Given** config role = home
**When** mediaopsd starts
**Then** it binds a unix socket, holds the client cert, and re-serves seedbox Control + Transfer
**And** it proxies Status code, message, and details byte-for-byte; it never writes staging or library paths

**Given** `ConfigurePool`
**When** the CLI sets N at run start
**Then** each proxied Range stream pins 1:1 to a dedicated upstream channel
**And** the N+1th concurrent stream is refused `ResourceExhausted`, never queued onto a shared channel

**Given** the AD-12 failure-history test
**When** N concurrent streams run against a fake transport
**Then** they produce N distinct upstream connections
**And** status RPC exposes `endpoint_fingerprint`

### Story 3.2: transfer — pull, verify, resume

As an operator,
I want PullFile with `.partial` resume and BLAKE3 proof,
So that a killed copy continues instead of restarting.

**Acceptance Criteria:**

**Given** an allowlisted `RemoteRef`
**When** PullFile runs
**Then** it writes `<final>.partial` plus sidecar `<final>.partial.b3` (versioned JSON: `file_len`, `range_len`, ranges with offset/len/blake3, all bytes)
**And** a range counts completed only after fsync and hash recorded; resume uses the sidecar's `range_len`, never current config

**Given** a copy killed mid-file
**When** resume lists then continues
**Then** completed ranges are skipped and the rest transfer
**And** any dir under `_incoming/` containing `*.partial*` is sacred to GC and empty-dir prune

**Given** the files-first scheduler
**When** multiple files and ranges are in flight
**Then** slots fill files-first, then split the largest remaining file
**And** size/mtime is never treated as proof; whole-file BLAKE3 is computed at the install gate (callers of `install()` may wait for the next story)

### Story 3.3: Library bootstrap and scheduler

As an operator,
I want `mediaops library bootstrap` to stand up the archive disk,
So that plan, lock, timer, and encode have a place to live — without installing a media server.

**Acceptance Criteria:**

**Given** a home disk and desired-state
**When** I run `mediaops library bootstrap --json`
**Then** schema dirs `movies` / `series` / `music` exist plus app-managed `_ops` and `_incoming` (never media-server libraries)
**And** it refuses if free space is below `min_free_gib`; it configures no Jellyfin/Plex libraries, users, or clients
**And** if a media server is already present with `_incoming` or `_ops` as a library, it warns and does not reconfigure

**Given** scheduler setup
**When** bootstrap finishes
**Then** systemd-user `mediaops-run.service` (oneshot) + `mediaops-run.timer` (`OnUnitInactiveSec`) exist, and the CLI takes a machine-global flock (fs4)
**And** there is no `OnCalendar` overlapping timer; lockfile records pid, started_at, command

**Given** NVENC
**When** bootstrap probes
**Then** the cap is persisted to machine state (this box's live cap is config, not a hardcoded 8)
**And** unit tests fake the exec port — they do not need a GPU

**Given** title-index
**When** bootstrap creates `state.db`
**Then** `title_index` is ready to record install digests on later `install()` calls
**And** `install(TitleId, verified staging handle)` is wired as the only library-path writer besides `replace`

## Epic 4: Tonight-playable on this box

Unattended plan/apply plus encode. First demo on this box. Grabber apply, holds inbox, and reclaim are later epics; `grabber=None` is the demo path.

### Story 4.1: sync plan/apply and CLI verbs

As an operator,
I want `plan`, `run`, `watch`, `why`, and `status`,
So that I can enqueue a title and later get a schema file without occupying a console.

**Acceptance Criteria:**

**Given** listings + title-index + snapshotted desired-state
**When** the pure planner runs
**Then** it emits Copy and Skip against index and budgets, music-first then videos
**And** upgrade class defaults to never; preflight fails if a copy would breach `min_free_gib`

**Given** `run`
**When** it executes
**Then** it is plan then apply of that exact Plan artifact in the same locked process
**And** apply re-parses config only from embedded TOML bytes and refuses on hash mismatch
**And** apply advances ready jobs only; lock conflict is ExitCode 3 plus skip-with-reason, never silent 0

**Given** `watch TITLE`
**When** it succeeds
**Then** it records a per-title want (and monitoring only if a grabber is configured — not required for this story) and exits 0
**And** it does not wait for the file to be playable

**Given** `why TITLE` / `status --json`
**When** a title is in pull, watermark, lock, or encode-queue
**Then** those stuck states are visible with local FS as truth
**And** grab/hold/reclaim slices of the chain may still be absent until Epics 6–7

**Given** CLI
**When** any of these verbs run
**Then** `--json` uses the single envelope; the process talks only to local mediaopsd over UDS
**And** Review/Unmonitor/DeleteRemote/Reclaim/EdgeApply/GrabApply exist on the Action enum but are not required to apply yet

### Story 4.2: encode

As an operator,
I want EncodePolicy execution on the home GPU,
So that Chrome can play HEVC-MP4 movies without destroying originals or encoding Keep-forever titles.

**Acceptance Criteria:**

**Given** EncodePolicy `(codec, depth, container, hdr)`
**When** a file is classified
**Then** the result is Keep | NvencH264 | Refuse
**And** HEVC-MP4 movies that break Chrome map to NvencH264 targeting H.264 8-bit; series-skip of HEVC-MP4 is an explicit named rule; HDR/DV and 2160p remux are Refuse/Keep-forever

**Given** an NvencH264 job
**When** encode runs
**Then** it writes `.converting`, calls install-gate `replace()`, then moves the original to backup
**And** `replace` is the only writer of `current_b3`; the original is never deleted before replace succeeds

**Given** the encode queue
**When** `encode pause` is used
**Then** it is a store flag the executor polls between jobs, never a signal to the lock holder
**And** concurrency honors probed NVENC cap and `max_nvenc`; encode is linked only into the CLI tree, never mediaopsd

**Given** unit tests
**When** they cover policy and the reversible transaction
**Then** they do not require a GPU; named failures (HEVC-MP4 Chrome, HDR refuse, delete-before-replace) have tests citing them

### Story 4.3: First demo on this box

As an operator,
I want the first demo runbook executed on this box,
So that bootstrap → plan → parallel pull → `.partial` resume → schema install → one NVENC encode is proven live.

**Acceptance Criteria:**

**Given** Epics 1–4 code
**When** the demo runs against the live SeedIt4Me box and home GPU
**Then** both bootstraps succeed (gRPC/mTLS up, home UDS), a plan is produced, a parallel Range pull beats the need for FTP-PASV, a kill at ~90% resumes from `.partial`, whole-file BLAKE3 + schema install succeed, and one HEVC-MP4 movie encodes under the probed NVENC cap
**And** `grabber=None` is an accepted path (a folder on the box, a disk at home)

**Given** this story
**When** it is implemented
**Then** live steps stay behind the explicit cargo feature + env var (AD-20)
**And** a demo runbook is produced; anything destructive is surfaced before execution
**And** default CI still does not require the live box or GPU

## Epic 5: Quiet box without the panel

Operator keeps grabber, edge, and path desired-state correct from a git-readable file. Packages/nginx that bootstrap skipped beyond daemon install land here.

### Story 5.1: arr crate over HttpTransport

As an operator,
I want complete Servarr/SAB/qBit clients behind one transport port,
So that grabber failures are cassettes, not live clicks.

**Acceptance Criteria:**

**Given** the `arr` crate
**When** it speaks HTTP
**Then** it uses only `HttpTransport`; the reqwest impl is linked only inside mediaopsd
**And** tests replay JSON request/response cassettes; every named grabber failure in `failure-history-tests.md` that is an HTTP failure has a cassette

**Given** Autobrr or Bazarr
**When** the tree is searched
**Then** they do not exist, including stubs

### Story 5.2: Grab apply as set-diff

As an operator,
I want indexer and client sets applied from desired-state,
So that a second apply is a no-op when Grab already matches.

**Acceptance Criteria:**

**Given** desired-state indexer/client sets keyed by name + priority
**When** GrabApply runs
**Then** it is set-diff: PUT missing, delete extras; duplicate add is a conflict, not an append
**And** custom-format packs are re-PUT on apply; GrabPolicy changes are explicit commands with a diff

**Given** API keys
**When** Test or apply needs a key
**Then** it is discovered from remote `config.xml` / `sabnzbd.ini` at runtime
**And** masked `********` is never stored or accepted; UI is a presence boolean

**Given** `mediaops seedbox apply --json` (or equivalent GrabApply verb)
**When** it completes twice with no desired-state change
**Then** the second run is a no-op
**And** the CLI still does not speak Sonarr HTTP — only local mediaopsd Control

### Story 5.3: Edge apply, doctor, and repair

As an operator,
I want EdgeInvariant enforced with read-only doctor and a confirmed repair,
So that a panel rewrite cannot silently break bind/`url_base`.

**Acceptance Criteria:**

**Given** ini/xml/nginx that would change
**When** EdgeApply or repair is about to write
**Then** a unified diff from the core `similar` module renders first
**And** EdgeInvariant is bind `127.0.0.1` + `url_base` + Host `$host` + Forms auth; Prowlarr app URLs include `url_base`

**Given** scheduled doctor
**When** it runs unattended
**Then** it is read-only Control + local checks
**And** EdgeInvariant drift fails reconcile; a panel fingerprint change freezes apply until repair-edge is explicit
**And** doctor refuses if cert PEMs sit inside a git work tree

**Given** `repair edge`
**When** it runs
**Then** it is one transaction (diff, then nginx + stack apply) gated by `--repair` plus a local confirm flag or pin
**And** `doctor --repair` from an unattended public laptop is a named failure with a test
**And** after install/upgrade an edge check is queued before success

### Story 5.4: Seedbox upgrade and version pins

As an operator,
I want `seedbox upgrade` and a pin matrix that can refuse,
So that Lidarr glibc traps and panel-click upgrades cannot happen.

**Acceptance Criteria:**

**Given** `mediaops seedbox upgrade --json`
**When** it runs
**Then** it re-runs bootstrap's install step (copy musl-static binary + restart unit over ssh)
**And** it is never a panel or package-manager path

**Given** Control proto package / semver
**When** the CLI connects
**Then** it refuses an unknown proto package and warns on minor skew

**Given** version pins (including Lidarr OS compatibility)
**When** an upgrade would violate the matrix
**Then** the transaction can refuse (ExitCode 5 for that verb)
**And** a test cites the Lidarr glibc trap

## Epic 6: Holds are an inbox

Import-blocked is a product feature, not a junk drawer.

### Story 6.1: Hold store and inbox join

As an operator,
I want holds persisted and joined against live grabber queue,
So that I can list age, size, and *arr reason without ingesting blocked NZBs.

**Acceptance Criteria:**

**Given** `store`
**When** this story migrates
**Then** `holds_decisions` exists keyed by `HoldKey {title_id, release_id}` (`release_id`: usenet NZB-name hash or torrent infohash)
**And** `release_id` is defined in `core`, carried in `proto` hold messages, and mapped from Servarr queue items only inside `arr`

**Given** `hold list --json`
**When** live queue and decisions differ
**Then** the inbox is live ⊖ decided, computed in `sync`
**And** blocked NZBs are never library paths; `_incoming` is not a hold folder a media server might ingest

### Story 6.2: Approve and Reject

As an operator,
I want Approve and Reject on a hold,
So that I can promote a release through PathSchema or tell *arr to try another — never auto-approve.

**Acceptance Criteria:**

**Given** a listed hold
**When** I `hold approve`
**Then** the release is promoted through the install gate / schema path
**And** PathSchema still refuses spaces and leftover scene tags

**Given** a listed hold
**When** I `hold reject`
**Then** that release is never-this-release and *arr may try another
**And** there is no auto-approve path and no agent-approve / confidence floor in v1

**Given** Research
**When** the CLI is inspected
**Then** a Research verb may exist as a stub or be omitted until CAP-11; it must not call an LLM in v1

## Epic 7: Reclaim after local proof

Free remote buffer only after local BLAKE3 proof. Complete the why-trace.

### Story 7.1: Unmonitor and why-trace completion

As an operator,
I want Unmonitor and a full why-trace,
So that *arr is not the catalog and stuck states are visible end-to-end.

**Acceptance Criteria:**

**Given** a title that exists locally (install digest in title-index) while *arr still monitors it as missing
**When** reconcile/Unmonitor runs
**Then** *arr is told to unmonitor via seedbox Control
**And** the CLI never opens *arr HTTP

**Given** `why TITLE` / `status`
**When** the title is in grab, import, hold, pull, encode, library, watermark, or lock
**Then** that chain is shown with local FS as truth
**And** disk-full surfaces seedbox `df` plus a reclaim preview (preview may land in the next story if this one only wires `df`)

### Story 7.2: Reclaim preview and apply

As an operator,
I want ranked reclaim with a qBit seeding guard,
So that I can free remote buffer without deleting seeding or private-under-goal torrents.

**Acceptance Criteria:**

**Given** `reclaim preview --json`
**When** it runs
**Then** results are ranked by ratio, private, and age
**And** a dry-run exists; without dry-run, reclaim must not exist (no leftover no-op timer)

**Given** `reclaim apply`
**When** a remote library unlink is about to happen
**Then** qBit is queried inside the seedbox `DeleteRemote` handler; seeding returns typed `SkippedSeeding`
**And** private-under-goal is untouched; usenet-complete is deletable after Copy; torrent delete belongs to reclaim only, never sync-after-copy
**And** local proof is only `install_b3` — no digest means no delete; size/mtime is not proof

**Given** Copy of a torrent that is a library hardlink
**When** sync finishes the copy
**Then** the torrent is left; delete is not implied

## Epic 8: Move the library, render the docs

Machine mobility and generated operator docs.

### Story 8.1: Library relocate and new-machine

As an operator,
I want `library relocate` and `new-machine` export/import,
So that moving disks or machines does not require hand-edited paths.

**Acceptance Criteria:**

**Given** `library relocate`
**When** schema roots change
**Then** systemd units and title-index paths rewrite through config/store owners, never by hand

**Given** `new-machine` export
**When** I import on a new home
**Then** desired-state + tls/ + title-index populate the active config dir and machine state — never a git work tree
**And** layout bootstraps even before media files exist; `state.db` loss without export refuses reclaim until `library reindex` re-hashes

### Story 8.2: docs render from PathSchema

As an operator,
I want `docs render` to generate operator docs from PathSchema,
So that SEEDBOX.md/AGENTS.md cannot lie about scan paths.

**Acceptance Criteria:**

**Given** PathSchema rules
**When** I run `mediaops docs render --json`
**Then** generated docs are produced from PathSchema, not hand-edited path lists
**And** EncodePolicy packs remain the encoder inputs so docs and code cannot drift on scan rules
**And** a test cites the "docs vs code" named failure
