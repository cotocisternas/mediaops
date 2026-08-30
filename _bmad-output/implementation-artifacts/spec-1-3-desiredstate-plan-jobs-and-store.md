---
title: '1.3 DesiredState, Plan, and Action'
type: 'feature'
created: '2026-08-29'
status: 'done'
baseline_commit: '1bb8fc204d9d318c93384a4a8a14d9574463fdd2'
review_loop_iteration: 1
context:
  - '_bmad-output/implementation-artifacts/epic-1-context.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** `mediaops-core` has identity and paths but no DesiredState or Plan, so later stories cannot snapshot config or carry an exhaustive Action list.

**Approach:** Add `Bytes`, DesiredState (TOML parse laws), Plan (raw bytes + blake3), and exhaustive `Action` in `core`. Jobs machines and sqlite wait (deferred-work).

## Boundaries & Constraints

**Always:**
- `DesiredState` is serde TOML with `deny_unknown_fields` and required `schema_version` (this story: `1` only). Size fields `max_copy_gib`, `min_free_gib`, `range_len_mib` convert to `Bytes(u64)` at parse (1 GiB = 2^30, 1 MiB = 2^20). No bare integer size crosses a crate boundary. Also parse ResourceBudget non-size fields `max_nvenc` (u32 count) and `lock` (bool).
- A Plan is a JSON artifact embedding the exact raw TOML bytes of the snapshotted desired-state plus `blake3(bytes)` (lowercase hex). Re-parse DesiredState only from those bytes. Every snapshot-hash comparison is bytes-hash vs bytes-hash — no canonical-serialization hashing.
- `Action` is one exhaustive enum: Copy, Skip, Review, Unmonitor, DeleteRemote, Encode, Reclaim, EdgeApply, GrabApply. Match with a `never` default (no `_` arm, not `#[non_exhaustive]`). Applying actions is later epics; the type must exist here.
- New `core` modules stay pure: no `std::fs`, no tokio, no rusqlite. `CORE_IO_MODULES` stays `walker.rs` / `install.rs`. `thiserror` in libraries. Default tests offline. Keep 1.1/1.2 public API. `store` crate stays an empty skeleton.

**Ask First:**
- A pinned crate version (`toml` 1.1.4, `blake3` 1.8.7) is unpublished.

**Never:** `core::jobs`, repository traits, rusqlite/`store` wiring, `title_index`, proto/tonic, CLI verbs, applying Plan actions, live box/GPU, `std::fs` in new core modules, rewriting TitleId/PathSchema/walker/install, linking `store` into any binary.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| DS happy | TOML with `schema_version = 1` and the ResourceBudget fields | `DesiredState`; sizes are `Bytes`, not raw integers | No error expected |
| Unknown field | Extra TOML key | Parse fails | deny_unknown_fields |
| Missing version | No `schema_version` | Parse fails | required field |
| Bad version | `schema_version = 2` | Parse fails | unsupported version |
| Plan round-trip | Raw TOML bytes → Plan → JSON → Plan | Embedded bytes identical; hex digest = blake3(bytes) | No error expected |
| Hash mismatch | Plan digest ≠ blake3(active bytes) | `matches_snapshot` is false | refuse; do not re-parse from disk |
| Exhaustive Action | Match on every variant | Compiles with no `_` arm | N/A |

</frozen-after-approval>

## Code Map

Reuse (do not rewrite): `crates/core/src/{lib,title_id,pathschema,walker,install}.rs` public API; `CORE_IO_MODULES` in `crates/arch-tests/src/lib.rs:140`; `crates/store` empty skeleton; `bins/mediaopsd/Cargo.toml` stays store-free.

Add under `crates/core/src/` and re-export from `lib.rs`:
- `bytes.rs` -- `Bytes(u64)` newtype; Display/serde as u64 bytes; no other crate sees a bare size integer
- `desired_state.rs` -- TOML parse → `DesiredState`; size fields become `Bytes`; `schema_version == 1`
- `plan.rs` -- `Action` enum; `Plan { desired_state_toml: Vec<u8>, desired_state_b3, actions }`; `from_toml_bytes`; `desired_state()` re-parses embedded bytes; `matches_snapshot(&[u8])` is blake3 equality; serde JSON

`crates/core/Cargo.toml` -- add workspace `toml` and `blake3` (hash TOML bytes; `rayon` is opt-in in blake3, so nothing has to be switched off). No tokio, rusqlite.

Read-only evidence: epic-1-context Desired-state/Plan paragraph; AD-9; Config convention (unit-suffixed sizes → `Bytes`); spec-1-2 public API must stay.

## Tasks & Acceptance

**Execution:**
- [x] `crates/core/src/bytes.rs` -- `Bytes` newtype -- boundary law
- [x] `crates/core/src/desired_state.rs` -- parse TOML; unit-test every DS matrix row -- AD-9
- [x] `crates/core/src/plan.rs` -- Action exhaustive; Plan snapshot/JSON/hash; unit-test plan rows -- AD-9
- [x] `crates/core/src/lib.rs` + `Cargo.toml` -- mods, re-exports, toml+blake3; crate docs: still no tokio/rusqlite; new modules are not IO -- keep 1.1/1.2 public

**Acceptance Criteria:**
- Given a desired-state TOML, when `core` parses it, then unknown fields and missing/`!=1` `schema_version` fail, and the three size fields exist only as `Bytes` on the struct.
- Given raw TOML bytes, when a Plan is written then read, then the embedded bytes are identical, the digest is blake3 of those bytes, and `desired_state()` re-parses only from the embedding.
- Given `Action`, when it is matched, then every variant is named and there is no catch-all arm.

