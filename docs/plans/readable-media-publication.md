---
title: Readable media publication
type: bugfix
created: '2026-09-19'
status: done
baseline_commit: 0023c4c3690563e1401ab65f5213e3808743b675
review_loop_iteration: 0
context:
  - mediaops/AGENTS.md
  - mediaops/docs/architecture.md
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Verified synced files are installed with private `0600` modes, causing Jellyfin playback permission errors despite successful Installed Jobs. Same-filesystem hard links preserve the staging mode; cross-filesystem installation explicitly creates a private copy and publishes it unchanged.

**Approach:** Make readable permissions part of successful media publication and verified recovery: final media `0644`, newly created library directories `0755`. Keep incomplete staging and control files private, and retain integrity verification and atomic no-overwrite behavior. Apply the same publication contract to completed encode replacements without changing backup permissions.

## Boundaries & Constraints

**Always:** Use the shared core install gate and PathSchema destinations. Set and sync file permissions before reporting successful installation. Verify recovery bytes against the persisted digest before permission normalization. Preserve ownership checks, private temporary directories, deadline semantics, supported music directory symlinks, and retryable recovery. Change only newly created library directories, not preexisting directory policies. Preserve private credentials and API state. Use isolated test fixtures, not real copies or encodes. Deploy only after checking active work and coordinating maintenance.

**Ask First:** Any need to broaden existing library directory permissions, change ACLs, overwrite unrelated media, or migrate an unbounded set of old installations. Existing authorization covers the focused fix, tests, local commit and safe home-side deployment.

**Never:** Recursive permission changes, world-writable files, redownloads or re-encoding as a permissions repair, seedbox deletion, automatic Hold approval, direct seedbox copy paths, or claims that readability guarantees every codec/client combination will play.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Same-device install | Verified private source | Atomic new destination has exact `0644` | Permission/fsync failures cannot report successful install |
| Cross-device install | Private owned temporary copy | Promote complete data to `0644`, sync, publish, safely clean temporary | Incomplete copy remains private and retryable |
| Recovery | Destination matches persisted digest | Normalize verified inode before proof/Installed, including expired deadline | I/O failure retains Verifying; mismatch never chmods destination |
| Collision | Existing destination or concurrent winner | Preserve its content and permissions | Existing no-overwrite refusal |
| Crash | Private or promoted owned temporary, one or two links | Clean only identity-validated owned data; preserve published link | Unsafe ownership/link shape refused |
| Directories | Missing nested schema parents, restrictive umask | Newly created parents are `0755`; existing modes remain | Propagate creation/permission errors |
| Replacement | Complete converting file plus live media | New media readable; old live/backup modes preserved | Preserve collision and rollback behavior |

</frozen-after-approval>

## Code Map

- `crates/core/src/install.rs`: `install_checked` verifies digest and creates parents; `copy_into_place_checked` hard-links or invokes `copy_across_devices`. These copy helpers also serve backups, so publication permissions must be explicit rather than unconditional.
- `crates/core/src/install.rs`: `InstallTemporary::clear_data` uses `owned_regular`, currently rejecting group/other bits. Separate strict private marker validation from accepted data modes while retaining owner, link count, destination inode and slot-identity checks.
- `crates/core/src/install.rs`: `replace` copies live bytes to backup then renames conversion over live; normalize only the completed conversion, not backup/live inode.
- `bins/mediaops-pull/src/main.rs`: `installed_matches`, both branches of `place_verified`, and `finish_verifying` govern matching-destination recovery and proof/status publication.
- `bins/mediaops-pull/src/tests.rs`: recovery tests exercise matching staged hardlinks, cross-device install, expired deadlines and API failure.
- `crates/transfer/src/pull.rs`: `ensure_staging_parent` and partial-file open need explicit private creation modes; completed staging promotion must not expose incomplete data.
- `crates/transfer/src/sidecar.rs`: `save` should explicitly create private sidecar temporaries. `crates/transfer/src/staging.rs` already makes writer locks private.
- `docs/architecture.md`: current installation and recovery guarantees; document publication modes and their limits.

## Tasks & Acceptance

