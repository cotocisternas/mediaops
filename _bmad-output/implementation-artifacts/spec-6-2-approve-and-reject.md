---
title: '6.2 Approve and Reject'
type: 'feature'
created: '2026-09-02'
status: 'done'
baseline_commit: '3d11022d7f2a780843b6841c37b030efe40d5787'
review_loop_iteration: 0
context:
  - '_bmad-output/implementation-artifacts/epic-6-context.md
  - '_bmad-output/implementation-artifacts/spec-6-1-hold-store-and-inbox-join.md
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Operators can list import-blocked holds but cannot Approve (promote through PathSchema) or Reject (never-this-release). Auto-approve must stay impossible.

**Approach:** Lock-free `hold approve`/`hold reject` persist `HoldDecision` and a Hold job. Reject also calls additive seedbox `HoldReject` so *arr can try another. Promotion through `install` runs on the exclusive apply path, never inside the approve process. Omit Research.

## Boundaries & Constraints

**Always:**
- `hold approve`/`reject` are lock-free. They may only `HoldsRepo::put` and Hold-job create/advance — never `install`/`replace` or writes under movies/series/music/`_incoming`.
- Key must be in the current inbox (live ⊖ decided). Else usage (exit 2). After `put(Approved|Rejected)` it drops out of `hold list`.
- Approve PathSchema-preflights (strip scene tags, then `render`). Spaces or leftover REPACJ/REPACK/PROPER → policy (exit 5), no row.
- Exclusive `plan`/`run`: each `Approved` key with a live RemoteRef becomes `Action::Copy` through existing `apply_copy`/`install`. Already-installed → `upgrade_never`.
- Reject: `put(Rejected)` then Control `hold_reject(HoldKey)`. Inside `arr` only: match HoldKey, `DELETE queue/{id}?removeFromClient=true&blocklist=true`. Queue `id` stays off proto. `grabber=None` reject is usage, not silent 0.
- No auto-approve, agent-approve, or confidence floor (cite that named failure). `--json` envelope. Offline tests; grabber mutations are cassettes. Additive `mediaops.v1`. CLI uses home UDS only.

**Ask First:** Reject cassette/queue JSON still has no numeric record `id` — do not guess `downloadId` as path id.

**Never:** `install` from `hold approve`; auto/agent-approve; LLM/Research; CLI/`sync` speaking *arr HTTP; `store` in mediaopsd; `_incoming` as a hold folder; a second join outside `sync`; POST `/blocklist`; changing HoldKey mapping.

## I/O & Edge-Case Matrix

- inbox approve → `put(Approved)`, Hold job Approved, list omits key, exit 0; no schema writes
- leftover scene tag / spaces → exit 5, no decision row
- exclusive flock held → approve/reject still exit 0
- `run` after Approved + live RemoteRef → Copy/install schema path (not scene name)
- already in title-index → Skip upgrade_never
- inbox reject → `put(Rejected)` + DELETE queue `blocklist=true`; list omits key
- grabber=None reject or unknown key → usage, no new decision
- no auto-approve path (no confidence field, no timer that puts Approved)

</frozen-after-approval>

## Code Map

Reuse: `HoldKey`/`HoldDecision`/`HoldsRepo::put` (`core/src/hold.rs`, `store/src/holds.rs:44`). `inbox` (`sync/src/hold.rs:8`). `hold list` (`bins/mediaops/src/hold.rs:29`). `advance_hold` (`jobs.rs:533`). `apply_copy`/`install`/`render`/`strip_scene_tags`. `ArrClient::delete` + `get_paged_with`. HoldList/gateway/GrabOps pattern. FakeControl in `sync` and `hold.rs` tests. Cassette `push`.

Add: optional `remote`/`placement` on `HoldLiveItem` (additive proto). `outputPath`→RemoteRef on seedbox allowlist; title/year/ext/S/E from nested *arr objects only in `arr` (list JSON envelope unchanged). `hold_reject(&HoldKey)` on ControlPort/GrabOps; `rpc HoldReject`; gateway proxy; None → usage ControlError. `arr` matches HoldKey, `DELETE queue/{id}?removeFromClient=true&blocklist=true`, cassette with `id`. `plan_actions`: Approved+live remote+placement → Copy; do not use `Action::Review`. CLI `hold approve|reject TITLE_ID RELEASE_ID`.

