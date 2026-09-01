# Review findings — PR #5 (Epic 4)

Source: `main...epic/4-tonight-playable`, chunked by story. SPEC-mediaops + `increments.md` + spine ADs + `epics.md` 4.1–4.3 + `epic-4-handoff.md`.
Layers: blind-hunter, edge-case-hunter, verification-gap, acceptance-auditor.

SPEC.md has no Tasks/Subtasks section; this file is the persisted review list.

## Chunk 4.1 — `ee76a85` plan/run/watch/why/status

27 files, +2567 / −198. Remaining chunks: 4.2 encode, 4.3 runbook/live-box.

4.1 patches applied 2026-09-01. `cargo test --workspace --offline` green.

### Review Findings

- [x] [Review][Patch] Extra listings that share a TitleId emit `Skip { duplicate_title }` instead of silent omit (1:1 TitleId kept) [`crates/sync/src/plan.rs:67`]
- [x] [Review][Patch] Plan treats on-disk schema files as installed (`upgrade_never`) via `scan_schema_files` [`bins/mediaops/src/run.rs:243`]
- [x] [Review][Patch] Conflicting `plan`/`run` truncates lockfile JSON; `status`/`why` hide the holder [`bins/mediaops/src/bootstrap.rs:539`]
- [x] [Review][Patch] `min_free_gib = 0` copies a file larger than free disk and underflows `remaining_free` [`crates/sync/src/plan.rs:131`]
- [x] [Review][Patch] `why TITLE` reports the oldest Want, not the open/current one [`bins/mediaops/src/status.rs:70`]
- [x] [Review][Patch] Apply swallows Want `Satisfy` after a successful install [`crates/sync/src/apply.rs:211`]
- [x] [Review][Patch] `record_install` runs before `PullEvent::Install`; a failed Install leaves pull `Verifying` with an index row [`crates/sync/src/apply.rs:203`]
- [x] [Review][Patch] `JobView` has no `title_id`; `status --json` cannot name stuck titles [`bins/mediaops/src/status.rs:23`]
- [x] [Review][Patch] `status` does not surface box-level watermark (`free` / `min_free`) [`bins/mediaops/src/status.rs:43`]
- [x] [Review][Patch] `why` treats title-index path as library truth without `stat`; scan/DS errors collapse to empty/`0` [`bins/mediaops/src/status.rs:99`]
- [x] [Review][Patch] `run` applies the in-memory Plan, not the JSON artifact it just wrote [`bins/mediaops/src/run.rs:114`]
- [x] [Review][Patch] `watch` is check-then-insert with no unique open-want constraint [`bins/mediaops/src/watch.rs:31`]
- [x] [Review][Patch] `Skip { upgrade_never }` does not satisfy an open Want [`crates/sync/src/apply.rs:105`]
- [x] [Review][Patch] Apply reuses any non-Installed Pull without matching remote/len/placement [`crates/sync/src/apply.rs:159`]
- [x] [Review][Patch] `placement_for` ignores parse_placement TitleId mismatch and falls through to `--title/--year` [`bins/mediaops/src/home.rs:318`]
- [x] [Review][Patch] Plan filenames collide and overwrite in the same UTC second [`bins/mediaops/src/run.rs:260`]
- [x] [Review][Patch] `cmd_run` empty-apply / `first_candidate_breaches` policy is untested [`bins/mediaops/src/run.rs:102`]
- [x] [Review][Patch] Desired-state `lock = true` is untested in plan and apply [`crates/sync/src/plan.rs:101`]
- [x] [Review][Patch] `upgrade_never` is not verified through `Store::list_titles` after apply `record_install` [`crates/sync/src/apply.rs:203`]
- [x] [Review][Patch] v5 `title_index.path` migration and empty-path fallback are untested [`crates/store/src/lib.rs:310`]
- [x] [Review][Patch] `status`/`why` lock-holder read is never asserted under a held flock [`bins/mediaops/src/bootstrap.rs:515`]
- [x] [Review][Patch] Apply success does not assert Want parent or Satisfied [`crates/sync/src/apply.rs:163`]
- [x] [Review][Patch] Album `pull --install` never adopts `parse_placement` in a test [`bins/mediaops/src/home.rs:318`]
- [x] [Review][Patch] Home-unit CLI test matches `--role` and `home` in Description, not `ExecStart=` [`bins/mediaops/tests/cli.rs:328`]
