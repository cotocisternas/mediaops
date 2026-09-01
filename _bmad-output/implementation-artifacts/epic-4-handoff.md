# Epic 4 handoff — Tonight-playable

**Written:** 2026-08-31  
**For:** a fresh implementation session (this file is the spec; do not re-research)  
**Repo:** `/home/coto/work/dev/mediaops`  
**HEAD at handoff:** `5813ab1` on `main` (pushed to `github/main`)  
**Sprint:** epics 1–3 `done`; `4-1` / `4-2` / `4-3` `backlog`

**Canonical contract:** `_bmad-output/specs/spec-mediaops/SPEC.md` + `increments.md` + `_bmad-output/planning-artifacts/architecture/architecture-mediaops-2026-08-29/ARCHITECTURE-SPINE.md` + `_bmad-output/planning-artifacts/epics.md` (Epic 4 only)

**Do not treat as current:** `product-idea.md` (rclone/rsync/rsync-ssh superseded by SPEC), `implementation-readiness.md` (still says next is story 1.1)

---

## Prompt for the fresh session

> Implement Epic 4 from `_bmad-output/implementation-artifacts/epic-4-handoff.md`. Follow it as the spec. Do not re-open architecture. Work 4.1 then 4.2 then 4.3. Offline tests only until 4.3; then write the runbook and **do not touch the live SeedIt4Me box or GPU until I confirm**.

Read, in this order:

1. This file
2. `epics.md` Epic 4 acceptance criteria
3. `deferred-work.md` — NVENC 0/1, no home-daemon unit, musl unproven, GetRange not streaming. Do not “fix” those except where this handoff says to.

Verify HEAD still has Epic 3 (`library bootstrap` / `pull` / home gateway exist; `bins/mediaops/src/run.rs` still stubs with “plan/apply waits for Epic 4”). If the tree has moved, rebase onto current `main` and **keep the locked decisions below**.

Do **not** invoke `bmad-build` unless the operator asks. That skill is a conversational story loop and will burn context. Follow the spine and ACs here instead.

Branch: `epic/4-tonight-playable` from `main`. Three sequential commits (same cadence as 1.x / Epic 2 / Epic 3). One PR at the end is fine.

---

## Current code (what you inherit)

Workspace is the architecture structural seed. Epics 1–3 are in the tree.

| Already real | Still stub / missing |
| --- | --- |
| TitleId, PathSchema, walker, install/replace gate | Planner, apply loop |
| DesiredState TOML, Plan artifact with **unit** `Action`s | `plan` / `watch` / `why` / `status` CLI |
| Jobs state machines + sqlite (`probes`, `title_index`, `jobs`, `machine` kv, user_version 4) | `JobsRepo` has get/create/advance only — no list |
| proto `mediaops.v1` Control+Transfer+Gateway | Seedbox Control RPCs other than Df/GrabApply-noop error “not this epic” |
| net mTLS, channel pool, home UDS gateway | No systemd unit for `mediaopsd --role home` |
| `seedbox bootstrap`, `library bootstrap`, `list`, `pull` | `run` takes flock then exits 5 |
| transfer PullFile + `.partial` + BLAKE3 | `arr` crate empty; encode = ffmpeg-encoders 0/1 probe |
| arch-tests AD-2 | No `live-box` feature, empty `fixtures/`, no README |

CLI today: `seedbox bootstrap`, `library bootstrap`, `list`, `pull`, `run` (stub). mediaopsd: `serve --role seedbox|home`; `reverse-connect` unused.

Out of this epic: arr HTTP, holds inbox, reclaim, doctor/repair, `docs render`, relocate, TUI, agents, GrabApply/EdgeApply apply.

---

## What “done” means

First demo on this box (`increments.md`), `grabber=None`:

