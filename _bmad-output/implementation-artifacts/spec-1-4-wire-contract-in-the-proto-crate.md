---
title: '1.4 Wire contract in the proto crate'
type: 'feature'
created: '2026-08-30'
status: 'done'
baseline_revision: 'bcc2e48c5f0695facd3a9a11c81d4b85bd9f2767'
review_loop_iteration: 1
followup_review_recommended: true
context:
  - '_bmad-output/implementation-artifacts/epic-1-context.md'
warnings: []
deferred: []
---

<intent-contract>

## Intent

**Problem:** `mediaops-proto` is an empty crate and `proto/` has no sources, so daemon and CLI have no shared `mediaops.v1` contract and conversions would otherwise be invented per consumer.

**Approach:** Generate one `mediaops.v1` Control + Transfer contract with `tonic-prost-build`, put every wire↔domain `From`/`TryFrom` and the only `tonic::Status` build/parse pair in `proto`, and land `core::ControlPort` with its canonical client impl in `proto`. Types and conversions only.

## Boundaries & Constraints

**Always:**
- Package `mediaops.v1`. Codegen is `tonic-prost-build` 0.14.6 (runtime `tonic` + `tonic-prost` 0.14.6, `prost` 0.14.4). No hand-written RPC types. Sources live in repo-root `proto/` (Structural Seed); `crates/proto` owns conversions.
- `proto` is the sole home of wire↔domain `From`/`TryFrom`. `RemoteRef`/`RemoteEntry` on the wire mirror core field-for-field (`root_id`+`rel_path`; `ref`+`len`+`mtime`+`nlink`). Wire→domain `RemoteRef` calls `RemoteRef::from_wire_parts`; `RemoteEntry` rebuilds via that ref then `RemoteEntry::from_wire_parts`. `rel_path` is a UTF-8 string; refuse lossy conversion.
- `ErrorDetail { exit_code: int32, reason: string, message: string }` is proto-owned. The only two Status functions pack/parse a serialized `ErrorDetail` in `Status::details` (`status_from_error_detail`, `error_detail_from_status`). `exit_code` is `ExitCode` as i32 (0–5); `reason` is `ExitCode::error_code()`. Unknown `exit_code` or missing details fail parse. Status code is `Unknown`; taxonomy lives in the detail. Nothing outside `proto` constructs `tonic::Status`.
- `core` defines `ControlPort` (async fn in trait, no tokio dep): `df`, `unmonitor`, `delete_remote`, `grab_apply`, `edge_check`, `key_discovery`, `guard_preview`. `proto` implements it over the generated `ControlClient`. `sync` will consume the trait later; do not inject it into binaries this story.
- Naming: services `Control`, `Transfer`; RPCs UpperCamelCase; messages `<Rpc>Request`/`<Rpc>Response`. Transfer: `List`, `Stat`, `GetRange` (server-streaming). Control: `Df`, `Unmonitor`, `DeleteRemote`, `GrabApply`, `EdgeCheck`, `KeyDiscovery`, `GuardPreview`. Every Control response has `string semver` and `string proto_package` (`mediaops.v1`). Evolution inside `mediaops.v1` is additive-only.
- `thiserror` in libraries. Default tests offline (no live box, GPU, or WAN). `core` stays free of tonic/tokio. Keep 1.1–1.3 public API.

**Block If:** rustc is not 1.98.0, a pinned crate is unpublished, or `protoc` is missing and cannot be installed.

**Never:** Live serve, TLS mint, Range probe, `ConfigurePool`/`PoolStatus`, HoldKey/hold messages, a Plan/Action/DesiredState proto, TitleId as a protobuf message, linking `proto` into bins/`net`/`transfer`, jobs/`store`, rewriting walker/`from_wire_parts`, applying Control/Transfer, constructing `Status` outside `proto`, `native-tls`.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| RemoteRef happy | wire `{root_id:"seedbox", rel_path:"a/b.bin"}` | `core::RemoteRef` via `from_wire_parts` | No error expected |
| Empty root | `root_id=""` | No domain ref | `EmptyRootId` |
| Escape path | `rel_path` `/etc/passwd`, `../..`, or `""` | No domain ref | `UnknownPath` |
| Non-UTF8 path | core `rel_path` that is not UTF-8 | No wire ref | Error; never `to_string_lossy` |
| RemoteEntry mtime | `mtime=-1`, `len=5`, `nlink=2` | Round-trip preserves all three | No error expected |
| ErrorDetail round-trip | `ExitCode::PolicyRefusal` + message | `Status.details` round-trips `exit_code=5`, `reason=policy_refusal` | No error expected |
| Status no details | `Status` with empty details | Parse fails | Do not invent an ErrorDetail |
| Bad exit_code | `exit_code=99` in details | Parse fails | Do not map to a default ExitCode |
| Df size | `DfResponse.free_bytes` uint64 | domain `Bytes` (no live client) | No bare size integer leaves proto |

