---
title: '8.1 Library relocate and new-machine'
type: 'feature'
created: '2026-09-03'
status: 'done'
baseline_commit: '23d7f19e581b58436ffeadb16b8e994cdb7e89cb'
review_loop_iteration: 0
context:
  - '_bmad-output/implementation-artifacts/epic-8-context.md
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Moving the home disk or standing up a new home still means hand-editing `library_root`, systemd units, and title-index proof. Losing `state.db` without an export silently disables reclaim.

**Approach:** Exclusive `library relocate` retargets the one home library through store + unit writers. `new-machine export|import` bundles desired-state + `tls/` + title-index (both digests) into the active XDG dirs, never a git work tree. `library reindex` rebuilds proof from on-disk schema files.

## Boundaries & Constraints

**Always:**
- Home root is sqlite `machine.library_root`, not DesiredState. Relocate writes the new canonical root, `ensure_layout`s, and rewrites the three user units via existing writers. Title-index `path` is schema-relative — do not prefix-rewrite relative rows. Absolute paths under the old root rewrite through `store` only.
- Install/replace remain the only filesystem library-path writers. Relocate does not copy or move media.
- Export is a directory: `desired-state.toml`, `tls/`, `title-index.json` from a `store` API (both digests). Import writes the active config dir + `state.db` — never a git work tree. Layout bootstraps with no media. Import restores both digests (not `record_install`). Refuse import if destination `title_index` is non-empty.
- Empty `state.db` (no export) → reclaim still has no proof. `library reindex` walks `scan_schema_files`, hashes with `Blake3Hex::of_reader`, `record_install`s. Matching digest may backfill path; mismatch is error, never overwrite `install_b3`.
- Exclusive `mediaops.lock` on relocate, reindex, export, import. `--json`. CLI-only store. `systemctl` only with `--enable-timer`, same ExecPort as bootstrap.

**Ask First:** adding `library_root` to DesiredState; tar/gpg bundle instead of a directory; probing a new `systemctl` path.

**Never:** story 8.2 `docs render`; seedbox `[[paths.roots]]` as home roots; mediaopsd linking `store`; PEMs or import into a git work tree; size/mtime as reclaim proof; multi-disk; `OnCalendar`; TUI.

## I/O & Edge-Case Matrix

- relocate happy → canonical `library_root`, schema dirs, units rewritten, relative index paths unchanged; no media copy
- relocate free < `min_free` → exit 5, no store/unit write
- relocate flock held → exit 3
- export then import on empty home (no media) → DS + tls in config dir; both digests in `state.db`; schema dirs exist
- import dest config under `.git` → exit 5, no PEM/DS write
- import into non-empty title-index → exit 2, no clobber
- empty `state.db`, files on disk, no export → reclaim apply deletes 0
- reindex after loss → rows with `install_b3`; reclaim can prove. IO → exit 1; digest clash → error

</frozen-after-approval>

## Code Map

Reuse: `LibraryCommand` Bootstrap-only (`bins/mediaops/src/main.rs:349`); nest `new-machine` like `Hold` (`:260`). Relocate = `bootstrap_library` without NVENC (`bins/mediaops/src/library.rs:24`: flock, watermark, `ensure_layout`, `put_machine("library_root")`, units). Units `write_user_units` / `write_home_unit` / `systemd_exec_start` (`crates/sync/src/lib.rs:62`; timer `OnUnitInactiveSec`, no `OnCalendar`). XDG + lock (`bins/mediaops/src/bootstrap.rs:610`). Title-index trait has no import (`crates/core/src/title_index.rs:14,72`; store `crates/store/src/{title_index.rs:5,lib.rs:102}`) — `record_install` cannot keep a distinct `current_b3`. Reindex: `scan_schema_files` (`crates/sync/src/lib.rs:97`) + `Blake3Hex::of_reader` (`crates/core/src/digest.rs:32`). Reclaim proof is an index row (`crates/core/src/reclaim.rs:164`; empty-db apply deletes 0 at `bins/mediaops/src/reclaim.rs:416`). `refuse_git_work_tree` (`crates/ssh/src/lib.rs:92`) on export `--out` and import config. Envelope `crates/core/src/lib.rs:115`. Arch: mediaopsd must not gain `store` (`crates/arch-tests/src/lib.rs:100`). No home root in DesiredState; `[[paths.roots]]` is seedbox; import copies DS bytes (`crates/core/src/desired_state.rs:61`).