1. Seedbox `mediaopsd` binds gRPC/mTLS; home `mediaopsd` on UDS.
2. `watch TITLE` records a want and exits 0.
3. `plan` / `run` Copy/Skip under budgets (music-first), then install through PathSchema.
4. Parallel Range pull with `.partial` resume and whole-file BLAKE3 (already in Epic 3; apply must use it).
5. Encode at least one HEVC-MP4 movie to H.264 8-bit under probed NVENC; HDR/DV/2160p remux refused.
6. `why` / `status` show pull, watermark, lock, encode-queue with local FS as truth.
7. Default CI still offline. Live steps behind cargo feature + env. 4.3 produces a runbook and **stops before anything destructive**.

---

## Locked decisions (do not re-litigate)

These close gaps the spine left for Epic 4. Inventing different shapes will fight AD-9/AD-10 and the existing unit `Action` enum.

### D1 — `Action` grows payloads (AD-9)

Story 1.3 shipped **unit** variants. Apply cannot run without title/remote/placement. No production plan files exist.

Change `core::plan::Action` to an internally tagged enum (`#[serde(tag = "type", rename_all = "snake_case")]`). Keep the same nine variants. Match exhaustively; no `_` arm.

```text
Copy    { title_id, remote: {root_id, rel_path}, file_len, placement }
Skip    { title_id?, reason }          // title_id optional for unindexed budget skips
Encode  { title_id }                   // emitted in 4.2; 4.1 may serialize but must not apply
Review / Unmonitor / DeleteRemote / Reclaim / EdgeApply / GrabApply
        unit or minimal placeholders — present on the enum, not applied in Epic 4
```

Update every 1.3 plan JSON test. `RemoteRef` / `Placement` need serde (add, don’t fork plan-only clones).

### D2 — Pure planner in `sync`; apply in `sync`; CLI is the flock holder (AD-4, AD-9, AD-10)

```text
plan(listings, title_index snapshot, open wants, DesiredState, free_bytes) -> Plan
apply(plan, ports) in the same locked CLI process
```

- Snapshot desired-state **bytes** at plan start; embed them + blake3 (already on `Plan`).
- Apply re-parses **only** from embedded bytes; refuse if `!plan.matches_snapshot(active_file_bytes)` (exit 4).
- `run` = plan + write JSON under `~/.local/state/mediaops/plans/` + apply that artifact + delete the plan file on successful apply completion (AD-6).
- `plan` writes the artifact and prints it; does not apply.
- Add workspace edge **already allowed:** `mediaops-sync` → `mediaops-transfer`. Do not add `sync` → `net` (CLI reaches net through transfer, as today).

### D3 — Grabber=None matching = PathSchema parse of remote `rel_path`

Walker `rel_path` is relative to an allowlisted root. The demo root **must be a schema tree** (`movies|series|music/...`).

- File entries whose `pathschema::parse` returns a `TitleId` are candidates.
- Add `pathschema::parse_placement(path) -> Result<(TitleId, Placement), PathSchemaError>` that requires a file component (folder-only paths are not Copy). Reuse the year/stem logic already in `parse`.
- Unparseable remotes are **omitted** (not Review-apply, not a second path renderer).
- Title already in `title_index` → `Skip { reason: "upgrade_never" }` even if remote exists.
- Open `Want` jobs do not invent a remote; they only prioritize a matching candidate and show up in `why` if unmatched.

Sort Copy: `album` first, then `movie`, then `series`. Within kind: titles with an open Want before others. Then listing order.

Budget: walk that order; if the next Copy would exceed `max_copy` **or** leave free space below `min_free`, that item and the rest of that class become `Skip { reason: "watermark" | "max_copy" }`. Preflight fail of `run` when **zero** Copies fit and at least one candidate existed only if the **first** candidate alone would breach — otherwise Skip is data, `run` exits 0 (AD-17).

Upgrade class is the constant **never**. Do **not** add a desired-state field this epic (`deny_unknown_fields` would break existing TOML). Named test must cite “auto-upgrade 1080p → 4k remux because disk is bored”.

### D4 — Jobs repo can list; encode parent becomes optional

Today `JobsRepo` is get/create/advance only. Add:

- `list(&self) -> Vec<Job>`
- `list_by_title(&self, &TitleId) -> Vec<Job>`