</intent-contract>

## Code Map

Reuse (do not rewrite): `RemoteRef::from_wire_parts` / `RemoteEntry::from_wire_parts` (`crates/core/src/walker.rs:34`, `:77`; test `:802`); `ExitCode` + `error_code()` (`crates/core/src/lib.rs:37`); `TitleId::render`/`parse` (`title_id.rs`); `Bytes` (`bytes.rs`); `CORE_IO_MODULES` (`arch-tests/src/lib.rs:146`) — `control_port.rs` stays off that list; AD-2 edges (`arch-tests/src/lib.rs:9-23`) already allow proto→core and bins/net/transfer→proto — do not add those consumer edges yet; workspace pins (`Cargo.toml:24-27`); `crates/proto` skeleton; `proto/.gitkeep`.

Add:
- `proto/mediaops.proto` -- package `mediaops.v1`; messages `RemoteRef`, `RemoteEntry`, `ErrorDetail`; Transfer `List`/`Stat`/`GetRange`; Control RPCs above. `ListRequest` empty; `ListResponse` repeated `RemoteEntry`; `Stat` takes `RemoteRef` returns `RemoteEntry`; `GetRangeRequest` `{ref, offset, len}` uint64 byte counts; `GetRangeResponse` `{bytes data}`; `UnmonitorRequest.title_id` is `kind:source:id`; `DeleteRemoteRequest` is a `RemoteRef`; `DeleteRemoteResult` is `DELETE_REMOTE_RESULT_UNSPECIFIED = 0`, `DELETED = 1`, `SKIPPED_SEEDING = 2` (never give 0 a success meaning); nested `RemoteRef`/`RemoteEntry` fields are required at conversion (absence is an error). `DfResponse.free_bytes` uint64; other Control requests empty this story. Replace `.gitkeep`.
- `crates/proto/build.rs` -- `tonic_prost_build::configure().compile_protos` with include dir repo-root `proto/` (from `CARGO_MANIFEST_DIR`). `cargo:rerun-if-changed` that dir.
- `crates/proto/Cargo.toml` -- runtime `mediaops-core`, `prost`, `tonic`, `tonic-prost`, `thiserror`; build-dep `tonic-prost-build`. No rusqlite/reqwest/native-tls.
- `crates/proto/src/lib.rs` -- `tonic::include_proto!("mediaops.v1")`; `From`/`TryFrom` for `RemoteRef`/`RemoteEntry`; `From<ControlError> for ErrorDetail`; `status_from_error_detail` / `error_detail_from_status`; `ControlPort` impl wrapping generated `ControlClient`. No hand-written RPC structs.
- `crates/core/src/control_port.rs` -- `ControlPort`, `ControlError {exit_code, message}`, `DeleteRemoteOutcome {Deleted, SkippedSeeding}` exhaustive. `df(&self) -> Bytes`; `unmonitor(&self, &TitleId)`; `delete_remote(&self, &RemoteRef) -> DeleteRemoteOutcome`; other methods `()`.
- `crates/core/src/lib.rs` -- `mod control_port` + re-exports; crate docs: still no tokio/tonic; ControlPort is a port, not I/O.
- `crates/arch-tests/src/lib.rs` -- fail if `tonic::Status` is constructed outside `crates/proto` (ADV-8).
- `.github/workflows/ci.yml` -- install `protobuf-compiler` before `cargo test`.

Read-only evidence: epic-1-context Wire paragraph; AD-3, AD-4 (no Plan RPC; SkippedSeeding; Status details byte-for-byte later), AD-13 (listing/`Stat`/`GetRange` shapes), AD-17, AD-22 (semver + proto package on Control responses); ADV-8 Construction A; spec-1-2 D1 wire door.

## Tasks & Acceptance