**Execution:**
- [x] `crates/core/src/install.rs` — implement explicit media publication permissions, owned-temp cleanup compatibility, newly created directory modes, and replacement handling; preserve backup semantics.
- [x] `crates/transfer/src/pull.rs`, `crates/transfer/src/sidecar.rs` — ensure incomplete data and staging containers remain private without chmodding surviving published hardlinks.
- [x] `bins/mediaops-pull/src/main.rs` — normalize only proven matching recovery destinations before success using no-follow opened inode; keep failures retryable and clean owned installation temporaries in both recovery branches.
- [x] Relevant inline core/transfer tests and `bins/mediaops-pull/src/tests.rs` — cover matrix scenarios, content/digest preservation, modes, collision safety, crash cleanup and recovery. Isolate umask changes in subprocesses.
- [x] `docs/architecture.md` — document readable-publication policy versus administrator-controlled existing directories/ACLs and codec compatibility.
- [x] This plan — record actual validation and deployment evidence, limitations and review findings; finalize the focused local commit in the parent session.

**Acceptance Criteria:**
- Given a verified `0600` media source, when same- or cross-device installation succeeds, then final media is `0644` with unchanged bytes and digest.
- Given an interrupted install with a proven matching private destination, when recovery succeeds, then readable permissions precede Title proof and Installed status.
- Given incomplete transfer data or control files, when copying or resuming, then they remain private and unrelated existing media is not modified.
- Given missing schema parents and restrictive umask, when publication succeeds, then newly created directories are traversable without modifying existing directory policies.
- Given the authorized home deployment, when tests pass and maintenance is safe, then updated binaries are deployed and Home readiness checked without starting new real sync or encode work.

## Spec Change Log

- 2026-09-19: Parent reports completion of all three independent reviews and requests focused patches/regressions. Clarified conservative refusal of inaccessible preexisting schema parents (never broaden them), and that exact Unix chmod modes can change ACL masks without explicit ACL rewriting. Existing pathname replacement races remain outside full hostile-filesystem hardening scope. Local commit and deployment were deferred at that checkpoint.
- 2026-09-19: Parent reports independent final review found no blockers and explicitly authorizes safe local home deployment, preserving maintenance coordination and prohibiting new syncs, remote changes, media writes and commits. This supersedes the earlier deployment deferral. The no-commit instruction applied only to the deployment subtask; the parent session retains the user's authorization for the focused local commit.

## Design Notes

A completed staging inode may acquire publication permissions immediately before linking; private staging ancestors must continue protecting it. Cross-device data may survive a crash after mode promotion but before publication, so cleanup must recognize exact publication mode while keeping control markers strictly private. Use descriptor-based normalization to avoid hashing one pathname resolution and chmodding another. Existing terminal Jobs are not automatically migrated; the thirteen known affected files were already repaired operationally.

## Verification

- Targeted core, transfer and worker tests with `--locked`, then `make test OFFLINE=1` including sibling-binary build.
- Formatting, architecture gate, Clippy and whitespace validation; record preexisting warnings separately.
- Independent review of publication ordering, collision and recovery safety.
- Build/deploy affected home binaries only with idle-work and maintenance checks; verify service/Node health and current media modes.
- Actual Jellyfin-user probing depends on available privileges; do not equate owner-user ffprobe success with confirmed Jellyfin playback.

### Implementation and validation evidence (2026-09-19)

