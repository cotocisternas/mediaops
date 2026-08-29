---
title: '1.1 Workspace scaffold and dependency law'
type: 'feature'
created: '2026-08-29'
status: 'done'
baseline_revision: 'f0f2339f86c17c14d9355bf22e2bbeeacacf8ba9'
review_loop_iteration: 0
followup_review_recommended: true
context:
  - '_bmad-output/implementation-artifacts/epic-1-context.md'
warnings: [oversized]
deferred: []
---

<intent-contract>

## Intent

**Problem:** The repo is a planning shell with no Cargo workspace, so later stories have nowhere legal to land types and CI cannot fail illegal crate edges.

**Approach:** Lay down the spine Structural Seed as a virtual workspace, pin the stack, put ExitCode / `--json` envelope / tracing in `core` + the two composition-root binaries, and enforce AD-2 in `mediaops-arch-tests`.

## Boundaries & Constraints

**Always:**
- Virtual workspace at repo root matching the Structural Seed paths: `Cargo.toml`, `proto/`, `crates/{core,proto,store,net,ssh,arr,transfer,sync,encode,arch-tests}`, `bins/{mediaops,mediaopsd}`, `fixtures/`.
- Package names `mediaops-<module>`; binary package names `mediaops` and `mediaopsd`. Edition 2024, `rust-version` 1.98.0, `rust-toolchain.toml` channel `1.98.0`.
- Pin these exact versions in `[workspace.dependencies]` (even if a crate does not use them yet): tonic / tonic-prost / tonic-prost-build 0.14.6, prost 0.14.4, rustls 0.23.43, tokio-rustls 0.26.4, rcgen 0.14.10, blake3 1.8.7, clap 4.6.6, rusqlite 0.40.2, tokio 1.53.1, reqwest 0.13.4, serde 1.0.229, toml 1.1.4, tracing 0.1.44, tracing-subscriber 0.3.23, thiserror 2.0.20, anyhow 1.0.104, similar 3.2.0, serde_json 1.0.151, cargo_metadata 0.23.1, directories 6.0.0, fs4 1.1.0.
- `thiserror` in library crates, `anyhow` only in binaries. Libraries never call `process::exit`. Each binary maps error → `ExitCode` in one function.
- `core` is pure domain (no I/O, no tokio runtime). Tracing subscriber init lives only in the binaries.
- AD-2 checker uses **Cargo** edges (depender depends on dependee). The spine mermaid arrows are provider → consumer; invert them. Allowed workspace Cargo edges: proto, store, net, ssh, arr, transfer, sync, encode, mediaops, mediaopsd → core; net, transfer, mediaops, mediaopsd → proto; transfer, mediaopsd → net; mediaopsd → arr; sync, mediaops → transfer; mediaops → store, ssh, sync, encode. Empty extra edges are fine (subgraph). Fail if `reqwest` is a direct dep outside `mediaops-arr`; `rusqlite` outside `mediaops-store`; `mediaops-encode` or `mediaops-store` in `mediaopsd`'s workspace-internal transitive closure; or direct deps named `rsync`, `rclone`, `ftp`, `ssh2`, `russh`, `ffmpeg-next`, `native-tls` on any member.
- Default tests: no network, live box, or GPU.
- Extend `.gitignore`; keep existing BMAD entries.

**Block If:** rustup cannot install `1.98.0`, or a pinned crate version is unpublished on crates.io.

**Never:** PathSchema/TitleId/walker, DesiredState/Plan/jobs/store schema, tonic proto codegen, TLS/certs, sqlite usage, reqwest in any crate, flock/config snapshot, LLM crates, serving gRPC, live-box/GPU work, pre-wiring adapter edges this story does not need, replacing `.gitignore` wholesale, editing `_bmad/` or planning-artifact prose.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| JSON happy | `mediaops --json` and `mediaopsd --json` | stdout = one `{ok, data, error}` envelope (`ok: true`, identity in `data`, `error: null`); stderr = tracing (JSON lines iff stderr is not a tty); process exit 0 | No error expected |
| Human happy | `mediaops` / `mediaopsd` with no `--json` | stdout = one human identity line (not JSON); tracing on stderr; exit 0 | No error expected |
| Usage | unknown flag, with or without `--json` | exit 2 (`ExitCode::Usage`); if `--json` present anywhere in argv, stdout is one envelope `ok: false` with `error.code` matching usage | clap/usage must not print the result envelope to stderr |
| AD-2 clean | current workspace graph | `cargo test -p mediaops-arch-tests` passes | No error expected |
| AD-2 violation (fixture) | synthetic metadata with `reqwest` on `mediaops-core`, or `mediaopsd` → `mediaops-store` | checker returns a non-empty violation list citing the illegal edge/crate | test asserts failure; do not commit an illegal workspace edge |