## Design Notes

Plan JSON field `desired_state_toml` is a UTF-8 string (TOML is text); hash the string's bytes. `desired_state_b3` is 64 lowercase hex chars.

`max_nvenc` and `lock` are ResourceBudget fields from the SPEC; they are not sizes and stay `u32` / `bool`. Policies, pins, and cert fingerprints wait for later stories — `deny_unknown_fields` will reject them until those fields are added.

Jobs (`advance`/`ready`) and `store` (`title_index`/`jobs`, rusqlite) are in deferred-work, not this spec.

## Verification

**Commands:**
- `cargo test -p mediaops-core --offline --locked` -- pass, including DS/Plan matrix
- `cargo test -p mediaops-arch-tests --offline --locked` -- AD-2 + core-IO still pass
- `cargo test -p mediaops --offline --locked` and `cargo test -p mediaopsd --offline --locked` -- 1.1 CLI matrix still pass

## Suggested Review Order

**DesiredState parse**

- Version peek first so v2-with-new-fields is UnsupportedVersion, not unknown-field.
  [`desired_state.rs:54`](../../crates/core/src/desired_state.rs#L54)

- Zero `range_len_mib` / `max_nvenc` are representable but nonsensical; refuse at parse.
  [`desired_state.rs:63`](../../crates/core/src/desired_state.rs#L63)

- Size fields become `Bytes` at parse; no bare integer leaves this crate.
  [`desired_state.rs:75`](../../crates/core/src/desired_state.rs#L75)

**Plan snapshot**

- Plan stores exact TOML bytes and blake3 of those bytes, never a canonical form.
  [`plan.rs:53`](../../crates/core/src/plan.rs#L53)

- Staleness is bytes-hash vs bytes-hash; it does not re-parse the active file.
  [`plan.rs:87`](../../crates/core/src/plan.rs#L87)

- Plan JSON refuses extra keys the same way DesiredState refuses extra TOML.
  [`plan.rs:31`](../../crates/core/src/plan.rs#L31)

- Loading a plan checks the digest before anything reads the snapshot as schema,
  and does not re-parse it -- an archived plan outlives its `schema_version`.
  [`plan.rs:129`](../../crates/core/src/plan.rs#L129)

- `from_json_slice` gives callers the typed error; `Deserialize` can only flatten it.
  [`plan.rs:117`](../../crates/core/src/plan.rs#L117)

**Action**

- Exhaustive unit enum; match every variant, no `_` arm, not non_exhaustive.
  [`plan.rs:10`](../../crates/core/src/plan.rs#L10)

- Wire tokens are snake_case, like every other identifier in the plan contract.
  [`plan.rs:9`](../../crates/core/src/plan.rs#L9)

**Bytes boundary**

- Display and serde are raw byte counts, not GiB/MiB.
  [`bytes.rs:8`](../../crates/core/src/bytes.rs#L8)

**Peripherals**

- Public re-exports; new modules stay out of the filesystem carve-out.
  [`lib.rs:17`](../../crates/core/src/lib.rs#L17)

- Workspace blake3 pin keeps default features; `rayon` was always opt-in, and
  dropping `std` would have cost later crates `Hasher::update_reader`.
  [`Cargo.toml:31`](../../Cargo.toml#L31)

- The core-IO scan stops at the first `#[cfg(test)]`, so a test-only item above
  `mod tests` is itself a violation.
  [`arch-tests/src/lib.rs:169`](../../crates/arch-tests/src/lib.rs#L169)

- Bytes-hash is not parse equality: same DesiredState, different TOML bytes, no match.
  [`plan.rs:210`](../../crates/core/src/plan.rs#L210)

## Review Findings

Pass 1 (`/code-review #3`, high effort). 6 findings, all applied; 0 deferred, 0 rejected.

- **Plan integrity ordering (high).** `from_json` parsed the embedded TOML before comparing the digest, so tampered bytes surfaced as a parse error instead of `DigestMismatch`, and a future `SCHEMA_VERSION` would have made every archived plan undeserializable. The digest is now compared first and the schema re-parse is lazy in `Plan::desired_state`.
- **Typed `PlanError` was unreachable (medium).** Added `Plan::from_json_slice`, so a caller can branch on `DigestMismatch` instead of string-matching a flattened `serde` message. `PlanError::Json` carries the parse message.
- **No lower bounds on DesiredState (medium).** `range_len_mib = 0` divides by zero in the first consumer that counts chunks; `max_nvenc = 0` stalls encode. Both are now `MustBeNonZero` at parse. No ceiling was invented for `min_free_gib` — the only non-arbitrary bound is the existing `SizeOverflow` guard.
- **`Action` wire casing (medium).** Now `#[serde(rename_all = "snake_case")]`, settled before 1.4 freezes the contract.
- **blake3 `default-features = false` (low).** The flag never touched `rayon` (already opt-in); it only dropped `std`, which would have denied later crates `Hasher::update_reader`. Flag removed, rationale corrected. `Cargo.lock` is unchanged.
- **AD-2 core-IO scan truncation (low).** `HAPPY_TOML` sat above `mod tests`, silently ending the `std::fs` scan mid-file. It moved inside the test module, and the scan now reports any `#[cfg(test)]` item that appears before `mod tests`.

**Verification:** `cargo test --workspace --offline --locked` → all pass (core 81, arch-tests 11, CLI matrices unchanged). `cargo clippy --workspace --all-targets --offline --locked` → clean.