- Media publication now explicitly promotes and syncs completed inodes to `0644`; backup copying retains its previous behavior (same-device hardlink preserves mode, cross-device backup copy remains private). Replacement refuses a multiply linked conversion rather than changing an old live/backup inode through an alias.
- Only newly created library parents are normalized to `0755`. Existing directory modes and supported music-directory symlinks are retained. Fresh install, replacement and verified worker recovery conservatively require traversal bits for owner, group and other on every schema parent beneath the library root. Restrictive parents are refused without chmod, including crash-left `0700` parents. This mode-bit gate may also refuse intentional group-only/ACL-managed layouts; it is not a service-identity access check.
- Recovery hashes and normalizes one no-follow opened descriptor before proof/Installed, and both matching-destination recovery branches clean owned installation temporaries. Permission and sync errors in recovery remain retryable.
- Staging containers are owned, no-follow opened private directories. Partial files and sidecar temporaries are created privately; resumed mutable files must be owned single-link regular files before changing permissions or truncating. Completed staged/published hardlinks are not made private again.
- Passed targeted core, transfer and worker tests with `--locked --offline` and `MEDIAOPS_TEST_INSTALL_FS=/dev/shm`, including actual cross-device fixtures.
- Passed `NO_COLOR=1 MEDIAOPS_TEST_INSTALL_FS=/dev/shm make test OFFLINE=1`, including the required workspace sibling-binary build. Tests use isolated/canned fixtures, not real syncs or encodes.
- Automatic umask coverage now lives in `crates/transfer/tests/publication_umask.rs`, a separate integration-test executable with one parent test. It launches and asserts success of one isolated child each for `077` and `000`; both check private partial/staging/control modes, readable publication, new schema parents and unchanged library-root policy. No test changes the parallel unit-test runner's umask.
- Passed `make fmt-check`, `make test-arch OFFLINE=1` (17 tests), `make clippy OFFLINE=1`, and `git diff --check`. Clippy reports existing warnings in unchanged code, including CLI test mutex guards held across await; this is not a zero-warning claim.
- Initial sandboxed transfer tests could not bind Unix sockets; rerunning with approved unsandboxed test access passed. Initial full-suite exact-screen tests included terminal color; rerunning with `NO_COLOR=1` passed without changing those tests.
- An initial umask subprocess spawned inside the parallel core suite transiently inherited unrelated test locks and caused failures. That approach was removed. The separate integration-test executable avoids concurrent unit-test lock inheritance and makes both umask variants part of ordinary `make test`.

### Matrix coverage

| Row | Executed passing coverage |
| --- | --- |
| Same-device | `install_writes_only_schema_library_path_from_staging_path`, `permission_failure_is_not_success` |
| Cross-device | `music_symlink_install_crosses_filesystems_without_overwriting`, `cross_device_copy_expiry_leaves_one_owned_recoverable_partial` |
| Recovery | `matching_destination_cleans_staged_hardlink`, `matching_destination_cleans_cross_device_source`, `installation_recovers_after_title_or_job_write_failure_even_after_deadline`, `destination_hash_io_error_keeps_verifying`, `mismatching_destination_preserves_staging` |
| Collision | `concurrent_no_replace_gate_has_one_winner_and_preserves_loser`, `cross_device_collision_preserves_destination_and_backup_copy_stays_private`, existing destination/symlink refusal tests |
| Crash | `promoted_temporary_cleanup_validates_modes_links_and_marker`, `published_temp_cleanup_preserves_the_installed_hard_link`, `job_cleanup_recovers_interrupted_copy_without_removing_unknown_files` |
| Directories | `publication_directories_ignore_umask_only_when_new`, music symlink test |
| Replacement | `replace_moves_live_file_to_backup_and_writes_schema_path`, `replace_refuses_existing_backup_and_restores_on_converting_rename_failure` |
| Private staging/control | `pull_writes_staging_and_hashes`, resume tests, `save_keeps_control_private_and_refuses_a_shared_temporary` |

### Independent review patches and final verification (2026-09-19)

Parent reports all three independent reviews complete. Applied the requested focused patches:

- Fresh staging must have exactly one link before publication chmod. `publication_refuses_shared_staging_without_changing_its_alias` checks refusal and unchanged bytes/modes. Recognized published-hardlink recovery remains the worker path.
- Mutable partial and sidecar temporary opens use `O_NONBLOCK`. `partial_fifo_without_reader_is_refused_without_blocking` and `save_refuses_fifo_temporary_without_blocking` exercise readerless FIFOs with bounded test waits.
- `partial_hardlink_is_refused_without_chmod_or_truncation` and `replacement_refuses_conversion_hardlink_without_changing_live_or_alias` assert unchanged content/modes and no publication/backup on refusal.
- The existing replacement rollback test now uses a valid complete conversion on `/dev/shm`: open/chmod and backup creation succeed, the actual rename fails with `CrossesDevices`, backup cleanup restores the original link count, and live bytes/mode remain unchanged. It no longer substitutes an early missing-source failure for rename rollback coverage.
- `recovery_permission_and_fsync_failures_keep_verifying_without_proof` injects one-shot thread-local failures at the actual worker chmod and fsync boundaries, after digest verification. Both retain Verifying, staging and bytes, publish no Title proof, and complete on retry even with an expired deadline. These are deterministic failure injections, not real device-failure tests.
- `interrupted_private_schema_parent_refuses_install_without_broadening` exercises a crash-equivalent private intermediate schema parent beneath an accessible leaf. `recovery_refuses_private_schema_parent_without_broadening_or_proof` exercises the equivalent matching-destination recovery refusal.
- Corrected architecture documentation: Unix modes affect ACL masks/effective permissions; no explicit ACL rewriting or preservation of effective ACLs is promised.
- Removed the three generated fallback review prompts, as requested by the parent.

