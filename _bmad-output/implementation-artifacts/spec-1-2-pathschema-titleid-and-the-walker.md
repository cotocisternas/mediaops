---
title: '1.2 PathSchema, TitleId, and the walker'
type: 'feature'
created: '2026-08-29'
status: 'done'
baseline_revision: 'a3273af46fab9af514cf311355e9bfcccf2d79c8'
review_loop_iteration: 0
followup_review_recommended: true
context:
  - '_bmad-output/implementation-artifacts/epic-1-context.md'
warnings: []
deferred: []
---

<intent-contract>

## Intent

**Problem:** `mediaops-core` has ExitCode and envelopes but no TitleId, PathSchema, walker, or install gate, so later stories cannot name library paths or list remotes without inventing a second renderer.

**Approach:** Add TitleId, `core::pathschema` (only renderer/parser), allowlist walker (`RemoteRef`/`RemoteEntry`), `staging_path`, and install-gate entry points `install`/`replace` as offline-tested types in `mediaops-core`.

## Boundaries & Constraints

**Always:**
- TitleId serializes `kind:source:id` with kinds `movie`+`tmdb`, `series`+`tvdb`, `album`+`mbid` (example `movie:tmdb:603`). Identity is TitleId, never a path string. Music remasters key by MBID, not folder year.
- Only `core::pathschema` renders/parses library paths. `parse(render(id)) == id`. Year is identical in the title folder and the file stem. Spaces are refused. Strip scene tags `REPACJ`, `REPACK`, `PROPER`. Reject bins include `needs-split` and `needs-year`.
- Staging paths only from `core::pathschema::staging_path` → `_incoming/<TitleId serialized>/…`.
- One walker is the sole producer of `RemoteRef {root_id, rel_path}` and `RemoteEntry {ref, len, mtime, nlink}`. Unknown paths error. Never follow symlinks off the allowlist. Do not list torrent save paths or `torrents/incomplete`.
- Install gate: `install(TitleId, verified staging handle) -> installed path` and `replace(TitleId, verified .converting handle, backup destination) -> installed path`. `replace` is encode's path and the only writer of `current_b3` (persistence of digests is story 1.3; this story returns the path). Callers other than tests may wait.
- TitleId/PathSchema stay pure functions. Walker and install gate use caller-supplied filesystem roots (tempdir fixtures). No tokio, network, rusqlite, reqwest, or proto codegen. `thiserror` in core. Default tests offline.
- Keep 1.1 workspace, AD-2 graph, binaries, and ExitCode/envelope/`CapabilityToken` behavior.

**Block If:** A pinned crate must be added that is unpublished, or rustc is not 1.98.0.

**Never:** DesiredState/Plan/jobs/`title_index`/store, tonic/proto, TLS, CLI verbs, live box/GPU, docs render, serving Transfer, writing `current_b3` to sqlite, a second path renderer, listing off-allowlist, following escape symlinks.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| TitleId round-trip | `movie:tmdb:603`, a series TVDB id, an album MBID | `parse(render(id)) == id` | Invalid kind/source/id → error, not a silent TitleId |
| Library year | Placement with year Y | Folder name and file stem both contain Y the same way; no spaces in the rendered relative path | Space in title → refuse |
| Remaster | Two album folders `Relayer.(1974)` and `Relayer.(2013)` sharing one MBID | `parse` yields that album TitleId for both; year is not identity | Must not treat year as a different album |
| Scene strip | Names containing REPACJ, REPACK, or PROPER | Those tags are removed | Leftover tag in a library path is not a successful parse |
| Reject bins | Paths using `needs-split` or `needs-year` | Classified as those reject bins, not a TitleId | Not a silent title parse |
| staging_path | TitleId + final name | `_incoming/<kind:source:id>/<final name>` | Empty/invalid TitleId → error |
| Walker happy | Tempdir allowlisted root with files | Only `RemoteRef`/`RemoteEntry` under that root (`len`/`mtime`/`nlink` from metadata) | No error expected |
| Unknown path | List or resolve outside the allowlist | Error | Do not return an entry |
| Symlink escape | Symlink inside allowlist pointing outside | Do not follow; outside contents are not listed | No entries for the escaped target |
| Torrent skip | Tree containing `torrents/incomplete` and a non-allowlisted torrent save dir | Those paths are not listed | Allowlisted files still list |

</intent-contract>

## Code Map