</intent-contract>

## Code Map

Greenfield: no `Cargo.toml` or `.rs` files exist. Do not overwrite `.gitignore` BMAD lines, `_bmad/`, `.agents/`, or `_bmad-output/` planning docs.

Create:
- `rust-toolchain.toml` -- pin `1.98.0`
- `Cargo.toml` -- virtual workspace + `[workspace.dependencies]` pins
- `proto/.gitkeep`, `fixtures/.gitkeep` -- seed dirs (no `.proto` bodies this story)
- `crates/core/src/lib.rs` -- `ExitCode`, JSON envelope types, `CapabilityToken` {ReadFs, ProbeMedia, ArrGet, ArrPost, SshExecAllowlist}
- `crates/core/Cargo.toml` -- `mediaops-core`; serde, serde_json, thiserror only
- `crates/{proto,store,net,ssh,arr,transfer,sync,encode}/` -- empty `lib.rs` skeletons, no extra workspace deps except `mediaops-proto` → `mediaops-core`
- `crates/arch-tests/` -- `mediaops-arch-tests`; cargo_metadata 0.23.1; AD-2 tests
- `bins/mediaops/src/main.rs`, `bins/mediaopsd/src/main.rs` -- clap `--json`, tracing init, call `core`, render, `exit_code` mapper; tokio + anyhow + clap + tracing-subscriber
- `.github/workflows/ci.yml` -- `cargo test -p mediaops-arch-tests` and `cargo test -p mediaops-core` (offline)
- `.gitignore` -- append `/target`, Cargo lock is **committed** (bins present)

Read-only evidence: `_bmad-output/planning-artifacts/architecture/architecture-mediaops-2026-08-29/ARCHITECTURE-SPINE.md` Structural Seed + Stack + AD-1/2/17/18/19/20; epic-1-context.md.

## Tasks & Acceptance

**Execution:**
- `rust-toolchain.toml` -- channel `1.98.0` -- spine toolchain
- `.gitignore` -- append `/target` (and typical `*.pem` / `tls/` secrets) without removing BMAD entries -- keep render cache ignored and build artifacts out of git
- `Cargo.toml` -- virtual workspace, resolver 3, members listed in Code Map, workspace package edition 2024 / rust-version 1.98.0 / version 0.1.0, all spine pins under `[workspace.dependencies]` -- single pin source
- `proto/.gitkeep`, `fixtures/.gitkeep` -- dirs exist in a greenfield checkout
- `crates/core/Cargo.toml` + `crates/core/src/lib.rs` -- `ExitCode` (Ok=0, Runtime=1, Usage=2, LockConflict=3, DriftVerify=4, PolicyRefusal=5) with `Termination`/`From<ExitCode> for i32`; envelope `{ok, data, error:{code, message}}` serde types + helpers; `CapabilityToken` enum; exhaustive matches; unit tests for discriminants and envelope round-trip -- AD-17/18/FR21
- `crates/proto/Cargo.toml` -- `mediaops-proto` depends on `mediaops-core` only; empty `src/lib.rs` -- AD-3 edge without codegen
- `crates/store/Cargo.toml`, `crates/net/Cargo.toml`, `crates/ssh/Cargo.toml`, `crates/arr/Cargo.toml`, `crates/transfer/Cargo.toml`, `crates/sync/Cargo.toml`, `crates/encode/Cargo.toml` -- `mediaops-*` empty libs, **no** rusqlite/reqwest/tokio/tonic yet -- skeletons only
- `crates/arch-tests/Cargo.toml` + `crates/arch-tests/src/lib.rs` -- `violations(metadata) -> Vec<Violation>` plus live `MetadataCommand` test from workspace root and fixture graphs for illegal I/O-matrix rows -- AD-2
- `bins/mediaops/Cargo.toml` + `bins/mediaops/src/main.rs` -- `mediaops-core`, clap, tokio (macros, rt-multi-thread), anyhow, tracing, tracing-subscriber, serde_json; `#[tokio::main]`; `--json`; tracing JSON-lines when stderr is not a tty; identity via core; one `to_exit_code` -- AD-1/18/19
- `bins/mediaopsd/Cargo.toml` + `bins/mediaopsd/src/main.rs` -- same pattern; no `mediaops-store` or `mediaops-encode` -- AD-1/2
- `bins/mediaops/tests/cli.rs`, `bins/mediaopsd/tests/cli.rs` -- `Command::new(env!("CARGO_BIN_EXE_*"))` for JSON happy, human happy, and usage rows
- `.github/workflows/ci.yml` -- push/PR: install rust-toolchain.toml, `cargo fetch`, then `cargo test --offline` for `mediaops-core`, `mediaops-arch-tests`, `mediaops`, `mediaopsd`

