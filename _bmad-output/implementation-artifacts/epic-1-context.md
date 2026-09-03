# Epic 1 Context: The laws live in types

<!-- Generated from planning artifacts. Regenerate with compile-epic-context if planning docs change. -->

## Goal

The home repo compiles as the architecture structural seed: identity, library paths, desired-state, plans, jobs, and the gRPC contract exist as types with offline tests. After this epic a developer can compile the workspace, CI fails illegal crate edges, and later epics compose against these types. Nothing talks to the live box or a GPU.

## Stories

- Story 1.1: Workspace scaffold and dependency law
- Story 1.2: PathSchema, TitleId, and the walker
- Story 1.3: DesiredState, Plan, jobs, and store
- Story 1.4: Wire contract in the proto crate

## Requirements & Constraints

- Plan is a first-class artifact. `Action` is one exhaustive enum: Copy, Skip, Review, Unmonitor, DeleteRemote, Encode, Reclaim, EdgeApply, GrabApply. Applying those actions beyond types is later epics; the types must exist here.
- CAP-11 stays deferred: reserve a capability-token enum in `core` only. No LLM runtime dependency and no agent-approve path.
- Every CLI verb supports `--json`. Stdout is a human result or a single `{ok, data, error}` envelope; stderr is tracing (JSON lines when not a tty).
- Identity is TitleId (kind + TMDB / TVDB / MBID), never a path string. Music remasters key by MBID, not folder year.
- Default tests never require network, the live box, or a GPU. Live-box integration stays behind an explicit cargo feature plus env var, never default CI.
- This epic does not bootstrap a seedbox, pull files, encode, apply grabber/edge desired-state, or talk to *arr.

## Technical Decisions

**Workspace.** Virtual workspace manifest. Layout: `proto/` sources; crates `core`, `proto`, `store`, `net`, `ssh`, `arr`, `transfer`, `sync`, `encode`, `arch-tests`; bins `mediaops` and `mediaopsd`; `fixtures/`. Package names `mediaops-<module>`; binaries stay `mediaops` / `mediaopsd`. Toolchain: Rust 1.98, edition 2024. Pin: tonic / tonic-prost / tonic-prost-build 0.14.6, prost 0.14.4, rustls 0.23.43, tokio-rustls 0.26.4, rcgen 0.14.10, blake3 1.8.7, clap 4.6.6, rusqlite 0.40.2, tokio 1.53.1, reqwest 0.13.4 (arr only), serde 1.0.229, toml 1.1.4, tracing 0.1.44 / tracing-subscriber 0.3.23, thiserror 2.0.20, anyhow 1.0.104, similar 3.2.0, serde_json 1.0.151, cargo_metadata 0.23.1, directories 6.0.0, fs4 1.1.0.

**Layering.** Binaries are composition roots only: parse/serve, snapshot config, take lock, call libraries, render output. Logic a test would want lives in library crates. `core` is pure domain (no I/O). `arch-tests` walks `cargo_metadata` and fails unless the internal graph is a subgraph of: core → proto, store, net, ssh, arr, transfer, sync, encode, both bins; proto → net, transfer, both bins; net → daemon, transfer; arr → daemon; transfer → sync and CLI; store, ssh, transfer, sync, encode → CLI only. External bans: reqwest only under `arr`; rusqlite only under `store`; encode and store never in the mediaopsd tree; rsync, rclone, ftp, ssh2, russh, ffmpeg-next, native-tls nowhere.

**Identity and paths.** TitleId serializes as `kind:source:id` (e.g. `movie:tmdb:603`). `core::pathschema` is the only renderer/parser; `parse(render(id)) == id`. Year lives in the folder and the file the same way; spaces are refused. Strip scene tags REPACJ, REPACK, and PROPER. Explicit reject bins include `needs-split` and `needs-year`. Staging paths come only from `core::pathschema::staging_path` (`_incoming/<TitleId>/…`). One allowlist walker is the sole producer of typed `RemoteRef {root_id, rel_path}` and `RemoteEntry {ref, len, mtime, nlink}`; unknown paths error; never follow symlinks off the allowlist; torrent save paths and `torrents/incomplete` are not listed. The install gate has exactly two entry points: `install` and `replace` (`replace` is encode’s path and the only writer of `current_b3`). Callers other than tests may wait until later epics.

**Desired-state, Plan, jobs, store.** `DesiredState` uses `deny_unknown_fields` and requires `schema_version`. Size fields are unit-suffixed (`max_copy_gib`, `min_free_gib`, `range_len_mib`) and convert to `Bytes(u64)` at parse — no bare integer size crosses a crate boundary. A Plan embeds the exact raw TOML bytes of the snapshotted desired-state plus `blake3(bytes)`; apply later re-parses only from those bytes and refuses on hash mismatch. `Action` is matched with a `never` default. Story 1.3 shipped those types only. The 1.3 remainder (`core::jobs` + `title_index`/`jobs` sqlite, repository traits in `core`) landed after Epic 2: `probes` at `user_version = 1`, then `title_index` / `jobs` (with `title_id`) at `user_version = 3`. rusqlite stays behind `spawn_blocking`; the CLI links `store`; the seedbox daemon stays store-free. `holds_decisions` landed with Epic 6 (`user_version` 6; PRs #7/#8). tokio is the only executor; `thiserror` in libraries, `anyhow` only in binaries.

**Wire.** One generated contract under package `mediaops.v1` via `tonic-prost-build`. `proto` is the sole home of wire↔domain `From`/`TryFrom`. `core` defines the `ControlPort` trait; `proto` ships the canonical implementation over generated clients. `proto` owns `ErrorDetail {exit_code, reason, message}` and the only two functions that build/parse `tonic::Status`. `RemoteRef` / `RemoteEntry` on the wire mirror `core` field-for-field. Evolution inside `mediaops.v1` is additive-only. Naming: services `Control` and `Transfer`; RPCs UpperCamelCase; messages `<Rpc>Request` / `<Rpc>Response`.

**Exit codes.** `core` owns exhaustive `ExitCode`: 0 ok, 1 runtime, 2 usage, 3 lock conflict, 4 drift/verify, 5 policy refusal. Libraries never call `exit`; each binary maps error → ExitCode in one place. ExitCode reflects the command’s own contract; refusals inside an apply loop are data, not exit 5.

## UX & Interaction Patterns

CLI-first; no separate UX contract. TUI is deferred and will attach to the tracing stream, not a second API. Timers never require a TUI. Stdout is only the result; progress is tracing on stderr.

## Cross-Story Dependencies

- 1.1 is the substrate. Later stories in this epic add types into that workspace; they do not invent a second layout.
- 1.2’s TitleId, PathSchema, walker types, and install-gate signatures are consumed by later `title_index` persistence and by 1.4 (proto mirrors `RemoteRef` / `RemoteEntry`). 1.2 review is closed; eight deferrals stay on the ledger.
- 1.3 shipped DesiredState / Plan / Action (what Epic 4 applies). The 1.3 remainder (`core::jobs` + `title_index`/`jobs`) is implemented; remaining Action variants exist as types but are not applied until Epics 5–7.
- 1.4’s Control / Transfer contract is what Epic 2 bound. Epic 1 itself was types and conversions only.
- `probes` landed with Epic 2 (`user_version = 1`). `title_index` / `jobs` (with `title_id`) are `user_version = 3`. `holds_decisions` landed with Epic 6 (`user_version` 6; PRs #7/#8).