Reuse (do not rewrite): `crates/core/src/lib.rs` ExitCode, Envelope, Identity, CapabilityToken and their tests; `crates/core/Cargo.toml` serde/serde_json/thiserror only; workspace pins; AD-2 in `crates/arch-tests`; composition-root binaries.

Add modules under `crates/core/src/` and re-export from `lib.rs`:
- `title_id.rs` -- `TitleId` {kind, source, id}; render/parse `kind:source:id`
- `pathschema.rs` -- library render/parse (movie / episode / track), scene-tag strip, reject bins `_ops/needs-split` and `_ops/needs-year`, `staging_path`
- `walker.rs` -- `RemoteRef`, `RemoteEntry`, allowlist list; sole producer of both types
- `install.rs` -- `VerifiedStagingHandle`, `VerifiedConvertingHandle`, `install`, `replace`; destination via PathSchema; atomic place into a caller library root; `replace` moves the live file to `backup_destination`

Read-only evidence: epic-1-context Identity and paths; AD-11 staging layout; AD-13; AD-20 tempdir/schema CI; 1.1 spec Never list (this story is what 1.1 deferred).

## Tasks & Acceptance

**Execution:**
- `crates/core/src/title_id.rs` -- TitleId render/parse + tests for movie/series/album round-trip -- identity law
- `crates/core/src/pathschema.rs` -- versioned grammar, year-in-folder-and-file, space refuse, scene-tag strip, reject bins, `staging_path`; unit-test every I/O-matrix path row -- AD-13/schema CI
- `crates/core/src/walker.rs` -- allowlist walker producing only `RemoteRef`/`RemoteEntry`; unit-test happy/unknown/symlink/torrent rows with tempdir trees -- AD-13/NFR6
- `crates/core/src/install.rs` -- `install` and `replace` signatures + tempdir tests that they are the only writers into the schema library path -- AD-13 gate
- `crates/core/src/lib.rs` -- `mod` + re-exports; crate docs: PathSchema/TitleId pure; walker/install may use caller roots; still no tokio/network -- keep 1.1 types public

**Acceptance Criteria:**
- Given TitleIds `movie:tmdb:…`, `series:tvdb:…`, and `album:mbid:…`, when PathSchema renders then parses, then `parse(render(id)) == id`, year matches in folder and file stem, spaces are refused, and album parse keys by MBID not folder year.
- Given names with REPACJ, REPACK, or PROPER, when the scene-tag strip runs, then those tags are gone, and `needs-split` / `needs-year` are reject bins.
- Given a remote-root allowlist in a tempdir, when the walker lists, then it emits only typed `RemoteRef`/`RemoteEntry`, unknown paths error, symlinks off the allowlist are not followed, and torrent save paths plus `torrents/incomplete` are not listed.
- Given staging and the install gate, when any path is built, then it uses only `staging_path` (`_incoming/<TitleId>/…`), and `install` / `replace` exist as the two library-path writers (tests call them; other crates may wait).
- Given `cargo test -p mediaops-core` and `cargo test -p mediaops-arch-tests` with default features, when they run, then they pass offline, binaries still match the 1.1 CLI matrix, and core has no tokio/rusqlite/reqwest.

## Spec Change Log

## Review Triage Log

### 2026-08-29 — Review pass
- intent_gap: 0
- bad_spec: 0
- patch: 12: (high 0, medium 7, low 5)
- defer: 0
- reject: 22
- addressed_findings:
  - `[medium]` `[patch]` `RemoteRef`/`RemoteEntry` fields are private; walker is the only constructor [`crates/core/src/walker.rs`](../../crates/core/src/walker.rs)
  - `[medium]` `[patch]` Album track stems carry the same `.(YYYY)` as the folder [`crates/core/src/pathschema.rs`](../../crates/core/src/pathschema.rs)
  - `[medium]` `[patch]` Reject bins only match `_ops/needs-split` and `_ops/needs-year` [`crates/core/src/pathschema.rs`](../../crates/core/src/pathschema.rs)
  - `[medium]` `[patch]` Duplicate canonical allowlist roots are refused; `list` walks the canonical dir [`crates/core/src/walker.rs`](../../crates/core/src/walker.rs)
  - `[medium]` `[patch]` Staging verify requires the exact `staging_path` tail, rejects symlinks, and rejects another TitleId's incoming dir [`crates/core/src/install.rs`](../../crates/core/src/install.rs)
  - `[medium]` `[patch]` Converting verify rejects symlink sources [`crates/core/src/install.rs`](../../crates/core/src/install.rs)
  - `[medium]` `[patch]` Second `install` is `DestinationExists`; `replace` refuses an existing backup and restores the live file if the converting rename fails [`crates/core/src/install.rs`](../../crates/core/src/install.rs)
  - `[low]` `[patch]` Years outside `1000..=9999` are refused on render [`crates/core/src/pathschema.rs`](../../crates/core/src/pathschema.rs)
  - `[low]` `[patch]` `.`, `..`, and interior NUL are refused in display tokens and staging final names [`crates/core/src/pathschema.rs`](../../crates/core/src/pathschema.rs)
  - `[low]` `[patch]` Movie stems have nothing after `.(YYYY)`; series stems are exactly `.SxxExx` [`crates/core/src/pathschema.rs`](../../crates/core/src/pathschema.rs)
  - `[low]` `[patch]` MBID ids normalize to lowercase [`crates/core/src/title_id.rs`](../../crates/core/src/title_id.rs)
  - `[low]` `[patch]` Happy-path `resolve` matches `list`; `nlink` is compared to `fs::metadata` [`crates/core/src/walker.rs`](../../crates/core/src/walker.rs)