Add: `TitleIndexRepo` full-row import + absolute-prefix rewrite; `library relocate|reindex`; `new-machine export|import`; matrix tests.

## Tasks & Acceptance

**Execution:**
- [x] `crates/core/src/title_index.rs` + `crates/store/src/{title_index,lib}.rs` -- import full rows (both digests); rewrite absolute paths under old root -- AD-8
- [x] `crates/sync/src/lib.rs` -- reindex: scan + hash + `record_install` -- no third FS writer
- [x] `bins/mediaops/src/{main,library}.rs` -- exclusive `library relocate|reindex`; relocate reuses bootstrap layout/units/`library_root` -- AD-7
- [x] `bins/mediaops/src/{main.rs,new_machine.rs}` -- exclusive `new-machine export|import`; directory bundle; git-work-tree refuse -- AD-7
- [x] `bins/mediaops/tests/cli.rs` + store unit tests -- I/O matrix; `--json`; no mediaopsd/store edge -- AD-2

**Acceptance Criteria:**
- Given `library relocate --library-root NEW`, when the schema root changes, then units and `machine.library_root` rewrite through those owners and media files are not copied.
- Given `new-machine export` then import on a new home, when the dest is not a git work tree, then desired-state + tls/ + title-index (both digests) populate the active config dir and machine state, and layout exists before any media.
- Given `state.db` loss without an export, when reclaim runs, then nothing is deleted until `library reindex` re-hashes local schema files.

## Spec Change Log

## Design Notes

Relocate retargets a disk the operator already moved. Bundle: `desired-state.toml`, `tls/`, `title-index.json` as `[{title_id, path, install_b3, current_b3}, …]`. Import copies DS bytes. `--library-root` is required on import so empty `ensure_layout` can run.

## Verification

**Commands:**
- `cargo test --workspace --offline --locked` -- pass (matrix + lock exit 3)
- `cargo test -p mediaops-arch-tests --offline --locked` -- mediaopsd has no `store`

## Suggested Review Order

**Relocate**

- Operator entry: retarget one home disk, never copy media
  [`main.rs:408`](../../bins/mediaops/src/main.rs#L408)

- Flock, watermark, layout, `library_root`, units via existing writers
  [`library.rs:110`](../../bins/mediaops/src/library.rs#L110)

- Refuse empty and filesystem-root `--library-root` before `ensure_layout`
  [`library.rs:212`](../../bins/mediaops/src/library.rs#L212)

- Relative schema paths stay; only absolute-under-old-root rewrites
  [`title_index.rs:79`](../../crates/core/src/title_index.rs#L79)

- Store applies that rewrite under exclusive lock
  [`title_index.rs:143`](../../crates/store/src/title_index.rs#L143)

**new-machine**

- Nested `export|import` like Hold; directory bundle, not an archive
  [`main.rs:164`](../../bins/mediaops/src/main.rs#L164)

- Export copies DS bytes + tls/ + full-row JSON including both digests
  [`new_machine.rs:33`](../../bins/mediaops/src/new_machine.rs#L33)

- Import refuses git/PEMs, empty-index, then layout/root, then `import_rows`
  [`new_machine.rs:79`](../../bins/mediaops/src/new_machine.rs#L79)

- Full-row insert keeps distinct `current_b3`; refuses a non-empty dest
  [`title_index.rs:116`](../../crates/store/src/title_index.rs#L116)

- tls/ replace-copy so dest keys match the bundle, not a merge
  [`new_machine.rs:188`](../../bins/mediaops/src/new_machine.rs#L188)

**Reindex**

- Exclusive rebuild of proof from on-disk schema files only
  [`library.rs:177`](../../bins/mediaops/src/library.rs#L177)

- `scan_schema_files` + `Blake3Hex::of_reader` + `record_install`; clash never overwrites
  [`lib.rs:114`](../../crates/sync/src/lib.rs#L114)

**Tests**

- Relocate: canonical root, units, relative paths, no media copy
  [`cli.rs:1089`](../../bins/mediaops/tests/cli.rs#L1089)

- Empty index + files → no reclaim proof; reindex restores `install_b3`
  [`lib.rs:647`](../../crates/sync/src/lib.rs#L647)