**Execution:**
- `proto/mediaops.proto` -- v1 services + field-for-field RemoteRef/RemoteEntry + ErrorDetail -- AD-3/13
- `crates/proto/build.rs` + `Cargo.toml` -- tonic-prost-build 0.14.6 against repo-root `proto/` -- AD-3
- `crates/core/src/control_port.rs` + `lib.rs` -- ControlPort + outcomes; unit-test exhaustive DeleteRemoteOutcome -- AD-3
- `crates/proto/src/lib.rs` -- include_proto, conversions, two Status fns, ControlPort client impl; unit-test every I/O-matrix row; `TryFrom` rejects `DeleteRemoteResult` 0/unknown and missing nested refs -- AD-3/17
- `crates/arch-tests/src/lib.rs` -- Status construction only in proto -- ADV-8
- `.github/workflows/ci.yml` -- protoc on the runner -- codegen

**Acceptance Criteria:**
- Given `.proto` sources under package `mediaops.v1`, when `mediaops-proto` builds, then codegen is `tonic-prost-build` (tonic 0.14.6 / prost 0.14.4) and proto is the only crate with wire↔domain `From`/`TryFrom`.
- Given Control and Transfer messages, when they are inspected, then `RemoteRef`/`RemoteEntry` match core field-for-field, and `ErrorDetail` plus the only two `tonic::Status` build/parse functions live in proto.
- Given `core::ControlPort`, when the canonical impl is used, then it lives in proto over generated clients, and adding a field inside `mediaops.v1` remains the evolution rule (no new package).
- Given default-feature tests, when they run, then they pass offline, binaries still match the 1.1 CLI matrix, core has no tonic/tokio, and neither binary links proto yet.

## Spec Change Log

- 2026-08-30 — Review pass 1 (bad_spec): `DeleteRemoteResult DELETED = 0` made an omitted proto3 field mean successful unlink. Code Map and Design Notes now require `UNSPECIFIED = 0`, `DELETED = 1`, `SKIPPED_SEEDING = 2`, and `TryFrom` that rejects 0/unknown; nested `RemoteRef`/`RemoteEntry` fields are required at conversion. Avoids freezing a default-delete inside additive-only `mediaops.v1`. KEEP: repo-root `proto/mediaops.proto` + `tonic-prost-build` from `crates/proto`; field-for-field `RemoteRef`/`RemoteEntry` via `from_wire_parts`; `ErrorDetail` in `Status::details` with `Code::Unknown`; I/O-matrix tests in `mediaops-proto`; `ControlPort` in core / `ControlPortClient` in proto; no bin link to proto; ADV-8 Status scan; CI `protobuf-compiler`; `DfResponse.free_bytes` → `Bytes`; `TitleId::render` on Unmonitor; `DeleteRemoteOutcome {Deleted, SkippedSeeding}` in core.

## Review Triage Log

### 2026-08-30 — Review pass
- intent_gap: 0
- bad_spec: 1: (high 1)
- patch: 8: (high 0, medium 6, low 2)
- defer: 0
- reject: 12: (high 0, medium 0, low 12)
- addressed_findings:
  - `[high]` `[bad_spec]` `DeleteRemoteResult DELETED=0` is the proto3 default so an omitted result becomes Deleted. Spec now requires unspecified=0 and conversion reject; code reverted for re-derivation.

### 2026-08-30 — Review pass
- intent_gap: 0
- bad_spec: 0
- patch: 7: (high 0, medium 5, low 2)
- defer: 0
- reject: 14: (high 0, medium 0, low 14)
- addressed_findings:
  - `[medium]` `[patch]` `ControlPort: Send + Sync` so later `sync` can spawn the port
  - `[medium]` `[patch]` Round-trip every `ExitCode` through `ErrorDetail`/`Status`; assert `status.message()`
  - `[low]` `[patch]` `InvalidErrorDetail` on garbage `Status.details`
  - `[medium]` `[patch]` `DeleteRemoteResponse` wire `result` 1/2 → `Deleted`/`SkippedSeeding`
  - `[medium]` `[patch]` ADV-8 scan: more constructors, IO failures are violations, skip only `crates/proto`
  - `[low]` `[patch]` `#[cfg(unix)]` on the non-UTF8 `rel_path` test
  - `[medium]` `[patch]` `tonic` default-features off; proto enables `codegen` + `transport` only

## Design Notes

RPC names are the UpperCamelCase of AD-3/AD-13 operations (`listing` → `List`; `guard preview` → `GuardPreview`). Empty Control request bodies are the additive baseline; Epic 5–7 add fields, they do not retype them.

`Status::with_details(Code::Unknown, message, encode(ErrorDetail))` is ADV-8 Construction A. Do not put ExitCode in metadata. Story 3.1 `ResourceExhausted` must extend these two functions, not add a third constructor.

