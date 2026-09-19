---
title: Restore healthy home sync
type: bugfix
created: '2026-09-19'
status: done
baseline_commit: 7b4c9045f67c33d78609db0b168d2a122020b8f9
review_loop_iteration: 0
context:
  - AGENTS.md
  - docs/setup.md
  - docs/usage.md
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The installed Home control plane is healthy but Cluster scheduling is locked, preventing even a sync preview. Doctor also fails when invoked from a deep Git working tree. The user wants working sync to the confirmed local destination `/mnt/storage/videos`, with existing telemetry work preserved and all default tests passing.

**Approach:** Repair the doctor traversal without weakening credential checks, validate and deploy the home binaries, then recover the stale runtime lock and preview/start a finite one-shot sync. Verify actual installed output or report concrete remaining blockers, rather than treating queued work as completed work.

## Boundaries & Constraints

**Always:** Preserve existing staged telemetry changes. Use the Home API and gateway for remote interactions. Keep the 256 GiB free-space reserve and existing 80 GiB concurrent-copy budget. Use current resourceVersion for Cluster updates. Coordinate recovery through the maintenance flock and recheck active work. Keep runtime credentials out of the repository and logs. Treat docs/ as current authority.

**Ask First:** Changing the library destination, lowering disk safeguards, destructive conflict resolution, remote repair/upgrade, or implementing continuous sync rather than the existing finite sync command.

**Never:** Reclaim remote files, run encoding, SSH to the seedbox, overwrite existing library media, bypass PEM protections, or silently discard telemetry changes. No broad lint cleanup. Do not assume the concurrency budget caps total sync bytes.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|---|---|---|---|
| Deep clean checkout | Git tree deeper than four levels | Doctor finishes credential scan and continues health checks | Real I/O failures still fail closed |
| Deep credential | PEM/CRT/KEY below previous cutoff | Policy refusal naming the path | No credential contents printed |
| Symlink entries | External directory, cycle, credential-named link | No following child directory symlinks; credential-named links refused | Never report incomplete traversal as clean |
| Stale runtime pause | No maintenance holder or bound work | Only Cluster lock changes to false | Resource-version conflict requires reinspection |
| New maintenance/work | Active maintenance or incompatible jobs | Recovery stops without interfering | Report blocker |
| Eligible sync | Fresh inventory and adequate capacity | Finite Jobs progress to verified installation | Inspect failure reasons; no blind duplicate requests |
| Budget pressure/conflicts | Preview includes excess work or conflicting placements | Preserve capacity and installation guards | Report blocked/pending entries rather than weaken safeguards |

</frozen-after-approval>

## Code Map

- `bins/mediaops/src/doctor.rs`: `walk_pems` aborts at depth >4 and follows directory symlinks through `is_dir`; preserve config/TLS/cwd scans.
- `bins/mediaops/tests/cli.rs`: subprocess current-directory regressions without process-global cwd changes.
- `crates/ssh/src/lib.rs`: shared fail-closed Git detection; leave bootstrap/import security protections intact.
- `bins/mediaops/src/home_library.rs`: maintenance pause/restoration and bound-job checks.
- `bins/mediaops/src/bootstrap.rs`: maintenance flock metadata and acquisition; old holder metadata is not a live lock.
- `crates/api/src/sync.rs`: locked Clusters reject preview and real sync.
- `crates/api/src/sync_plan.rs`: finite inventory and immutable Job snapshots.
- `bins/mediaops-scheduler/src/main.rs`: rolling concurrent budget, not total-request byte cap.
- `bins/mediaops-pull/src/main.rs`: verified local installation and staging cleanup.
- `bins/mediaops-inventory/src/unmonitor.rs`: existing Servarr behavior may unmonitor proved-present movies/albums after installation; sync does not reclaim remote data.
- `Makefile`: sibling workspace build before default tests; install deploys home roles and CLI.

## Tasks & Acceptance

**Execution:**
- [x] `bins/mediaops/src/doctor.rs` — replace arbitrary depth limit with safe iterative traversal and add deep-tree, symlink, Git-marker, and credential regression tests.
- [x] `bins/mediaops/tests/cli.rs` — cover deep checkout cwd behavior without weakening policy exits.
- [x] `Makefile` validation entrypoints — run focused tests, full default gate, formatting, Clippy, and proto checks; fix confirmed failures within recovery scope.
- [x] `/home/coto/.cargo/bin/` installed home binaries — deploy validated changes with a coordinated service restart before transfers begin.
- [x] Runtime Cluster `home` and Sync — under maintenance coordination, recheck queues, recover only the lock, inspect fresh preview, submit an idempotent request, and verify Job/Title/file outcomes.
- [x] `docs/plans/restore-home-sync.md` — record results and unresolved operational constraints without credentials.

**Acceptance Criteria:**
- Given this checkout as cwd, when doctor runs after deployment, then directory depth alone does not prevent complete health checks.
- Given existing telemetry work, when fixes are applied, then that work remains intact and default tests pass.
- Given the confirmed home destination, when sync runs, then installed Jobs have corresponding Title proofs and local files, or exact blockers are reported without claiming completion.

