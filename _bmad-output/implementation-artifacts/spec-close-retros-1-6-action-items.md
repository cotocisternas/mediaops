---
title: 'Close Epic 1–6 retrospective action items'
type: 'feature'
created: '2026-09-02'
status: 'done'
baseline_commit: '22d2581ac2a837da5dd59a349aa0c1ee4534958d'
review_loop_iteration: 0
context:
  - '_bmad-output/implementation-artifacts/sprint-status.yaml
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Eighteen open retro items from Epics 1–6 are blocking new epics. Specs still lie (staging path, AD-15, Epic 5 FR map); bootstrap remints a split CA and fail-opens edge check; musl `mediaopsd` is unproven; sidecar/gateway/encode/`run`/holds tests have known holes.

**Approach:** Close every open item except the Epic 4 live demo (deferred: SeedIt4Me/GPU needs operator confirm). Specs match as-built; bootstrap refuses partial `tls/` and fail-closes edge check; musl build is an offline Make/CI gate; transfer/encode/apply/holds get the named tests and the small code fixes they require. Do not rewrite git history.

## Boundaries & Constraints

**Always:**
- Offline tests only. AD-2: reqwest stays a direct dep of `mediaops-arr`; CLI does not link `arr`. `ReqwestTransport` is constructed only in `mediaopsd`.
- Staging identity is `TitleId::staging_token()` hyphen form (`movie-tmdb-603`), not colon `TitleId` text.
- Complete TLS bundle is five files: `ca.pem`, `server.pem`, `server.key`, `client.pem`, `client.key`. Empty dir → mint. All five → reuse. Any other non-empty set → policy error, no write.
- Bootstrap edge check matches upgrade: Control must succeed, `!invariant_ok` is Policy, persist `EDGE_FINGERPRINT_KEY`. After UDS retries, TCP-fallback to seedbox (same as probe). Transcript tests pass `skip_edge: true`.
- Sidecar `offset.checked_add(len)` must be `Some` and `≤ file_len`; `usize::try_from(len)` before allocate; fail `TransferError::Sidecar` (no silent drop).
- `mediaops run` honors `after_install` (thread `ExecPort`); JSON `RunData` reports encode `ran` / `skipped` / `error`. ffprobe failure is error, never `Keep` / silent `Ok`. Keep/Refuse after a successful probe stay non-errors.
- Future story commits put `N-M` (hyphen, e.g. `2-1`) in the subject so `git_evidence.py --stories` can attribute.

**Ask First:**
- Anything that would SSH, pull, encode, or write nginx on the live SeedIt4Me box.
- Dropping FR4/FR10 from the requirements inventory (this pass only records they were not Epic 5).

**Never:**
- Live demo / FTP-PASV claim (deferred-work.md 2026-09-02 split).
- Rewrite published commits. Move reqwest out of `arr`. Link `arr` into the CLI. Remint+scp on AlreadyThere to "fix" a partial bundle. `make test` requiring musl/cmake. Auto-approve holds. LLM.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| TLS empty | `tls/` missing or 0 files | mint 5 files, proceed | N/A |
| TLS complete | all 5 files | reuse, do not remint | N/A |
| TLS partial | 1–4 of the 5 names | no mint, no overwrite | `BootstrapError::Policy` |
| Bootstrap edge | Control reachable, invariant ok | `applied: true`, fingerprint persisted | N/A |
| Bootstrap edge down | UDS and TCP Control fail | not applied | runtime/policy; not silent success |
| Sidecar OOB | range `offset+len > file_len` or `len` > `usize` | load/verify fail | `TransferError::Sidecar` |
| Gateway N streams | `ConfigurePool(N)` then N concurrent UDS `GetRange` held open | N WAN slots; N+1 `ResourceExhausted` | N/A |
| `run` + Nvenc | `nvenc_cap=1`, canned HEVC probe/ffmpeg | envelope encode outcome; `current_b3` changes or `ok:false` | probe/exec error not Keep |
| ffprobe fail | scan / `after_install` ExecPort error | not `Keep`; not silent skip | error / `probe_error` |
| Apply 2nd call | `Grabber::Servarr` + fake GrabOps | first `noop:false`+diff; second `noop:true` | N/A |
| Approved hold `run` | scene-named remote, exclusive flock | library PathSchema path, no leftover scene tag | N/A |