`GetRange` is a range RPC: `offset`+`len` are uint64 byte counts (same unit as `RemoteEntry.len`, not `Bytes` — 1.2 already froze that metadata as `u64`). `df` is the size that crosses a crate boundary, so it becomes `Bytes`.

`proto_package` on every Control response is the string `mediaops.v1`. There is no Handshake RPC.

Proto3 omitted enums decode as 0. `DeleteRemoteResult` therefore reserves 0 as unspecified; a missing or zero `result` is a conversion error, never `Deleted`. Same law for nested `RemoteRef`/`RemoteEntry` fields: absence is `ConvertError`, not a default ref.

## Verification

**Commands:**
- `protoc --version` -- present (CI installs `protobuf-compiler`)
- `cargo test -p mediaops-proto --offline --locked` -- pass, including every I/O-matrix row
- `cargo test -p mediaops-core --offline --locked` -- pass; ControlPort exists; 1.2/1.3 tests unchanged
- `cargo test -p mediaops-arch-tests --offline --locked` -- AD-2 + core-IO + Status-ownership pass
- `cargo test -p mediaops --offline --locked` and `cargo test -p mediaopsd --offline --locked` -- 1.1 CLI matrix still pass
- `cargo clippy --workspace --all-targets --offline --locked` -- clean

## Auto Run Result

Status: done

**Summary:** Generated `mediaops.v1` Control + Transfer contract in `proto/mediaops.proto` via `tonic-prost-build`. Wire↔domain conversions and the only two `tonic::Status` pack/parse functions live in `mediaops-proto`. `core::ControlPort` is implemented over the generated `ControlClient`. Types and conversions only; binaries still do not link proto.

**Files changed:**
- [proto/mediaops.proto](../../proto/mediaops.proto) — `mediaops.v1` services, field-for-field refs, `ErrorDetail`, `DeleteRemoteResult` unspecified=0
- [crates/proto/build.rs](../../crates/proto/build.rs) — `tonic-prost-build` against repo-root `proto/`
- [crates/proto/Cargo.toml](../../crates/proto/Cargo.toml) — tonic/prost/thiserror; tonic `codegen`+`transport`
- [crates/proto/src/lib.rs](../../crates/proto/src/lib.rs) — include_proto, conversions, Status helpers, `ControlPortClient`
- [crates/core/src/control_port.rs](../../crates/core/src/control_port.rs) — `ControlPort: Send + Sync`, `ControlError`, `DeleteRemoteOutcome`
- [crates/core/src/lib.rs](../../crates/core/src/lib.rs) — re-exports; still no tokio/tonic
- [crates/arch-tests/src/lib.rs](../../crates/arch-tests/src/lib.rs) — ADV-8 Status construction only in proto
- [.github/workflows/ci.yml](../../.github/workflows/ci.yml) — install `protobuf-compiler`
- [Cargo.toml](../../Cargo.toml) — tonic `default-features = false`
- [Cargo.lock](../../Cargo.lock) — tonic/prost lock

**Review findings:** Pass 1: 1 high bad_spec (DeleteRemote 0-default) — spec amended, code re-derived. Pass 2: 7 patches applied (0 high, 5 medium, 2 low). 0 deferred. 14 rejected (live ControlPort RPCs, AD-22 package refuse, server-side reverse conversions, Transfer port, RPC deadlines, CI clippy pin, proto comments).

**Follow-up review recommendation:** true. Patched this pass: high 0, medium 5, low 2. Score `3 × 5 + 1 × 2 = 17` (≥ 5).

**Verification:** `protoc --version` → libprotoc 36.0. `cargo test -p mediaops-proto --offline --locked` → 13 passed (I/O matrix plus unspecified result, missing refs, every ExitCode, InvalidErrorDetail). `cargo test -p mediaops-core --offline --locked` → 82 passed. `cargo test -p mediaops-arch-tests --offline --locked` → 13 passed. CLI matrices for `mediaops` and `mediaopsd` unchanged. `cargo clippy --workspace --all-targets --offline --locked` → clean.

**Residual risks:** `ControlPortClient` is type-checked, not RPC-exercised (no live client this story). Unparseable wire Status still maps to `ControlError` Runtime on the client path; parse helpers themselves fail closed. `reason` is not re-checked against `exit_code` on decode. ADV-8 scan remains a substring walk (aliases/macros can hide constructors). `GetRange` stream is generated bytes only; Epic 3 binds it.
