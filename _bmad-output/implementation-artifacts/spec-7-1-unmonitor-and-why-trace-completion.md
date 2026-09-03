---
title: '7.1 Unmonitor and why-trace completion'
type: 'feature'
created: '2026-09-03'
status: 'done'
baseline_commit: 'c773297930578b05896977a62f3cde4284ad2e9f'
review_loop_iteration: 0
context:
  - '_bmad-output/implementation-artifacts/epic-7-context.md
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** *arr still monitors titles that already have an install digest at home, and `why`/`status` omit grab/import/hold and report home disk instead of seedbox `df`.

**Approach:** Reconcile Unmonitors `title_index.install_b3 ∩ wanted/missing` via seedbox Control. Lock-free `why`/`status` show grab → import → hold → pull → encode → library plus lock, home watermark, and seedbox `df`. Ranked reclaim preview stays in 7.2.

## Boundaries & Constraints

**Always:**
- Local proof for Unmonitor is a `title_index` row (`install_b3` is NOT NULL). On-disk schema files without a row do not Unmonitor. Size/mtime is not proof.
- Home-side intersection: Control snapshot of wanted/missing TitleIds, then `Action::Unmonitor { title_id }`. Apply calls `ControlPort::unmonitor`. CLI/`sync` never open *arr HTTP.
- Seedbox Unmonitor → `GrabOps::unmonitor`: GET wanted/missing, PUT `monitored: false` on the series/movie/album (not the episode). Not in the missing set → success no-op. `grabber=None` Unmonitor is usage; `wanted_missing` is empty.
- TitleId from `tmdbId` / `tvdbId` / `foreignAlbumId` (top-level or nested `movie`/`series`/`album` as in queue). Numeric *arr `id` is the PUT path only, never a TitleId.
- `why`/`status` stay lock-free. Local slices from Store + `is_file()`. Remote slices (grab, import, hold, `df`) from home UDS Control/Transfer. UDS down → those fields null, exit 0. `--json` envelope.
- Disk-full is seedbox `DfSnapshot.free`. Keep home `watermark` as `free_bytes(library_root)`. Additive `mediaops.v1`. Offline tests; grabber HTTP is cassettes.

**Ask First:** Wanted/missing cassette has no `tmdbId`/`tvdbId`/`foreignAlbumId` and no nested movie/series/album — do not invent another id field.

**Never:** reclaim preview/apply; `DeleteRemote`; Unmonitor without `install_b3`; CLI/`sync` *arr HTTP; replacing home watermark with `df`; auto-unmonitor timer; two-way sync; Research/LLM.

## I/O & Edge-Case Matrix

- local install_b3 + *arr missing → plan Unmonitor, apply PUT monitored:false (cassette), exit 0
- missing but no title_index row (on-disk only) → no Unmonitor
- already unmonitored / not in wanted/missing → Unmonitor RPC success, no PUT
- grabber=None Unmonitor → usage; wanted_missing empty; why/status still work
- why TITLE in grab/import/hold/pull/encode/library/lock → those fields set; library.present from `is_file()`; df from seedbox; watermark stays home
- why/status with no UDS → grab/import/hold/df null, local slices + lock still render, exit 0
- exclusive flock held → why/status still exit 0 and report lock holder

</frozen-after-approval>

## Code Map

Reuse: `ControlPort::unmonitor` / `df` (`crates/core/src/control_port.rs:18`). Proto Unmonitor (`proto/mediaops.proto:55`, client `crates/proto/src/lib.rs:570`). Seedbox stub (`crates/net/src/seedbox.rs:135`) + unused test (`:510` — drop Unmonitor). Gateway byte-forward (`crates/net/src/gateway.rs:117`). `Action::Unmonitor` unit (`crates/core/src/plan.rs:37`, exhaustive `:264`) — give it `title_id`. Apply no-op (`crates/sync/src/apply.rs:128`); copy GrabApply dispatch (`:118`). Planner Skip on any index row (`crates/sync/src/plan.rs:70`); Unmonitor only `install_b3` ∩ missing, not `on_disk`. `prepare_plan` already `hold_list` (`bins/mediaops/src/run.rs:296`). `TitleIndexEntry::install_b3` (`crates/core/src/title_index.rs:50`). `ArrClient::wanted_missing` (`crates/arr/src/servarr.rs:329`). `title_id_from_queue_item` (`crates/arr/src/apply.rs:1115`). `put_series`/`put_movie` (`sonarr.rs:27`, `radarr.rs:27`); add `put_album`. HoldReject + cassette pattern (`arr` + `fixtures/arr/hold_reject_delete.json`). FakeControl (`sync/src/apply.rs:915`, `hold.rs:76`). FakeGrabOps (`net/src/seedbox.rs:578`, `bins/mediaops/src/hold.rs:213`). Why/status (`bins/mediaops/src/status.rs` WhyData `:12` StatusData `:45`); lock-free (`bootstrap::lock_holder_if_contended`). CLI envelopes (`bins/mediaops/tests/cli.rs:500`). `connect_home` from `hold.rs:53`. AD-2 (`crates/arch-tests/src/lib.rs:9`).