Prefer **schema v5 `path TEXT NOT NULL` on `title_index`** — SPEC maps TitleId → path → digest; AD-8 omitted path. Empty DB migrate; v4 rows without path: treat as missing and fall back to walking `movies/series/music` + `parse` once.

**Encode parent:** `Job::new` currently requires Encode → Pull parent. Already-local encode (no Copy) cannot do that honestly. Change Encode parent to **optional**. `encode_ready(job, parent)` is true when Encode is `Queued` and either:

- parent is a Pull in `Installed`, or
- parent is `None` and the title has a `title_index` row (local file exists).

Update the “encode needs parent” tests. Pull may still omit parent (already allowed) but apply-created Pulls from a Want should set `parent_job_id`.

### D5 — Lock classes (AD-4) — enforce in CLI

| Exclusive (flock, exit 3 on conflict) | Lock-free (no flock; single-row writes only) |
| --- | --- |
| `plan`, `run`, `pull`, `library bootstrap`, `seedbox bootstrap`, `encode run` (4.2) | `watch`, `why`, `status`, `encode pause` (4.2), `list` |

Replace `run_stub`. Keep writing pid / started_at / command into the lockfile (`bootstrap::exclusive_lock` already does). `status` reads that file if present and flock would block.

### D6 — Home gateway systemd unit (pick up 3.3 deferral)

`run` talks only to UDS. Timer cannot succeed if the operator forgot to start `mediaopsd --role home`.

In 4.1, `library bootstrap` also writes `mediaopsd-home.service` (simple, `Restart=on-failure`, ExecStart = `mediaopsd serve --role home --tls-dir … --desired-state …`). Do not enable it unless `--enable-timer` (same flag as today’s run timer). Document in 4.3 runbook.

Do **not** invent a seedbox address in the CLI. Home daemon still owns `--upstream` / desired-state `seedbox_address`.

### D7 — Encode policy lives in `encode`

4.2 does **not** add EncodePolicy fields to desired-state TOML (`deny_unknown_fields`). Hardcode the v1 matrix in `encode` as named rules, tested:

| Rule | Result |
| --- | --- |
| Movie + HEVC + 10-bit + MP4 + not HDR | `NvencH264` (target H.264 8-bit) — “HEVC-MP4 Chrome dropped frames” |
| Series + HEVC + MP4 | `Keep` — explicit **series-skip** named rule |
| HDR or Dolby Vision | `Refuse` / Keep-forever |
| `height >= 2160` | `Refuse` |
| H.264 8-bit already | `Keep` |
| Anything else v1 | `Keep` unless the movie HEVC-MP4 rule matches |

Classification input is a small `ProbeMedia` struct (codec, depth, width, height, container, hdr flag) filled by `ffprobe` JSON via `ExecPort`. Unit tests pass a struct; they never run ffmpeg. Transcript tests cover the ffprobe argv.

Write `.converting` under `_incoming/<staging_token>/` (handle only checks the **filename** is `<dest>.converting`). Then `replace(library_root, title_id, handle, backup)` with backup `{library_root}/_ops/backup-hevc-originals/<staging_token>/<filename>`.

**Fix in 4.2:** `replace` today rejects backup if `backup.starts_with(library_root)` — too broad because `_ops` is under the library root. Change to “not under a schema library dir (`movies`/`series`/`music`)”. Tests: `_ops/backup-hevc-originals/...` legal; `movies/...` not. Original is never deleted before replace succeeds (already in `replace`).

NVENC concurrency: keep probe as ffmpeg-encoders presence (0/1) per deferred-work. Session cap = `min(desired_state.max_nvenc, max(stored nvenc_cap, 1) if hevc else 0)`. If cap is 0, Encode refusal is exit 5 only for the **`encode run FILE`** verb; inside `run` it is data (AD-17). Pause = `machine` kv `encode_pause=1` polled between jobs, never a signal to the lock holder.

ffmpeg via `ExecPort` only. No `ffmpeg-next`. Seedbox/mediaopsd still must not link `encode` (arch-tests).

