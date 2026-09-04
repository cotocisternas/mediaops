---
title: '7.2 Reclaim preview and apply'
type: 'feature'
created: '2026-09-03'
status: 'done'
baseline_commit: 'e4d6a8af9086908705cd2f5024740991b35a98f2'
review_loop_iteration: 0
context:
  - '_bmad-output/implementation-artifacts/epic-7-context.md
  - '_bmad-output/implementation-artifacts/spec-7-1-unmonitor-and-why-trace-completion.md
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Remote buffer has no real reclaim: `DeleteRemote`/`Reclaim` are no-ops, there is no ranked dry-run, and Copy must never imply torrent delete.

**Approach:** Lock-free `reclaim preview --json` ranks surplus by ratio, private, and age. Exclusive `reclaim apply` unlinks allowlisted remotes only after `install_b3` proof. qBit is queried inside seedbox `DeleteRemote`; seeding is `SkippedSeeding`. No reclaim timer.

## Boundaries & Constraints

**Always:**
- Proof is `title_index.install_b3` plus the library file on disk (7.1). No digest → no delete. Size/mtime is not proof.
- Preview is lock-free dry-run. Apply takes the exclusive flock. No `mediaops-reclaim.timer`. No leftover no-op on this verb's apply path.
- Rank by ratio, private, age. Private-under-goal is omitted (untouched). `nlink > 1` (library hardlink of a torrent) skips. Usenet-complete (no qBit match) is deletable after proof.
- Seedbox `DeleteRemote`: qBit query and unlink in one handler. Seeding → `SkippedSeeding` (data, not exit 5). GuardPreview is preview/`why` only, never a delete precondition.
- Unlink only through the PathSchema allowlist. Unknown paths error. Never follow symlinks off it. Never walk torrent save paths or `torrents/incomplete`.
- Torrent unlink belongs to reclaim apply only. Copy must not call `delete_remote` or qBit `delete`. CLI/`sync` never open grabber HTTP. Additive `mediaops.v1`. `--json`. Offline cassettes. `grabber=None` works: qBit down → `SkippedSeeding`; usenet with proof may unlink.
- `why`/`status` may add a reclaim sibling next to seedbox `df`; do not replace home watermark.

**Ask First:** qBit `torrents/info` cassette has no `state` / `ratio` / `is_private` / `content_path`|`save_path` / `hash` — do not invent other field names. qBit.conf has no WebUI user/password — do not hardcode `admin`/`adminadmin`.

**Never:** two-way sync; delete without `install_b3`; treating size/mtime as proof; Copy deleting torrents; GuardPreview as a delete precondition; reclaim systemd timer; CLI seedbox address; Research/LLM; Unmonitor/hold changes; Epic 8 reindex (still refuse when digest missing).

## I/O & Edge-Case Matrix

- preview --json → ranked candidates; no mutation; lock-free
- apply, install_b3 + on-disk + not seeding + not private-under-goal + nlink==1 → allowlist unlink, Deleted
- apply, qBit seeding → SkippedSeeding, file remains
- apply, private-under-goal or nlink>1 or no install_b3 → no delete
- apply, exclusive flock held → exit 3
- Copy of a torrent library hardlink → no DeleteRemote, no qBit delete
- grabber=None / qBit down → SkippedSeeding, no crash
- no reclaim timer; `run` Copy never deletes remote

</frozen-after-approval>

## Code Map

Reuse: `Action::DeleteRemote`/`Reclaim` units (`crates/core/src/plan.rs:40,44`). Apply no-op (`crates/sync/src/apply.rs:152`); Copy never deletes (`:165`). `ControlPort::delete_remote`/`guard_preview` (`crates/core/src/control_port.rs:20,34`). `DeleteRemoteOutcome` (`:130`). Proto `DELETED=1 SKIPPED_SEEDING=2` (`proto/mediaops.proto:118`); client `crates/proto/src/lib.rs:604`; GuardPreview empty (`:175`). Seedbox stubs (`crates/net/src/seedbox.rs:164,296`; unused test `:556` — drop both). Gateway (`crates/net/src/gateway.rs:131`). `RemoteEntry` nlink/mtime (`crates/core/src/walker.rs:80`); no unlink; `is_torrent_skip` `:525`. `install_b3` (`crates/core/src/title_index.rs:50`). `QbitClient::torrents` (`crates/arr/src/qbit.rs:119`); unused outside that file. GrabOps has no qBit; Servarr-only inject (`bins/mediaopsd/src/main.rs:269`) — qBit guard must not require Servarr. Locks: why/hold vs `run.rs:273`. Nested clap `Hold` (`bins/mediaops/src/main.rs:158`). df vs watermark (`status.rs:18`). FakeControl (`sync/src/apply.rs:962`). Only timer: `mediaops-run.timer` (`crates/sync/src/lib.rs:64`). AD-2.