</frozen-after-approval>

## Code Map

**Docs (as-built, do not change code behavior):**
- Staging: `crates/core/src/title_id.rs:129-135` `staging_token`; `pathschema.rs:391-408` `staging_path`; example `_incoming/movie-tmdb-603/…`. Lies: `epics.md:125,270`; `ARCHITECTURE-SPINE.md:135`; also `spec-1-2-*.md:36,88,134`; `epic-1-context.md:31`; `epic-6-context.md:38`; `install.rs:33` crate doc. Not SPEC.md.
- Follow-up flags: `spec-1-1-*.md` and `spec-1-4-*.md` frontmatter + body `followup_review_recommended: true` → `false` (crates consumed by later epics).
- AD-15 vs AD-2: spine `ARCHITECTURE-SPINE.md:155-159` and `epics.md:129,579` say reqwest linked only in mediaopsd. As-built: `crates/arr/Cargo.toml:9`; `arch-tests` `lib.rs:77-81`; ctor `bins/mediaopsd/src/main.rs:275-277`. Rewrite: arr owns reqwest (AD-2); only mediaopsd constructs `ReqwestTransport`.
- FR4/FR10: `epics.md:35,47,154,160,196-198,565-567`. Delivered: grab set-diff + nginx splice/edge API. Not delivered: `paths.roots` apply, Swizzin package install. Record later; do not implement here.
- Process: no CONTRIBUTING/AGENTS. Add `N-M` subject rule to `README.md`. Add done as-built `spec-2-1`, `spec-2-2`, `spec-2-3` from Epic 2 ACs + SHAs `d2d9331`/`b76cc23`/`6e2be4c`. Do not invent spec-3/4/5 (those items were subjects-only).

**Musl:** `crates/ssh/src/lib.rs:117-133` `musl_build_command` (transcript-only). No `.cargo/config.toml`. `rust-toolchain.toml` has no `targets`. `Makefile` / `.github/workflows/ci.yml` host test only. Add musl linker pin + `make musl` + CI step (`musl-tools`, `cmake`, `protobuf-compiler`). Keep off `make test`.

**TLS remint:** `bootstrap.rs:358-368` `tls_bundle_on_disk` all-or-nothing; `:176-185` any missing → mint. Complete set listed `:370-381`. AlreadyThere does not scp certs (`ssh/lib.rs:329-331`). Tests `bootstrap.rs:783-826` cover 0 and 5 files only.

**Edge check:** install fail-open `bootstrap.rs:249-259`; upgrade fail-closed `:301-343` + `doctor::EDGE_FINGERPRINT_KEY`. Reuse upgrade loop; add `socket`/`skip_edge` on `BootstrapArgs`; TCP fallback as probe `:215-236`. Tests: `upgrade_runs_edge_check_and_persists_fingerprint` (`:1069-1104`); transcript tests get `skip_edge: true`.

**Sidecar:** `crates/transfer/src/sidecar.rs` `load` `:55-71` (version + `range_len>0` only); `pull.rs:157-177` `verify_recorded_ranges` allocates `vec![0; len as usize]` then silent-drop. Tests: `load_zero_range_len_is_sidecar_error`, `resume_skips_recorded_ranges_*`. Overlap still out of scope.

**Gateway:** `crates/net/src/gateway.rs:101-113,220-237`; pool `pool.rs:39-47`; `tcp_connect_count` `listen.rs:24-28`. Existing `configure_pool_opens_n_wan_channels_and_refuses_n_plus_one` does not hold UDS GetRange. Cite `failure-history-tests.md:46`. Loopback is the fake transport.