**Acceptance Criteria:**
- Given a checkout of this story, when the tree is inspected, then it matches the Structural Seed paths and package names, `rust-toolchain.toml` is 1.98.0, edition is 2024, and `[workspace.dependencies]` has every pinned version above.
- Given `crates/arch-tests`, when `cargo test -p mediaops-arch-tests` runs, then the live workspace is an AD-2 subgraph and fixture cases fail on reqwest-outside-arr, rusqlite-outside-store, encode/store in the mediaopsd closure, and banned crate names.
- Given `mediaops` and `mediaopsd`, when each is started with `--json`, without `--json`, and with an unknown flag, then stdout/stderr/exit match the I/O matrix (no listen, no lock, no extra stdout).
- Given `cargo test` with default features, when the suite runs, then nothing requires network, a live box, or a GPU, and `mediaops-core` has `CapabilityToken` with no LLM crate dependency.

### Review Findings

- [x] [Review][Patch] Parse `--json=` values to match clap; `--json=false` stays human on every path [`bins/mediaops/src/main.rs:37`]
- [x] [Review][Patch] `reqwest` pin keeps default features, so the first `mediaops-arr` consumer can pull `native-tls` [`Cargo.toml:35`]
- [x] [Review][Patch] Transitive encode-in-mediaopsd closure test does not require `mediaops-encode` in the message [`crates/arch-tests/src/lib.rs:286`]
- [x] [Review][Patch] Human usage and `--json=true` usage tests omit the stderr envelope assertion [`bins/mediaops/tests/cli.rs:64`]
- [x] [Review][Patch] `--help`/`--version` tests only assert exit 0, not clap text vs identity/JSON [`bins/mediaops/tests/cli.rs:106`]
- [x] [Review][Patch] CLI ignores stdout flush errors and clap `err.print()` failures [`bins/mediaops/src/main.rs:63`]
- [x] [Review][Patch] `AppError::Runtime` → exit 1 is never exercised by binary tests [`bins/mediaops/src/main.rs:53`]

## Spec Change Log

## Review Triage Log

### 2026-08-29 — Review pass
- intent_gap: 0
- bad_spec: 0
- patch: 10: (high 0, medium 5, low 5)
- defer: 0
- reject: 16
- addressed_findings:
  - `[medium]` `[patch]` CI compiled only four packages without `--locked`; now `cargo fetch --locked` and `cargo test --workspace --locked --offline` in [ci.yml](../../.github/workflows/ci.yml)
  - `[medium]` `[patch]` `--help`/`--version` exited 2 via `try_parse`; both binaries print clap help/version and exit 0
  - `[medium]` `[patch]` piped `human_happy` only checked non-empty stderr; it now requires JSON tracing lines
  - `[medium]` `[patch]` envelope tests treated a missing `error`/`data` key as null; they now require a present null
  - `[medium]` `[patch]` AD-2 closure tests could pass on illegal-edge text; they now require the closure message and a BFS intermediate-crate walk
  - `[low]` `[patch]` `json_requested` only matched exact `--json`; `--json=` forms are recognized (and `args_os` avoids UTF-8 abort)
  - `[low]` `[patch]` usage with `--json` later in argv did not assert the envelope stays off stderr
  - `[low]` `[patch]` live `MetadataCommand` now passes `--offline`
  - `[low]` `[patch]` live AD-2 test now asserts every Structural Seed package is a workspace member
  - `[low]` `[patch]` ignored `emit_error` results and silent human runtime errors; failures go through `to_exit_code` and tracing on stderr