## Design Notes

Library render takes TitleId plus placement (display title, year, and episode/track extras). TitleId itself has no title string; year in the path is display, recovered identity is the `{tmdb|tvdb|mbid-…}` token in the folder.

Golden paths (dots, no spaces; year copied into folder and stem):

```
movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mkv
series/The.Wire.(2002).{tvdb-79126}/The.Wire.(2002).S01E01.mkv
music/Relayer.(2013).{mbid-0f82b02e-c6cd-4242-b195-93d4bf3e0d63}/01.The.Gates.Of.Delirium.(2013).flac
```

`parse` of a 1974 Relayer folder with the same MBID still yields that album TitleId. `RemoteRef.rel_path` is relative to its allowlisted root, never an absolute string.

## Verification

**Commands:**
- `cargo test -p mediaops-core --offline --locked` -- pass, including matrix rows
- `cargo test -p mediaops-arch-tests --offline --locked` -- pass (AD-2 unchanged)
- `cargo test -p mediaops --offline --locked` and `cargo test -p mediaopsd --offline --locked` -- 1.1 CLI matrix still pass

## Auto Run Result

Status: done

**Summary:** TitleId (`kind:source:id`), versioned PathSchema (only renderer/parser), allowlist walker (`RemoteRef`/`RemoteEntry`), `staging_path`, and install-gate `install`/`replace` in `mediaops-core`. 1.1 ExitCode/envelope/CLI and AD-2 unchanged.

**Files changed:**
- [crates/core/src/title_id.rs](../../crates/core/src/title_id.rs) — TitleId render/parse; MBID lowercased
- [crates/core/src/pathschema.rs](../../crates/core/src/pathschema.rs) — grammar, scene-tag strip, reject bins, staging_path
- [crates/core/src/walker.rs](../../crates/core/src/walker.rs) — allowlist list/resolve; sealed RemoteRef/RemoteEntry
- [crates/core/src/install.rs](../../crates/core/src/install.rs) — verified handles, install, replace
- [crates/core/src/lib.rs](../../crates/core/src/lib.rs) — modules and re-exports; crate docs

**Review findings:** 12 patches applied (0 high, 7 medium, 5 low). 0 deferred. 22 rejected (sqlite `current_b3`, blake3/partial, CLI/docs, cross-crate uniqueness tests, TOCTOU exclusive rename, in-allowlist symlink follow, serde, extra parse grammars).

**Follow-up review recommendation:** true. Patched this pass: high 0, medium 7, low 5. Score `3 × 7 + 1 × 5 = 26` (≥ 5).

**Verification:** `cargo test -p mediaops-core --offline --locked` → 35 passed. `cargo test -p mediaops-arch-tests --offline --locked` → 8 passed. `cargo test -p mediaops` and `mediaopsd --offline --locked` → 1.1 CLI matrix still pass (3 unit + 9 cli each).

**Residual risks:** `fs::rename` is same-filesystem; cross-device staging → library waits. Walker skips every symlink (stricter than follow-if-still-allowlisted). `replace` still does not persist `current_b3` (story 1.3). `RemoteEntry.ref_()` is the public getter because `ref` is a keyword — proto mirroring in 1.4 should map that field explicitly.

