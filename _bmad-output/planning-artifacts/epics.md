---
stepsCompleted: []
inputDocuments:
  - _bmad-output/specs/spec-mediaops/SPEC.md
  - _bmad-output/specs/spec-mediaops/module-map.md
  - _bmad-output/specs/spec-mediaops/grabber-inventory.md
  - _bmad-output/specs/spec-mediaops/bootstrap-surfaces.md
  - _bmad-output/specs/spec-mediaops/failure-history-tests.md
  - _bmad-output/specs/spec-mediaops/increments.md
  - _bmad-output/planning-artifacts/architecture/architecture-mediaops-2026-08-29/ARCHITECTURE-SPINE.md
---

# mediaops - Epic Breakdown

## Overview

This document provides the complete epic and story breakdown for mediaops, decomposing the requirements from SPEC-mediaops (the canonical contract standing in for a PRD), its companions, and the Architecture Spine into implementable stories.

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

{{requirements_coverage_map}}

## Epic List

{{epics_list}}