Add: `ControlPort`/`GrabOps::wanted_missing() -> Vec<TitleId>`; rpc `WantedMissing` (repeated title_id strings + handshake); `GrabOps::unmonitor`; `PlanRequest.wanted_missing`; WhyData `{grab, import, hold, df}`; StatusData `df`; Why/Status `--socket`/`--tls-dir`.

## Tasks & Acceptance

**Execution:**
- [x] `crates/core/src/{plan,control_port}.rs` -- `Unmonitor { title_id }`; `wanted_missing` on both ports -- identity
- [x] `proto/mediaops.proto` + `crates/proto/src/lib.rs` -- additive WantedMissing; client -- AD-3
- [x] `crates/arr/src/{apply,servarr,sonarr,radarr,lidarr}.rs` + `fixtures/arr/` -- snapshot + PUT monitored:false cassettes -- AD-15
- [x] `crates/net/src/{seedbox,gateway}.rs` -- implement Unmonitor + WantedMissing; grabber=None -- AD-4
- [x] `crates/sync/src/{plan,apply,hold}.rs` -- emit/apply Unmonitor; FakeControl -- AD-9
- [x] `bins/mediaops/src/{run,status,main,hold}.rs` + `tests/cli.rs` -- prepare_plan snapshot; lock-free why/status chain + df -- FR7

**Acceptance Criteria:**
- Given install_b3 and *arr wanted/missing, when exclusive `run` applies, then *arr is PUT `monitored: false` via Control, not from the CLI process.
- Given `why TITLE --json` / `status --json`, when the title is in grab, import, hold, pull, encode, library, watermark, or lock, then that slice is present and library presence is the home file, not *arr.
- Given disk-full, when why/status run with UDS, then `df` is seedbox `free_bytes` (reclaim preview absent).

## Spec Change Log

## Design Notes

Unmonitor PUT is the parent record (Sonarr series, not episode). `wanted_missing` is a snapshot RPC, not a field on HoldList. why `import` = TitleId in Transfer List; `grab` = TitleId in wanted/missing or queue; `hold` = `hold_list`. Do not add `reclaim` JSON.

## Verification

**Commands:**
- `cargo test --workspace --offline --locked` -- pass (Unmonitor cassette, no-row, grabber=None, why chain, df, lock-free)
- `cargo test -p mediaops-arch-tests --offline --locked` -- no arr in CLI; no reqwest outside arr

## Suggested Review Order

**Plan intersection**

- Home emits Unmonitor only for Servarr ∩ title-index ∩ wanted/missing
  [`plan.rs:264`](../../crates/sync/src/plan.rs#L264)

- Plan action carries the TitleId apply will unmonitor
  [`plan.rs:37`](../../crates/core/src/plan.rs#L37)

- Exclusive plan snapshots wanted/missing over home UDS
  [`run.rs:313`](../../bins/mediaops/src/run.rs#L313)

**Apply and Control**

- Apply dispatches Unmonitor through Control, never *arr HTTP
  [`apply.rs:128`](../../crates/sync/src/apply.rs#L128)

- Additive WantedMissing snapshot on Control
  [`mediaops.proto:64`](../../proto/mediaops.proto#L64)

- Seedbox Unmonitor is usage if grabber is none
  [`seedbox.rs:136`](../../crates/net/src/seedbox.rs#L136)

**Grabber HTTP**

- GET wanted/missing continues past a single *arr failure
  [`apply.rs:256`](../../crates/arr/src/apply.rs#L256)

- GET parent then PUT monitored false; never episode id
  [`apply.rs:1327`](../../crates/arr/src/apply.rs#L1327)

**Why / status**

- Lock-free why fills grab/import/hold/df over UDS; UDS down is null
  [`status.rs:256`](../../bins/mediaops/src/status.rs#L256)

- Home watermark stays; seedbox df is a separate field
  [`status.rs:18`](../../bins/mediaops/src/status.rs#L18)

- Control usage from apply stays exit 2
  [`run.rs:370`](../../bins/mediaops/src/run.rs#L370)

**Tests**

- Cassette PUT is asserted via hit counts, not expect-ok
  [`apply.rs:2545`](../../crates/arr/src/apply.rs#L2545)

- cmd_plan observes Unmonitor in the plan JSON
  [`run.rs:542`](../../bins/mediaops/src/run.rs#L542)