**Encode/`run`:** discard `bins/mediaops/src/run.rs:169-188`; `RunData` `:28-33` has no encode field. `after_install` `encode_cmd.rs:298-328`. Scan Keep-on-err `:69-71`; after_install silent Ok `:312-314`. Probe already errors `crates/encode/src/ffprobe.rs:26-53`. TITLE path already maps err `:167-169`. Reuse encode_cmd HEVC10/`Transcript` test helpers. `session_cap` 0 without `nvenc_cap` — tests must set cap.

**Apply CLI:** `apply_cmd.rs:97-127` uses `start_pair` → `Grabber::None` → always noop (`seedbox.rs:149-160`). Use `start_pair_with_grab_ops` (`test_support.rs:73-81`) + fake `GrabOps` (do not link arr). Template: `HoldGrabOps` in `run.rs:609`.

**Holds `run`:** plan Copy `crates/sync/src/plan.rs:103-153`; apply `apply.rs:84-108`. Existing `cmd_plan_approved_hold_copies_schema_path_not_scene_name` (`run.rs:537`) is plan-only. Need `cmd_run` + scene file on remote (write **before** List; `file_len==0` skipped). `nvenc_cap=0`. Approve must not install.

## Tasks & Acceptance

**Execution:**
- [x] `epics.md`, `ARCHITECTURE-SPINE.md`, `spec-1-2-*.md`, `epic-1-context.md`, `epic-6-context.md`, `crates/core/src/install.rs` -- replace `_incoming/<TitleId…>` with `_incoming/<TitleId::staging_token()>/` (hyphen example) -- spec reconciliation
- [x] `spec-1-1-*.md`, `spec-1-4-*.md` -- `followup_review_recommended: false` (frontmatter + body) -- crates consumed
- [x] `ARCHITECTURE-SPINE.md` AD-15, `epics.md` AD-15 + story 5.1 -- reqwest is arr's dep; only mediaopsd constructs `ReqwestTransport` -- match AD-2
- [x] `epics.md` Epic 5 intro + FR map -- Epic 5 = nginx splice + grab/edge API; Paths root-folder apply and Swizzin packages are later -- do not implement Paths/packages
- [x] `README.md` -- document `N-M` in commit subjects -- process lesson
- [x] `_bmad-output/implementation-artifacts/spec-2-{1,2,3}-*.md` -- status `done` as-built records from Epic 2 ACs + SHAs -- epic-2 item
- [x] `.cargo/config.toml`, `rust-toolchain.toml`, `Makefile`, `.github/workflows/ci.yml` -- musl-static `mediaopsd` offline gate (`make musl`) -- prove link
- [x] `bins/mediaops/src/bootstrap.rs` -- refuse partial tls bundle; fail-closed edge check with UDS-then-TCP and `skip_edge` -- remint + 5.3 AC
- [x] `crates/transfer/src/sidecar.rs`, `crates/transfer/src/pull.rs` -- bound-check before allocate; tests that OOB/overflow error -- sidecar item
- [x] `crates/net/src/gateway.rs` tests -- N concurrent UDS GetRange + N+1 exhausted; cite collapsing Range onto one TCP -- 3.1/AD-12
- [x] `bins/mediaops/src/run.rs`, `bins/mediaops/src/encode_cmd.rs` -- honor `after_install`; encode fields on `RunData`; ffprobe fail ≠ Keep; tests -- epic 4 items
- [x] `bins/mediaops/src/apply_cmd.rs` -- second-apply test via `start_pair_with_grab_ops` + fake GrabOps -- observe set-diff
- [x] `bins/mediaops/src/run.rs` tests -- exclusive `cmd_run` installs Approved hold onto PathSchema path without scene tags -- spec-6-2 AC
- [x] sprint-status.yaml -- via `sprint_status.py update --set-action-status` mark closed items `done`; leave epic-4 live-demo item `open` -- tracking