Final commands and exact results:

| Gate | Result |
| --- | --- |
| `NO_COLOR=1 MEDIAOPS_TEST_INSTALL_FS=/dev/shm cargo test -p mediaops-core -p mediaops-transfer -p mediaops-pull --locked --offline` | **231 passed**: core 182, transfer unit 33, worker 15, umask integration parent 1; 0 failed/ignored. The integration parent additionally runs and checks two single-test children. |
| `NO_COLOR=1 MEDIAOPS_TEST_INSTALL_FS=/dev/shm make test OFFLINE=1` | Required workspace build succeeded; **908 passed, 0 failed, 0 ignored, 0 measured, 0 filtered**, summing Cargo harness summaries. The two captured umask child runs are additional to this total. Actual cross-device fixtures were enabled. |
| `make test-arch OFFLINE=1` | **17 passed, 0 failed, 0 ignored** (also included in the workspace total above). |
| `make fmt-check` | Passed. |
| `make clippy OFFLINE=1` | Passed with preexisting warnings in unchanged code/lines, including core statfs casts, derivable default, redundant conversions/closures, and test mutex guards held across await. No warning originates in the review patches. |
| `git diff --check` | Passed. |

Local verification logs are `target/readable-publication-test.log` and `target/readable-publication-clippy.log` (generated build artifacts, not source deliverables).

### Authorized home deployment — 2026-09-19 21:25:40 UTC

Independent parent final review reported no blockers. Read this plan, `restore-home-sync.md`, current operator configuration/usage docs, CLI flock/pause/apply/doctor code, supervisor shutdown code, and the Makefile before deployment. No implementation source changed during this deployment session.

**Build:** Passed `cargo build --release --locked --offline -p mediaops -p mediaops-home -p mediaops-api -p mediaops-scheduler -p mediaops-gateway -p mediaops-inventory -p mediaops-pull`. The supervisor was already up to date and its release hash did not change. `make install` was deliberately not used because it also installs the daemon and TUI. The prior 908-test final validation above was not rerun during this operational-only deployment; the offline release build and live checks below were executed in this session. Initial sandboxed runtime checks were denied access to local Unix sockets/systemd; authorized unsandboxed execution supplied that access. No root escalation was used.

**Coordination and restoration:**

- Held the existing exclusive nonblocking flock on `/home/coto/.local/state/mediaops/mediaops.lock` continuously through pause, stop, replacement, restart, verification and restoration. Preserved its inode (`49160789`), mode and original metadata bytes; temporarily recorded deployment PID `1755028` while held. Restored the original metadata before releasing. A separate final nonblocking probe acquired and released the flock successfully: no deployment lock remains held. Restored holder text is historical metadata, not evidence of a live holder.
- Saved the original runtime Cluster `home` (`uid=cluster-29`, generation `7`, resourceVersion `5213541`, `spec.lock=false`). Updated only `spec.lock` to true using that exact current version; received resourceVersion `5290078`, generation `8`. No conflict retry or forced update occurred.
- Checked all Jobs before pause, after pause, immediately before stop, after restart and after restoration: **13 Installed, zero nonterminal** (including zero unbound Pending). Also checked **zero Wants, zero approved Holds**, and the existing single Sync with planning complete. No Sync was created or resubmitted.
- Verified the paused Cluster UID/spec/generation remained ours before stopping and before restoration. After restart, restored only our `true -> false` using current resourceVersion `5290078`; apply returned `5290144`, and the subsequent observed Cluster was `5290145`, generation `9`. Full final spec equals the original, including root `/mnt/storage/videos`, 256 GiB reserve, 80 GiB copy budget, 32 MiB Range length, concurrency 8, grabber, roots and encode-pause setting. No intervening spec/generation conflict occurred.
- Stopped `mediaops-home.service` only after idle checks; confirmed inactive state, PID zero and an empty/removed service cgroup before deployment. Staged and fsynced each replacement on the destination filesystem, then atomically renamed each binary into place while the service was stopped; this is per-file atomic replacement, not a single multi-file filesystem transaction. Prior binary copies were retained in the local generated evidence directory for recovery. Restarted only after all seven replacements and installed-hash checks succeeded.