### D8 — AD-20 live feature (4.3)

Add cargo feature `live-box` on `bins/mediaops` (tests gated `#[cfg(feature = "live-box")]`). Tests also require `MEDIAOPS_LIVE=1`. Default `cargo test` never enables it. No network in default tests.

4.3 **implements the runbook and the gate**. It does **not** run against SeedIt4Me or the GPU until the operator says yes. Surface destructive steps: `seedbox bootstrap --yes` (scp, systemd enable, cert mint), pull of real bytes, encode replacing a library file.

### D9 — Leave these deferred alone

- musl-static aws-lc (2.3)
- GetRange streaming disk pipe (2.2)
- Walker/prune depth caps (1.2 / 3.2)
- Path `NAME_MAX` (1.2)
- `ControlPort::df` dropping semver (trait shape)
- Comment-preserving TOML splice
- Full OpenSSH `Include` / `Host *`

---

## Story 4.1 — sync plan/apply and CLI verbs

**Commit message:** `Add plan/run/watch/why/status and the grabber=None planner.`

### Code map

| Area | Work |
| --- | --- |
| `crates/core/src/plan.rs` | Payload `Action`; keep digest/snapshot API |
| `crates/core/src/pathschema.rs` | `parse_placement` |
| `crates/core/src/jobs.rs` | `JobsRepo::list` / `list_by_title`; Encode parent optional + `encode_ready` (land here so store migrates once) |
| `crates/core/src/walker.rs` | serde on `RemoteRef` if needed |
| `crates/store` | Implement list*; schema v5 `title_index.path` |
| `crates/sync` | New modules: `plan.rs` (pure), `apply.rs` (orchestration). Keep layout/systemd helpers. Depend on `mediaops-transfer` |
| `bins/mediaops/src/main.rs` | Subcommands `plan`, `run`, `watch`, `why`, `status`. Keep `list`/`pull` |
| `bins/mediaops/src/run.rs` | Real run; delete stub message |
| `bins/mediaops/src/library.rs` | Write `mediaopsd-home.service` |
| CLI tests | envelopes, lock exit 3, watch lock-free, plan watermark skips |

### CLI contracts

All `--json` → single `{ok,data,error}` envelope. Stderr tracing.

- `mediaops watch <TITLE>` — `TITLE` is `kind:source:id`. Insert Want `Open` if none open for that title. Exit 0. Do not wait for playable. Grabber monitoring: no-op while `Grabber::None`.
- `mediaops plan` — exclusive lock, list via UDS (`transfer::list_entries`), snapshot TOML, pure plan, write `~/.local/state/mediaops/plans/<utc>-<b3prefix>.json`, print actions.
- `mediaops run` — exclusive lock, plan+apply in-process. ConfigurePool from `probes` (same as `pull` today). For each Copy: create Pull job (parent = open Want if any), `Start`, `pull_file`, `FinishRanges`, `VerifiedStagingHandle` + `install` + `record_install` + `Install`. Resume: if job is already `Pulling`/`Verifying` or `.partial` exists, continue (Epic 3 sidecar). Skip/budget is data.
- `mediaops why <TITLE>` — lock-free. Chain: want state, title-index / library parse, pull job, watermark (free vs min_free), lock holder if any, encode job (queued empty until 4.2). Grab/hold/reclaim absent is OK.
- `mediaops status` — lock-free. Lock holder JSON, open wants, in-flight jobs, last plan name if a file remains.

Talk only to home UDS. Reuse `home.rs` connect helpers; do not duplicate seedbox dial.

`pull` remains for manual Epic 3 use. Apply should call `pull_file`, not shell out to the `pull` subcommand.

### Tests (offline)

- Planner: music-first order; installed → skip upgrade_never; watermark skip; max_copy skip; unparseable omitted; want prioritizes matching title.
- Apply: hash mismatch refuses; lock conflict exit 3; install goes through PathSchema (spaces/scene tags fail); `.partial` resume still works when apply is killed mid-file (fake `RangeSource`).
- Watch/why/status JSON envelopes.
- Timer unit still has `OnUnitInactiveSec` and no `OnCalendar`.
- Named failures: watermark breach, lock conflict silent 0, auto-upgrade never.