## Design Notes

Spine mermaid `core --> proto` means proto **consumes** core, not that `mediaops-core` depends on `mediaops-proto`. `store --> cli` means the CLI may depend on store. Invert before coding the allowlist.

Skeleton `main` identity `data`: `{ "name": "<bin>", "version": "<pkg version>" }`. Human line: `mediaops <version>` / `mediaopsd <version>`.

Do not enable `tonic-prost-build` until story 1.4; `proto/` stays empty.

## Verification

**Commands:**
- `rustc --version` -- contains `1.98.0`
- `cargo test -p mediaops-core` -- pass, offline
- `cargo test -p mediaops-arch-tests` -- pass, offline
- `cargo run -p mediaops -- --json` -- stdout JSON `ok: true`; exit 0
- `cargo run -p mediaopsd -- --json` -- same
- `cargo run -p mediaops -- --nope-not-a-flag; echo $?` -- exit 2
- `cargo metadata --format-version 1 --no-deps` -- members match Code Map; no reqwest/rusqlite on the wrong packages

## Auto Run Result

Status: done

**Summary:** Greenfield Cargo workspace matching the Structural Seed: pinned Rust 1.98.0 / edition 2024, `ExitCode` + JSON envelope + `CapabilityToken` in `mediaops-core`, identity composition-root binaries with `--json` and stderr tracing, and `mediaops-arch-tests` enforcing AD-2.

**Files changed:**
- [rust-toolchain.toml](../../rust-toolchain.toml) — pin 1.98.0
- [Cargo.toml](../../Cargo.toml) — virtual workspace and spine pins
- [Cargo.lock](../../Cargo.lock) — committed lockfile
- [.gitignore](../../.gitignore) — `/target`, `*.pem`, `tls/`
- [proto/.gitkeep](../../proto/.gitkeep), [fixtures/.gitkeep](../../fixtures/.gitkeep) — seed directories
- [crates/core](../../crates/core/src/lib.rs) — ExitCode, envelope, CapabilityToken
- [crates/proto](../../crates/proto) plus empty adapter crates — skeletons; proto depends on core
- [crates/arch-tests](../../crates/arch-tests/src/lib.rs) — AD-2 `violations` + fixtures
- [bins/mediaops](../../bins/mediaops/src/main.rs), [bins/mediaopsd](../../bins/mediaopsd/src/main.rs) — composition roots
- [.github/workflows/ci.yml](../../.github/workflows/ci.yml) — locked offline workspace tests

**Review findings:** 10 patches applied (0 high, 5 medium, 5 low). 0 deferred. 16 rejected (CI cosmetics, fmt/clippy, crate extraction, pty tty tests, later-story types, unpinned extras).

**Follow-up review recommendation:** true. Patched this pass: high 0, medium 5, low 5. Score `3 × 5 + 1 × 5 = 20` (≥ 5).

**Verification:** `rustc --version` → 1.98.0. `cargo test --workspace --offline --locked` passed (core 5, arch-tests 8, each CLI 8 including matrix rows). `cargo run -p mediaops -- --json` and `mediaopsd -- --json` → stdout `ok: true`, exit 0. Unknown flag exit 2. `--help` exit 0. `cargo metadata --no-deps --offline --locked` ok.

**Residual risks:** unused workspace pins are not in `Cargo.lock` until a member depends on them. `--help --json` prints clap help (exit 0) rather than a JSON envelope. TTY (human) tracing is implemented but not pty-tested. Duplicate CLI `main`/tests across the two bins.