**Exact deployed binaries:** all under `/home/coto/.cargo/bin/`, mode `0755`. Installed SHA-256 matches `target/release/` for every row:

| Binary | SHA-256 |
| --- | --- |
| `mediaops` | `6ccfae44c21d2d4a835475bb86f9b76672b375797e5e193018ae42acb292c9c8` |
| `mediaops-home` | `3f33c361a116cc08a039350434b730d693fa69b2f0f0f7d26127cd941dcfc192` |
| `mediaops-api` | `1604c6dfa6e327182a7f4a63c87ccb5739f6b9b3804fffd7b1b0b35fe3bdc9eb` |
| `mediaops-scheduler` | `31913b9c3b7bd8b37f7f7bac50306d20d6ac327f6b1c983428cb08cf9ea39e45` |
| `mediaops-gateway` | `e0985c026b63447db41789dc7e9f6e68e61ec7d998aaee75f41348a3ce994e61` |
| `mediaops-inventory` | `f2927b3e672880f25ca1c1fad037187958eb2278c25c411d76f0a45823ca3ee4` |
| `mediaops-pull` | `1d92f12a5fe605c0895bda544737b493017c386afa6deb92fb516998c91762d7` |

**Runtime checks:** service active/running, main PID `1755541`, `NRestarts=0`. Verified `/proc/PID/exe` hashes for the supervisor plus all five live roles against release builds: API `1755593`, scheduler `1755594`, gateway `1755595`, inventory `1755596`, pull `1755597`. All three Nodes were Ready with heartbeats after restart; the separate final check found heartbeat ages inventory 0 seconds, pull/scheduler 8 seconds. Deployed `mediaops doctor -o json` from the checkout exited 0: read-only, invariant true, frozen false, empty drift, all six key-presence booleans true. Doctor used the existing Home gateway; no key contents were printed or stored.

**Known repaired media:** matched all 13 queued entries of existing Sync `restore-home-sync-20260919` to Jobs by both name and UID. Before and after deployment, every destination was a regular non-symlink file with no symlink path components, exact Job size and exact `0644` mode. Total **38,195,638,679 bytes**. Device/inode, size, mtime and ctime were unchanged across the deployment; Job objects also remained unchanged. No chmod, content write or independent full rehash was performed.

| Previously repaired media | Verified files |
| --- | --- |
| The End of Oak Street (2026) | Movie: 1 |
| It's Always Sunny in Philadelphia (2005) | S18E05, S18E06: 2 |
| Lanterns (2026) | S01E04, S01E05: 2 |
| Lioness (2023) | S03E06, S03E07: 2 |
| Reacher (2022) | S04E07, S04E08: 2 |
| Star Trek: Strange New Worlds (2022) | S04E08, S04E09: 2 |
| Ted Lasso (2020) | S04E06, S04E07: 2 |

Generated local evidence and recovery copies are under `target/readable-publication-deployment/`: `report.json`, original/paused/final Cluster snapshots, original lock metadata, before/after media metadata, doctor output, deployment script and prior binary copies. These are ignored build artifacts, not committed deliverables. No PEMs or credential contents were copied into the worktree. `git diff --check` passed; no commit, branch or index write was made.

### Remaining limits

- Deployment and local readiness are verified, but no new real copy or encode was initiated to exercise publication live. Functional publication/recovery coverage is the isolated test evidence above. Existing terminal Jobs are not migrated.
- No Jellyfin-user read probe or playback verification was performed; no password prompt or privileged bypass was attempted. Mode checks are not proof of service-identity access, full content integrity, ACL behavior or client/codec compatibility. Library-root ancestor restrictions and service identity remain administrator concerns. Exact modes can alter ACL masks; effective ACL preservation is not guaranteed.
- The 45 preexisting blocked Sync entries documented in `restore-home-sync.md` remain outside this deployment; no conflict repair, Hold approval, reindex, reclaim, remote change, SSH, encoding, media overwrite or broad permission change was performed. Remote daemon and local TUI binaries were not deployed.
- Preexisting pathname replacement races are not claimed to be full hostile-filesystem hardening; this remains outside the requested scope.

### PR #16 review follow-up — 2026-09-19

Implemented all three approved findings; the frozen approved intent above is unchanged.