### Not in 4.1

Encode apply, `encode` CLI, live box, Review apply. Fix album `--install` on manual `pull` if `parse_placement` makes album Placement available — that unblocks D3 for music-first.

---

## Story 4.2 — encode

**Commit message:** `Execute EncodePolicy on the home GPU with reversible replace.`

Depends on 4.1 (`encode_ready`, jobs list, apply loop).

### Code map

| Area | Work |
| --- | --- |
| `crates/encode` | `policy.rs`, existing probe, `run.rs` (ffmpeg via ExecPort), `ffprobe.rs` |
| `crates/core/src/install.rs` | Backup path may live under `_ops/` |
| `crates/sync/src/apply.rs` | After Pull `Installed`, ffprobe; if NvencH264 create Encode job (parent = that Pull) and run when ready. Honor pause + `min(max_nvenc, cap)` |
| `bins/mediaops` | `encode scan`, `encode run`, `encode pause` |
| tests | Named failures: HEVC-MP4 Chrome, HDR refuse, delete-before-replace |

**Do not ffprobe the seedbox.** Encode decision is home-side after install (or already-local). Plan artifact is not a perfect encode preview until after pull. First demo: pull HEVC-MP4 → install → encode. Apply-after-install is enough.

- `encode scan` — classify files under `movies/` (report series-skip). JSON. Lock-free.
- `encode run [TITLE]` — exclusive. One title or drain ready encode jobs.
- `encode pause` / `encode pause --off` — lock-free machine kv.

ffmpeg argv (v1), conservative: `-c:v h264_nvenc -pix_fmt yuv420p -c:a copy`. Tests assert argv via `TranscriptExec` (`crates/ssh` already has this double; reuse or put a test double next to encode), not pixels.

### Tests

- Policy table: movie HEVC10 mp4 → NvencH264; series HEVC mp4 → Keep; hdr/dv → Refuse; 2160p → Refuse.
- Failed ffmpeg leaves the original in place (no `replace` called).
- Pause: executor skips starting the next encode when flag set.
- mediaopsd still does not depend on encode (arch-tests).
- No GPU in default tests.

---

## Story 4.3 — First demo on this box

**Commit message:** `Add the first-demo runbook and the live-box test gate.`

### Deliverables

1. Feature `live-box` + env `MEDIAOPS_LIVE` as D8.
2. Runbook: `_bmad-output/implementation-artifacts/demo-epic-4.md` covering:

   - Prerequisites: `~/.ssh/config` Host `seedbox`, desired-state at `~/.config/mediaops/desired-state.toml`, home disk, Ada GPU, `grabber=None`.
   - **Destructive / live list (must be confirmed):** `seedbox bootstrap --yes` (musl build, scp, systemd --user enable mediaopsd, cert mint — refuse if config dir is a git work tree), Range probe, pull of real bytes, encode replace + backup under `_ops`.
   - Start home gateway (systemd unit from 4.1 or manual `mediaopsd serve --role home …`).
   - `library bootstrap --library-root …` if needed.
   - Place **one schema-valid HEVC-MP4 movie** on an allowlisted seedbox root (folder on the box; not torrent save paths).
   - `watch movie:tmdb:<id>`
   - `plan --json` then `run --json`
   - Kill at ~90%, `run` again (resume from `.partial`)
   - Confirm schema install + BLAKE3 in title-index
   - Confirm encode produced H.264 (pick a HEVC-MP4 sample)
   - `why` / `status` output
   - Success bar: beat “need FTP-PASV”, not a published MiB/s SLA in CI
   - Note: “live execution pending operator confirm” until it actually runs

3. Prefer **no** live test that talks to the box by default. If `bins/mediaops/tests/live.rs` exists, it compiles only with `--features live-box` and is ignored unless `MEDIAOPS_LIVE=1`.