**Acceptance Criteria:**
- Given colon-form staging in planning docs, when the tree is grepped for `_incoming/<TitleId`, then remaining hits are historical reviews only (or the hyphen-token form).
- Given `tls/` with 4 of 5 files, when bootstrap runs, then it exits policy and the 4 files are unchanged.
- Given bootstrap install with Control up, when it succeeds, then edge fingerprint is persisted; when Control is down and `skip_edge` is false, then it does not report applied success.
- Given `make musl` on CI (or local with musl-gcc), when it runs, then `mediaopsd` links for `x86_64-unknown-linux-musl`.
- Given a sidecar range past `file_len` or a `len` that cannot fit `usize`, when load or resume runs, then it errors `Sidecar` without allocating that buffer.
- Given pool size N, when N UDS GetRange streams are held open, then the N+1th is `ResourceExhausted` and GetRange does not open extra WAN TCP.
- Given `cmd_run` with `nvenc_cap=1` and a canned HEVC probe, when install finishes, then the envelope includes encode outcome and `current_b3` changes or the command fails visibly.
- Given ffprobe ExecPort error on scan or after_install, when those paths run, then the result is not Keep and not a silent skip.
- Given Servarr GrabOps that diffs then noops, when `seedbox apply` runs twice, then first JSON `noop:false` with diff and second `noop:true`.
- Given an Approved hold with a scene-named remote file, when exclusive `run` applies, then the library path is PathSchema and contains no leftover scene tag.
- Given this pass, when sprint-status is read, then all retro action items except `epic-4-retro-item-12-*` are `done`.

## Design Notes

Bootstrap chicken-egg: first install may have no home UDS. Fail-closed means Control (UDS then seedbox TCP), not "home gateway must already be up." `skip_edge` is test-only, same as upgrade.

`RunData` encode field: `{ran: usize, skipped: usize, error: Option<String>}` (or equivalent nested object). `skipped` covers paused/`cap==0`/Keep/Refuse. Probe/exec failures are `error`, not skipped.

Partial tls: refuse, do not invent AlreadyThere cert copy.

## Verification

**Commands:**
- `make test OFFLINE=1` -- expected: default suite green (no musl, no live box)
- `cargo test --locked --offline -p mediaops-transfer -p mediaops-net -p mediaops --bins --test cli` -- expected: new sidecar/gateway/run/apply/hold tests pass
- `make musl` -- expected: `mediaopsd` links for musl (CI and any machine with `x86_64-linux-musl-gcc` + cmake)

**Manual checks (if no CLI):**
- `git_evidence.py` still cannot attribute old SHAs; README states the future-subject rule.
- Live SeedIt4Me demo was not run.

## Suggested Review Order

**Bootstrap / TLS**

- Fail-closed install edge check: UDS then seedbox TCP, then persist fingerprint.
  [`bootstrap.rs:261`](../../../bins/mediaops/src/bootstrap.rs#L261)

- Partial `tls/` is Policy; unlistable dir is Io, never remint.
  [`bootstrap.rs:366`](../../../bins/mediaops/src/bootstrap.rs#L366)

**Transfer / gateway**

- Sidecar `offset+len` and `usize` checked before any range buffer alloc.
  [`sidecar.rs:78`](../../../crates/transfer/src/sidecar.rs#L78)

- N held UDS GetRange streams; N+1 exhausted; no extra WAN TCP.
  [`gateway.rs:529`](../../../crates/net/src/gateway.rs#L529)

**Encode / run / apply / holds**

- `cmd_run` honors `after_install` and reports `{ran, skipped, error}`.
  [`run.rs:88`](../../../bins/mediaops/src/run.rs#L88)

- ffprobe failure is `probe_error`, never Keep or silent skip.
  [`encode_cmd.rs:304`](../../../bins/mediaops/src/encode_cmd.rs#L304)

- CLI second apply observes Servarr set-diff via fake GrabOps.
  [`apply_cmd.rs:176`](../../../bins/mediaops/src/apply_cmd.rs#L176)

- Exclusive `run` installs an Approved hold onto a PathSchema path.
  [`run.rs:772`](../../../bins/mediaops/src/run.rs#L772)

**Specs / musl gate**

- AD-15 matches AD-2: reqwest lives in `arr`; only mediaopsd constructs it.
  [`epics.md:129`](../../planning-artifacts/epics.md#L129)

- Musl-static `mediaopsd` pin (`musl-gcc` + `crt-static`); not part of `make test`.
  [`config.toml:4`](../../../.cargo/config.toml#L4)