- `bins/mediaops/src/doctor.rs`: the iterative PEM scanner now tracks visited directory `(dev, ino)` identities before reading entries. Child symlinks remain excluded and metadata/read-directory/entry/type failures remain policy errors with path context. `pem_scan_visits_duplicate_directory_identities_once` seeds overlapping pending directories and a `child/.` alias, then asserts exactly one credential hit. This is a nonprivileged duplicate-identity test seam, not a reproduced bind-mount infinite traversal; no mounts were performed. Existing deep-tree, symlink and fail-closed missing-root tests still pass. Hostile concurrent pathname replacement remains outside the guarantee.
- `crates/core/src/install.rs`: replaced recursive absolute-path parent creation with a component-wise walk from an existing directory `library_root`. The root must exist and be a directory; its permission policy is not broadened or subjected to the schema traversal-bit requirement. Each schema ancestor is checked before creating children, using the same traversal validation as recovery/replacement. Only directories created by this call receive `0755`; existing modes and supported music-directory symlinks are preserved. Digest verification, no-overwrite publication and recovery ordering are unchanged.
- Added `private_schema_ancestor_refuses_install_before_creating_children` and `missing_library_root_refuses_install_without_creating_it`. They assert refusal, no child/root creation, and unchanged private staging bytes/mode. Existing private intermediate-parent, music-symlink/cross-device, collision and recovery coverage remains passing.
- Corrected the comment in `publication_directories_ignore_umask_only_when_new` to point to `crates/transfer/tests/publication_umask.rs`. The integration test still checks separate child processes with umasks `077` and `000`; the parallel core harness does not change umask.

Validation performed in this follow-up:

| Command | Result |
| --- | --- |
| `NO_COLOR=1 MEDIAOPS_TEST_INSTALL_FS=/dev/shm cargo test -p mediaops-core --locked --offline install::tests` | 34 passed; actual cross-device fixtures enabled. |
| `NO_COLOR=1 cargo test -p mediaops --locked --offline pem_scan` | 4 PEM scanner tests passed. |
| `NO_COLOR=1 cargo test -p mediaops-transfer --locked --offline --test publication_umask` | 1 parent test passed, asserting both isolated umask children succeed. |
| `NO_COLOR=1 MEDIAOPS_TEST_INSTALL_FS=/dev/shm make test OFFLINE=1` | Required sibling workspace build succeeded; **911 passed, 0 failed, 0 ignored**, summing Cargo harness summaries (captured umask children additional). |
| `make test-arch OFFLINE=1` | 17 passed. |
| `make fmt-check` | Passed after `make fmt`. |
| `make clippy OFFLINE=1` | Passed with warnings in existing code, including statfs casts and test mutex guards held across await; no warning originates in added code. |
| `git diff --check` | Passed. |

The initial sandboxed full test attempt failed on local Unix-socket bind denials (`Operation not permitted`); the same gate passed with approved unsandboxed test execution. Logs are ignored build artifacts: `target/pr16-review-test.log`, `target/pr16-review-test-unsandboxed.log`, `target/pr16-review-arch.log`, and `target/pr16-review-clippy.log`. No deploy, live-box feature, real encode, remote operation, Git commit/push, or GitHub reply was performed. Parent owns review, commit/push and subsequent hosted CI verification; this evidence is local validation, not a claim that updated hosted CI has run.

## Suggested Review Order

**Publication and recovery**

- Verify completed media permissions before atomic publication, retaining no-overwrite protection.
  [`install.rs:393`](../../crates/core/src/install.rs#L393)
- Recover readable permissions only after validating the persisted digest on the opened inode.
  [`main.rs:451`](../../bins/mediaops-pull/src/main.rs#L451)
- Preserve temporary ownership checks and refuse inaccessible existing schema parents.
  [`install.rs:677`](../../crates/core/src/install.rs#L677)

**Privacy and regression coverage**

- Keep mutable staging private and refuse shared or nonregular inodes before writes.
  [`pull.rs:171`](../../crates/transfer/src/pull.rs#L171)
- Exercise permissive and restrictive umasks in isolated subprocesses.
  [`publication_umask.rs:1`](../../crates/transfer/tests/publication_umask.rs#L1)
- Document publication guarantees and administrator-controlled access limitations.
  [`architecture.md:96`](../architecture.md#L96)