Add: `ReclaimPolicy` in core; `DeleteRemote { remote }`; allowlist unlink; GrabOps qBit snapshot; additive GuardPreview items; seedbox DeleteRemote=guard+unlink; `sync` preview/apply; CLI `reclaim preview|apply`; why/status sibling; cassettes; all fakes.

## Tasks & Acceptance

**Execution:**
- [x] `crates/core/src/{plan,control_port,walker,reclaim}.rs` -- DeleteRemote payload; ReclaimPolicy; allowlist unlink -- identity
- [x] `proto/mediaops.proto` + `crates/proto/src/lib.rs` -- additive GuardPreview items -- AD-3
- [x] `crates/arr/src/{qbit,apply}.rs` + `fixtures/arr/` -- typed torrents/info guard cassettes -- AD-15
- [x] `crates/net/src/{seedbox,gateway}.rs` -- DeleteRemote atomic with qBit; implement GuardPreview -- AD-4
- [x] `crates/sync/src/{reclaim,apply,plan}.rs` -- preview rank; apply DeleteRemote; Copy still no-op remote delete -- AD-9
- [x] `bins/mediaops/src/{main,reclaim,status,run}.rs` + `tests/cli.rs` -- lock-free preview; exclusive apply; why/status sibling -- FR7

**Acceptance Criteria:**
- Given `reclaim preview --json`, when it runs, then candidates are ranked by ratio, private, and age, and nothing is unlinked.
- Given `reclaim apply` and `install_b3` plus on-disk, when the remote is not seeding and not private-under-goal, then seedbox unlinks via allowlist and qBit was queried in that same handler.
- Given seeding, when DeleteRemote runs, then outcome is `SkippedSeeding` and the file remains.
- Given Copy of a torrent that is a library hardlink, when sync finishes, then no DeleteRemote and no qBit delete.

## Spec Change Log

## Design Notes

`reclaim apply` emits `DeleteRemote { remote }`; do not fold into `mediaops run`. Rank: omit private-under-goal; public/low-ratio/older first; usenet is age-only. Fail-closed if qBit cannot answer. Goal ratio is a `ReclaimPolicy` constant, not DesiredState.

## Verification

**Commands:**
- `cargo test --workspace --offline --locked` -- pass (preview rank, seeding skip, no-digest, hardlink skip, Copy-no-delete, grabber=None, no reclaim timer)
- `cargo test -p mediaops-arch-tests --offline --locked` -- no arr in CLI; no reqwest outside arr

## Suggested Review Order

**Policy and ranking**

- Home ranks surplus; private-under-goal and seeding never become candidates
  [`reclaim.rs:184`](../../crates/core/src/reclaim.rs#L184)

- qBit path match is component-bounded so `/` cannot cover the library
  [`reclaim.rs:99`](../../crates/core/src/reclaim.rs#L99)

**DeleteRemote**

- Seedbox queries qBit and unlinks in one handler; seeding/private skip
  [`seedbox.rs:165`](../../crates/net/src/seedbox.rs#L165)

- Allowlist unlink never follows symlinks or torrent save paths
  [`walker.rs:384`](../../crates/core/src/walker.rs#L384)

- Apply records Deleted vs SkippedSeeding and does not abort Copy
  [`apply.rs:154`](../../crates/sync/src/apply.rs#L154)

**CLI**

- Lock-free preview / exclusive apply; not folded into `run`
  [`reclaim.rs:29`](../../bins/mediaops/src/reclaim.rs#L29)

- Dispatch for `reclaim preview|apply --json`
  [`main.rs:870`](../../bins/mediaops/src/main.rs#L870)

**Wire**

- Additive GuardPreview items for ranking only, never a delete precondition
  [`mediaops.proto:177`](../../proto/mediaops.proto#L177)

**Tests**

- Copy still never calls delete_remote
  [`apply.rs:1193`](../../crates/sync/src/apply.rs#L1193)