4. Sprint-status: mark `4-1`, `4-2`, `4-3`, `epic-4` done **only after** 4.1+4.2 tests are green. 4.3 can be `done` when the runbook and gate exist even if the live run has not been executed.

### Do not in 4.3

- SSH to SeedIt4Me or run NVENC without a human “yes”.
- Enable the user timer against production disk without confirm (`--enable-timer`).
- Claim FTP-PASV 30 MiB/s was beaten without a measured live run.

---

## Architecture / crate rules while coding

- AD-2: `arch-tests` must stay green. `reqwest` still only arr (still unused). `encode`/`store` never in mediaopsd.
- AD-16: ffmpeg/ffprobe/ssh/systemctl only through `ExecPort`.
- AD-17: `run` exits 0 if the apply **loop** finished; skips/holds/encode-refuse inside the loop are envelope data. Exit 3 lock, 4 snapshot mismatch / verify, 5 `encode run` on HDR.
- AD-18: stdout = result only.
- AD-20: no live box/GPU in default tests. Failure-history names in test fn names where this epic owns the row.
- `thiserror` in crates, `anyhow` only in bins.
- Do not add Autobrr/Bazarr, rsync, rclone, native-tls.

---

## Suggested file list (expected, not exclusive)

```
_bmad-output/implementation-artifacts/epic-4-handoff.md   # this file (already written)
_bmad-output/implementation-artifacts/demo-epic-4.md      # 4.3
_bmad-output/implementation-artifacts/sprint-status.yaml
crates/core/src/plan.rs
crates/core/src/pathschema.rs
crates/core/src/jobs.rs
crates/core/src/install.rs          # backup constraint
crates/core/src/walker.rs          # serde if needed
crates/store/src/lib.rs            # v5 path + list jobs
crates/store/src/jobs.rs
crates/store/src/title_index.rs
crates/sync/Cargo.toml
crates/sync/src/lib.rs
crates/sync/src/plan.rs            # new
crates/sync/src/apply.rs           # new
crates/encode/src/lib.rs
crates/encode/src/policy.rs        # new
crates/encode/src/run.rs           # new
bins/mediaops/src/main.rs
bins/mediaops/src/run.rs
bins/mediaops/src/home.rs
bins/mediaops/src/library.rs
bins/mediaops/src/watch.rs         # new or inlined
bins/mediaops/src/status.rs        # new
bins/mediaops/Cargo.toml           # live-box feature
bins/mediaops/tests/cli.rs
```

---

## Verification

After 4.1 and 4.2:

```
cargo test --workspace
cargo test -p mediaops-arch-tests
```

Must be green. `mediaops run --help` shows plan/run/watch/why/status/encode. `mediaopsd --help` unchanged except still reverse-connect unused.

Grep the tree: no `plan/apply waits for Epic 4` stub string.

After 4.3: runbook exists; `live-box` feature on `bins/mediaops`; default tests still pass without GPU.

---

## Sprint / git

- Branch: `epic/4-tonight-playable` from current `main`.
- Three commits as above. PR when 4.1+4.2 are test-green; 4.3 runbook can ride the same PR.
- Update `sprint-status.yaml` `last_updated` and story rows as each story lands (`in-progress` → `done`).
- Do not mark `epic-4: done` until 4.1 and 4.2 are done and 4.3 artifacts exist.

---

## Risks — do not “simplify away”

1. **Unit `Action` left as-is** — apply becomes out-of-band job rows and the Plan artifact is a lie.
2. **Planner invents TitleId from scene names** — PathSchema is the only writer; unparseable remotes are omitted.
3. **CLI dials the seedbox** — forbidden; UDS only (bootstrap probe chicken-egg already exists; do not spread it to plan/run).
4. **Encode on mediaopsd** — arch-tests must fail that.
5. **Backup under `movies/`** — forbidden; `_ops/backup-hevc-originals/` after fixing `replace`’s overly broad `starts_with(library_root)`.
6. **Live demo in CI** — forbidden.
7. **`run` exit 0 while still a stub** — the 4.1 stub must die; a timer that no-ops is the named leftover-reclaim failure class.