## Tasks & Acceptance

**Execution:**
- [x] `crates/core/src/{hold,control_port}.rs` -- optional remote/placement; `hold_reject` ports -- identity
- [x] `proto/mediaops.proto` + `crates/proto/src/lib.rs` -- HoldReject + additive live fields -- AD-3
- [x] `crates/arr/src/{apply,servarr}.rs` + `fixtures/arr/` -- placement/outputPath; DELETE+blocklist cassette -- AD-15
- [x] `crates/net/src/{seedbox,gateway}.rs` -- HoldReject; outputPath→RemoteRef; None is usage -- AD-4
- [x] `crates/sync/src/{plan,apply,hold}.rs` -- Approved+remote → Copy; FakeControl -- AD-9
- [x] `bins/mediaops/src/{main,hold}.rs` + `tests/cli.rs` -- lock-free JSON; PathSchema refuse; no auto-approve -- FR17

**Acceptance Criteria:**
- Given a listed hold, when `hold approve --json` runs, then the key and Hold job are Approved, it leaves the inbox, and that process writes no library/`_incoming` file.
- Given spaces or leftover scene tags in the intended placement, when approve runs, then exit 5 and no decision row.
- Given Approved plus a live RemoteRef, when exclusive `run` applies, then Copy/`install` lands on a PathSchema path (not the scene name).
- Given a listed hold, when `hold reject --json` runs, then the key is Rejected, *arr is `DELETE queue/{id}?blocklist=true` (cassette), and it leaves the inbox.
- Given grabber=None or a key not in the inbox, when approve/reject runs, then usage and no new decision.
- Given CLI/apply, when inspected, then no auto-approve or agent-approve path exists.

## Design Notes

CLI: `hold approve|reject <title_id> <release_id>`. JSON `data`: `{title_id, release_id, decision}`. Approve does not install (lock class); `run` consumes `Approved` ∩ live as next-run input. Reject cassette must include queue `id`; HoldKey mapping unchanged; `skipRedownload` stays default false. Omit `hold research`. No confidence field.

## Verification

**Commands:**
- `cargo test --workspace --offline --locked` -- pass (approve/reject matrix, PathSchema refuse, reject cassette, Approved Copy)
- `cargo test -p mediaops-arch-tests --offline --locked` -- no store in mediaopsd; no arr in CLI

## Suggested Review Order

**Lock-free verbs**

- Approve/reject persist a decision; they never call install.
  [`hold.rs:94`](../../bins/mediaops/src/hold.rs#L94)

- PathSchema preflight refuses spaces and leftover scene tags.
  [`hold.rs:120`](../../crates/core/src/hold.rs#L120)

- CLI: `hold approve|reject TITLE RELEASE`, lock-free, no Research.
  [`main.rs:229`](../../bins/mediaops/src/main.rs#L229)

**Reject → *arr**

- Additive HoldReject; queue id stays off the wire.
  [`mediaops.proto:63`](../../proto/mediaops.proto#L63)

- Match HoldKey then DELETE queue/{id}?blocklist=true.
  [`apply.rs:951`](../../crates/arr/src/apply.rs#L951)

- grabber=None reject is usage, not silent 0.
  [`seedbox.rs:313`](../../crates/net/src/seedbox.rs#L313)

**Promote on exclusive run**

- *arr titles become dotted PathSchema tokens without requiring *File.
  [`apply.rs:999`](../../crates/arr/src/apply.rs#L999)

- Directory outputPath maps to exactly one media file.
  [`seedbox.rs:340`](../../crates/net/src/seedbox.rs#L340)

- Approved ∩ live RemoteRef becomes Copy, not Review.
  [`plan.rs:103`](../../crates/sync/src/plan.rs#L103)

- `run`/`plan` load Approved holds from store ∩ live list.
  [`run.rs:279`](../../bins/mediaops/src/run.rs#L279)

**Peripherals**

- Approve does not blocklist; reject advances Hold job.
  [`hold.rs:407`](../../bins/mediaops/src/hold.rs#L407)