## Spec Change Log

## Verification

- Baseline `make test`: 889 passed, zero failed; sandbox socket denials disappear under approved execution.
- Baseline `make fmt-check` and `make proto`: passed.
- Baseline `make clippy OFFLINE=1`: succeeds with existing warnings.
- Repeat full validation after changes.
- Runtime: doctor, status, Node freshness, sync preview, Sync/Job outcomes, and installed Title/local-file evidence. No GPU/live-box test.

### Implementation and validation — 2026-09-19

- Doctor now scans iteratively without an arbitrary depth limit. `DirEntry::file_type` avoids following child directory symlinks, including external directories and cycles. Credential-named symlinks (including dangling links and links to directories) still refuse. Read-directory, entry, and type errors remain policy refusals with path context. Existing config/TLS/cwd scans, Git detection, and bootstrap/import protections are unchanged.
- Added five regression tests covering deep clean trees, deep `.pem`/`.crt`/`.key` files, directory and file Git markers, child symlinks, fail-closed missing-root I/O, and subprocess cwd scan-continuation/policy exits (the clean-cwd fixture intentionally stops at a missing gateway, while deployed doctor provides the successful health-check evidence) without changing process-global cwd. No credential contents appear in errors.
- `cargo test -p mediaops --locked --offline doctor`: passed (9 unit tests and 3 CLI tests selected). Initial sandbox attempt hit Unix-socket bind denials in two existing integration tests; approved unsandboxed execution passed.
- `make test OFFLINE=1`: **894 passed, 0 failed**, including the required sibling workspace build. No live-box feature, GPU test, SSH, or encoding.
- `make fmt-check`, `make proto`, and `make clippy OFFLINE=1`: passed. Existing Clippy warnings remain; no broad lint cleanup. `git diff --check`: passed.
- Existing matrix protection tests passed, including maintenance lock conflicts/restoration, API bind-time maintenance checks, finite Sync/idempotency tests, destination-proof and symlink conflicts, disk watermarks, and verified installation/staging cleanup.
- Staged telemetry diff remained unchanged: SHA-256 of `git diff --cached` was `b24df3e851f2b4a06d0e27740bd43d08ae0cf4416d8e205d7c1e891c0693845a` before deployment and after code validation. No Git index writes, commits, or branch changes.

### Deployment and recovery

- Built release `mediaops`, `mediaops-home`, `mediaops-api`, `mediaops-scheduler`, `mediaops-gateway`, `mediaops-inventory`, and `mediaops-pull` with `--locked --offline`. Held the existing maintenance flock, rechecked the locked Cluster and empty Job/Want/Sync queues plus absence of approved Holds, stopped the Home service, atomically replaced those seven installed binaries, and restarted the service. No remote binaries or TUI changes.
- Installed binary SHA-256 hashes match the release builds. `mediaops-home.service` is active/running. Deployed `mediaops doctor -o json` succeeds from this checkout, with edge invariant true, no drift, and Ready scheduler/inventory/pull Nodes.
- Reacquired the maintenance flock and repeated work checks before recovery. Updated only `Cluster.spec.lock: true -> false`, applying current resourceVersion `539879` and receiving `5213540`; verified all other spec fields unchanged. No resource-version conflict occurred and no automatic conflict retry was used.
- Destination remains `/mnt/storage/videos`; reserve remains **256 GiB**, concurrent-copy budget **80 GiB**, Range length **32 MiB**, and Range concurrency **8**. The concurrent budget is not a total-request cap.
- Fresh dry-run inventory generation `88198`: **13 wouldQueue** entries totaling **38,195,638,679 bytes (35.6 GiB)**, **45 blocked**; confirmed preview wrote no Jobs. Available space before copying: **583,393,951,744 bytes (543.3 GiB)**, sufficient for the reserve plus this batch and temporary installation space.
- After another flock/queue check, submitted exactly one finite request with idempotency key **`restore-home-sync-20260919`**. It captured fresh inventory generation `88201`, scheduled **13 Jobs**, and retained **45 blocked** entries. No Wants were created and no duplicate request/retry was submitted.

### Verified outcome and remaining operational work

At the final file-proof checkpoint, **3 Jobs were Installed, 1 Pulling, and 9 Pending**, with no Failed or Refused Jobs. All three installed files were checked directly for regular-file type and exact size, with nondrifted Title digests matching their Jobs. Total verified installed bytes: **9,797,744,582**. The finite queue is still running, not complete.

First installation evidence:

- Job: `pull-movie-key-theendofoakstreet.2026-whole`.
- Title: `movie:key:theendofoakstreet.2026`.
- Local file: `/mnt/storage/videos/movies/The.End.of.Oak.Street.(2026)/The.End.of.Oak.Street.(2026).mkv`.
- Verified regular non-symlink file size: **7,413,836,034 bytes**.
- Job `verifiedB3`, Title `installB3`, and Title `currentB3` all equal `2f7446b2fe56498dc6ee66a8f81589bd391d326d62fd2cb607e331a2de79530e`; Title is not drifted. This is the worker's verified digest plus direct file/type/size inspection, not an additional independent rehash.
- Also verified installed: `pull-series-key-itsalwayssunnyinphiladelphia.2005-s18-e5` (**1,238,114,534 bytes**, digest `5362aa90807f005059c663156293a9958e9557867a5cb0b168df2aa075aa3db9`) and `pull-series-key-itsalwayssunnyinphiladelphia.2005-s18-e6` (**1,145,794,014 bytes**, digest `d48ef01de240a6d4062843e76dfa131dea6df02b599cd2d25695bfb7d32c873f`). Available space at that checkpoint: **573,227,229,184 bytes**, still above the reserve.
- All 13 Jobs were bound to the pull worker within the existing budget. Pending bound Jobs wait for worker execution; scheduling is not proof of installation. Follow with `mediaops get Job -o wide` and `mediaops get Sync restore-home-sync-20260919 -o json`. Do not resubmit merely because some Jobs remain Pending.

Exact retained blockers:

1. **44 Mr. Robot files already exist as regular local files.** One entry has a drifted recorded Title proof (`series:key:mrrobot.2015`, S01E01); the API reports that its recorded placement is drifted/missing/unreadable or traverses a symlink. The other **43** are blocked with `destination or unsafe parent already exists without a matching proof`. Existing content and proofs were not changed. Resolving these requires a separately considered inspection/reindex/repair, never overwrite or destructive conflict resolution without approval.
2. **One Hearts of Darkness release** remains blocked with `source requires an explicit, unambiguous Hold decision`. No Hold decision was changed.
3. The initial checkpoint was incomplete; the final checkpoint below supersedes it. Do not resubmit this completed request.

Three independent parent-session reviews completed (blind, edge-case, verification-gap). Corrected the description of the clean-cwd subprocess regression: it proves continuation to the gateway, not successful end-to-end health. Live deployed doctor supplies separate health evidence. Static child symlinks are excluded; pathname traversal is not a security boundary against hostile concurrent directory replacement (a pre-existing check/use race, not a new guarantee). Existing telemetry work has follow-up hardening/test gaps: whole-export response-body deadlines, shutdown-thread spawn-failure handling, unavailable-collector recovery coverage, and automated production-callsite export assertions. Worker terminal metrics exclude controller-written refusals, and scheduler missing-node/error handling can obscure API failures. These are recorded for focused follow-up, not silently claimed fixed. Generated fallback review prompts are superseded by the completed parent reviews.

No reclaim, encoding, SSH, remote repair/upgrade, disk safeguard reduction, or continuous-sync implementation was performed. Existing Servarr inventory behavior may unmonitor proved-present movies/albums after installation; no remote media deletion was requested.

### Reindex scope assessment

All 44 blocked Mr. Robot destinations are canonical, readable regular files without symlink components and exactly match listed sizes (160,804,141,526 bytes total). This does not prove content equality. Whole-library reindex would scan approximately 2,556 candidate files / 3.40 TB, including the supported music-root symlink, with repeated hashing potentially exceeding 10 TB of logical reads. It has no targeted-title option. Another existing drifted proof belongs to Radiohead / Amnesiac track 1. Reindex can clear identical drifted proofs and add missing ones, but changed digests refuse and failures can leave Cluster locked. No whole-library reindex was started as an incidental repair.

### Final completion — 2026-09-19 17:59:58 UTC

All **13 of 13** manifest Jobs (matched by name and UID) are Installed: **38,195,638,679 bytes / 35.572 GiB**. No Pending, Pulling, Verifying, Failed, or Refused Jobs remain in this batch. Every destination was checked as a regular file without symlink components, with exact Job length and matching nondrifted Title proof; Job verifiedB3, Title installB3 and currentB3 agree. No independent rehash was performed. Doctor exited 0; all Nodes were Ready with fresh heartbeats. Disk available was **545,199,259,648 bytes / 507.756 GiB**, safely above the unchanged 256 GiB reserve. Cluster remains unlocked. The 45 blocked manifest entries were excluded from the batch and remain unresolved as documented above.

## Suggested Review Order

- Remove the depth cutoff while retaining credential detection and fail-closed filesystem errors.
  [`doctor.rs:37`](../../bins/mediaops/src/doctor.rs#L37)
- Preserve existing telemetry lifecycle and exporter behavior alongside the focused repair.
  [`lib.rs:73`](../../crates/telemetry/src/lib.rs#L73)
- Verify deep-tree credentials and static symlink behavior without opening linked directories.
  [`doctor.rs:200`](../../bins/mediaops/src/doctor.rs#L200)
- Prove checkout-directory scans continue to RPC and still reject credentials.
  [`cli.rs:52`](../../bins/mediaops/tests/cli.rs#L52)
