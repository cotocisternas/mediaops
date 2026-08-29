---
title: '1.2 PathSchema, TitleId, and the walker'
type: 'feature'
created: '2026-08-29'
status: 'done'
baseline_revision: 'a3273af46fab9af514cf311355e9bfcccf2d79c8'
review_loop_iteration: 1
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

### Review Findings

Adversarial code review of commit `35a4f03`, 2026-08-29. Four layers (blind-hunter, edge-case-hunter, verification-gap, acceptance-auditor). Nine findings were reproduced with throwaway tests against a clean tree; those are marked **[confirmed]**.

#### Decisions taken (2026-08-29)

- [x] [Review][Decision] **D1 `RemoteRef`/`RemoteEntry` sealed against story 1.4 → resolved: validating constructor.** Both constructors are `pub(crate) fn new` (`crates/core/src/walker.rs:22`, `:45`) with private fields, so `crates/proto` — which already depends on `mediaops-core` — cannot write the `TryFrom<wire::RemoteRef>` that `epics.md:317` requires. Resolution: add `pub fn from_wire_parts(root_id, rel_path) -> Result<Self, WalkerError>` to both types, enforcing the invariants a receiver can actually check without a filesystem (non-empty `root_id`; `rel_path` relative, no `ParentDir`/`RootDir`/`Prefix`). `pub(crate) new` stays as the walker's filesystem door. The `TryFrom` impls still live in `proto`, so the epic's "proto is the sole home of wire↔domain conversion" law is unchanged. Rationale: a wire-borne ref was already produced by a walker on the remote side and cannot be re-validated locally, so literal sole-production cannot survive 1.4; a feature gate was rejected because Cargo unifies features workspace-wide and would enforce nothing. → becomes a patch.
- [x] [Review][Decision] **D2 `resolve` bypasses the listing exclusions → resolved: share exclusions.** Apply `is_torrent_skip` inside `resolve` so a partial download cannot be minted as a `RemoteRef` through the door `list` closes. One rule, one place. → becomes a patch.
- [x] [Review][Decision] **D3 `core` filesystem I/O vs the epic's "pure domain" law → resolved: add an arch-test.** Add a test in `crates/arch-tests` confining `std::fs` use to `walker` and `install`, so the 1.2 carve-out is enforced the way AD-2 already is rather than living in a doc comment. → becomes a patch.
- [x] [Review][Decision] **D4 `staging_path` embeds `:` → resolved: swap the separator.** `_incoming/movie:tmdb:603/…` cannot exist on SMB, exFAT, or NTFS, while the same function deliberately rejects `/` and `\`. Amend the spec's Always constraint to a filesystem-safe staging token (e.g. `movie-tmdb-603`) while `TitleId::render` keeps its colon form for identity. Requires a Spec Change Log entry plus updates to `staging_path`, `VerifiedStagingHandle::verify`, and `staging_path_uses_serialized_title_id`. → becomes a spec change plus a patch.

- [x] [Review][Patch] Panic: byte-slicing a non-ASCII path aborts `parse` instead of erroring [crates/core/src/pathschema.rs:419, :475]
- [x] [Review][Patch] `render` emits episode/season paths its own `parse` rejects when either number is ≥ 100 [crates/core/src/pathschema.rs:175, :482]
- [x] [Review][Patch] `resolve` accepts a path outside every allowlisted root when it canonicalizes into one [crates/core/src/walker.rs:157]
- [x] [Review][Patch] `torrents/incomplete` is listed when the allowlisted root is the torrents dir itself [crates/core/src/walker.rs:214]
- [x] [Review][Patch] One unreadable subdirectory aborts the entire listing [crates/core/src/walker.rs:184]
- [x] [Review][Patch] Nested allowlist roots yield duplicate entries and an insertion-order-dependent `resolve` [crates/core/src/walker.rs:115]
- [x] [Review][Patch] `dest.exists()` follows symlinks, so a dangling symlink at the library path bypasses `DestinationExists` [crates/core/src/install.rs:140]
- [x] [Review][Patch] `mtime` collapses pre-epoch timestamps and metadata errors into `0` [crates/core/src/walker.rs:221]
- [x] [Review][Patch] `mtime`/`nlink` assertions compare against the same private helpers that produced them [crates/core/src/walker.rs:306]
- [x] [Review][Patch] `TitleMismatch`, `MissingLive`, and `MissingSource` are asserted nowhere; deleting the guards keeps the suite green [crates/core/src/install.rs:137, :161]
- [x] [Review][Patch] `VerifiedConvertingHandle` verifies far less than `VerifiedStagingHandle` despite the shared "Verified" name [crates/core/src/install.rs:88]
- [x] [Review][Patch] `replace` rollback discards the original error with `?`, hiding why the install failed [crates/core/src/install.rs:177]
- [x] [Review][Patch] Leading-zero TMDB/TVDB ids mint two distinct identities for one title [crates/core/src/title_id.rs:170]
- [x] [Review][Patch] Album track stems are parsed far looser than movie and series stems [crates/core/src/pathschema.rs:471]
- [x] [Review][Patch] `backup_destination` is never checked against `library_root`, so a backup can be written into the library [crates/core/src/install.rs:167]
- [x] [Review][Patch] `strip_scene_tags` rewrites `-` and `_` to `.` even with no tag to strip [crates/core/src/pathschema.rs:136]
- [x] [Review][Patch] `parse` is a weaker gate than `render`: braces and reserved tokens it refuses to emit are accepted back [crates/core/src/pathschema.rs:208]
- [x] [Review][Patch] `PathSchemaError::RejectBin(&'static str)` is stringly-typed; `reject_bin()` silently returns `None` on drift [crates/core/src/pathschema.rs:105, :126]
- [x] [Review][Patch] `GRAMMAR_VERSION` is never read by `render`, `parse`, or `staging_path`; its only test is a tautology [crates/core/src/pathschema.rs:13]
- [x] [Review][Patch] `RemoteEntry` exposes two public getters for one field, `r#ref()` and `ref_()` [crates/core/src/walker.rs:54]
- [x] [Review][Patch] `list()` sorts entries but no test observes the order; deleting `sort_by` keeps the suite green [crates/core/src/walker.rs:149]
- [x] [Review][Patch] `Io(String)` discards `ErrorKind`, so EXDEV, ENOSPC, and EACCES are indistinguishable to callers [crates/core/src/walker.rs:88, crates/core/src/install.rs:52]
- [x] [Review][Patch] `reject_symlink_file` reports EACCES, ELOOP, and ENOTDIR all as "source file not found" [crates/core/src/install.rs:114]
- [x] [Review][Patch] The `staging_path` "empty/invalid TitleId → error" branch is unreachable; `TitleId` already enforces it [crates/core/src/pathschema.rs:298]
- [x] [Review][Patch] Module visibility is inconsistent: `pathschema`/`walker` are `pub mod`, `install`/`title_id` are private [crates/core/src/lib.rs:7]

- [x] [Review][Defer] No `NAME_MAX` or path-length guard on rendered components [crates/core/src/pathschema.rs:324] — deferred, surfaces as an opaque io error at install time rather than a schema error
- [x] [Review][Defer] Walker recursion depth is unbounded [crates/core/src/walker.rs:181] — deferred, needs a depth cap or an explicit trust boundary for remote trees
- [x] [Review][Defer] `RemoteEntry` carries `nlink` but no `dev`/`ino`, so hardlinks cannot be paired [crates/core/src/walker.rs:36] — deferred, adding fields changes the 1.4 wire mirror
- [x] [Review][Defer] Non-unix builds fabricate `nlink = 1` and both test modules use `std::os::unix` unguarded [crates/core/src/walker.rs:229] — deferred, no non-unix target today
- [x] [Review][Defer] Legitimate titles containing "Proper" or "Repack" can never be rendered or parsed [crates/core/src/pathschema.rs:377] — deferred, the spec mandates the tag list; needs a product call on positional stripping
- [x] [Review][Defer] No `fsync` of the parent directory after rename, so "atomic" is not crash-durable [crates/core/src/install.rs:146] — deferred, beyond a types-and-offline-tests story
- [x] [Review][Defer] Nothing constrains the staged source to a staging root; any dir ending in the `_incoming/<TitleId>/<name>` tail verifies [crates/core/src/install.rs:76] — deferred, adding a staging-root parameter changes the gate signature
- [x] [Review][Defer] `REPACJ` is frozen into a public const, doc, and test as a scene tag [crates/core/src/pathschema.rs:15] — deferred, it reads as a typo of `REPACK` but both the spec and epic context list it; confirm upstream

## Spec Change Log

### 2026-08-29 — Review pass 2 (commit `35a4f03`)

- **Staging token is no longer the colon-formed TitleId.** The Always constraint
  read "Staging paths only from `core::pathschema::staging_path` →
  `_incoming/<TitleId serialized>/…`". A colon cannot appear in a directory name
  on SMB, exFAT, or NTFS, and `staging_path` already rejects `/` and `\` for the
  same portability reason. Staging paths now use `TitleId::staging_token()` —
  `_incoming/movie-tmdb-603/…`. `TitleId::render()` is unchanged and remains the
  identity on the wire and in the store; only the path token differs.
- **`RemoteRef`/`RemoteEntry` gain a second, validating constructor.** The
  Always constraint read "One walker is the sole producer of `RemoteRef`…".
  Story 1.4 must build these types in `proto` from wire messages, which sealed
  `pub(crate)` constructors made impossible. `from_wire_parts` is now the one
  other door: it cannot re-check a remote filesystem, so it enforces a non-empty
  `root_id` and a relative `rel_path` with no `..`, root, or prefix component.
  The walker remains the sole producer *from a filesystem*, and the `TryFrom`
  impls still live in `proto` per the epic's layering rule.
- **`core` filesystem I/O is now enforced, not just documented.**
  `crates/arch-tests` confines `std::fs` in `mediaops-core` to `walker` and
  `install`, closing the gap where the 1.2 carve-out lived only in a doc comment.


## Review Triage Log

### 2026-08-29 — Review pass 2 (commit `35a4f03`, four layers)

- intent_gap: 0
- bad_spec: 2 (staging colon token; sealed `RemoteRef`/`RemoteEntry` vs the 1.4 mirror)
- patch: 29: (high 2, medium 17, low 10)
- defer: 8
- reject: 6
- decisions_taken: 4 (all resolved into patches; see Decisions taken above)
- addressed_findings:
  - `[high]` `[patch]` `parse` panicked on non-ASCII paths at two byte-slicing sites; both now use a char-boundary-safe tail split [`crates/core/src/pathschema.rs`](../../crates/core/src/pathschema.rs)
  - `[high]` `[patch]` `render` emitted `S01E100`-style paths its own `parse` rejected; season/episode/track ranges are now refused at render [`crates/core/src/pathschema.rs`](../../crates/core/src/pathschema.rs)
  - `[medium]` `[patch]` `resolve` accepted an outside symlink pointing in, and minted refs for `torrents/incomplete`; both are now `UnknownPath` [`crates/core/src/walker.rs`](../../crates/core/src/walker.rs)
  - `[medium]` `[patch]` `torrents/incomplete` is skipped from the absolute path, so allowlisting the torrents dir itself no longer leaks partial downloads [`crates/core/src/walker.rs`](../../crates/core/src/walker.rs)
  - `[medium]` `[patch]` Strict `list` names the failing path; new `list_partial` survives unreadable subtrees and reports what it skipped [`crates/core/src/walker.rs`](../../crates/core/src/walker.rs)
  - `[medium]` `[patch]` Nested allowlist roots are refused (`NestedRoot`), ending duplicate entries and order-dependent `resolve` [`crates/core/src/walker.rs`](../../crates/core/src/walker.rs)
  - `[medium]` `[patch]` `mtime` keeps its sign for pre-epoch timestamps instead of collapsing to `0` [`crates/core/src/walker.rs`](../../crates/core/src/walker.rs)
  - `[medium]` `[patch]` `mtime`/`nlink` are asserted without routing through the helpers under test [`crates/core/src/walker.rs`](../../crates/core/src/walker.rs)
  - `[medium]` `[patch]` `install` uses `symlink_metadata`, so a dangling symlink squatting the library path is `DestinationExists` [`crates/core/src/install.rs`](../../crates/core/src/install.rs)
  - `[medium]` `[patch]` `VerifiedConvertingHandle` requires `<rendered name>.converting`, not any `.converting` file [`crates/core/src/install.rs`](../../crates/core/src/install.rs)
  - `[medium]` `[patch]` `replace` rollback reports both failures via `RollbackFailed` and names where the live bytes are [`crates/core/src/install.rs`](../../crates/core/src/install.rs)
  - `[medium]` `[patch]` A backup destination inside `library_root` is refused; `replace` also refuses a symlinked live file [`crates/core/src/install.rs`](../../crates/core/src/install.rs)
  - `[medium]` `[patch]` `TitleMismatch`, `MissingLive`, and `MissingSource` now have direct tests; deleting the guards fails the suite [`crates/core/src/install.rs`](../../crates/core/src/install.rs)
  - `[medium]` `[patch]` Leading-zero TMDB/TVDB ids are refused, so one title cannot mint two identities [`crates/core/src/title_id.rs`](../../crates/core/src/title_id.rs)
  - `[medium]` `[patch]` Album track stems are parsed as strictly as movie and series stems [`crates/core/src/pathschema.rs`](../../crates/core/src/pathschema.rs)
  - `[medium]` `[patch]` `strip_scene_tags` preserves `-` and `_` separators instead of rewriting them to `.` [`crates/core/src/pathschema.rs`](../../crates/core/src/pathschema.rs)
  - `[medium]` `[patch]` `parse` is no longer a weaker gate than `render`; display tokens are validated on the way back in [`crates/core/src/pathschema.rs`](../../crates/core/src/pathschema.rs)
  - `[low]` `[patch]` `PathSchemaError::RejectBin` carries `RejectBin`, not a `&'static str` that can drift [`crates/core/src/pathschema.rs`](../../crates/core/src/pathschema.rs)
  - `[low]` `[patch]` `GRAMMAR_VERSION` is pinned by golden-path tests instead of a tautological assert [`crates/core/src/pathschema.rs`](../../crates/core/src/pathschema.rs)
  - `[low]` `[patch]` `WalkerError::Io` / `InstallError::Io` carry the path and `ErrorKind`; `io_kind()` exposes it [`crates/core/src/walker.rs`](../../crates/core/src/walker.rs)
  - `[low]` `[patch]` `reject_symlink_file` no longer reports EACCES/ELOOP as "source file not found" [`crates/core/src/install.rs`](../../crates/core/src/install.rs)
  - `[low]` `[patch]` Duplicate getter `ref_()` removed; `list()` order pinned by test; unreachable `InvalidTitleId` branch deleted; module visibility made uniform [`crates/core/src/lib.rs`](../../crates/core/src/lib.rs)
- rejected:
  - Missing `Cargo.toml` hunk — false positive; `thiserror` was already declared at `a3273af`
  - TOCTOU exclusive rename / `RENAME_NOREPLACE` — rejected in pass 1, unchanged
  - EXDEV cross-device copy fallback — recorded residual risk, unchanged
  - `list` skips in-allowlist symlinks while `resolve` follows them — rejected in pass 1
  - Special file types (fifo/socket/device) silently skipped — correct for a media walker
  - "`core` doc claims no I/O" — misread; the doc was updated and the 1.2 spec authorizes the carve-out


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

**Residual risks:** `fs::rename` is same-filesystem; cross-device staging → library waits. Walker skips every symlink (stricter than follow-if-still-allowlisted). `replace` still does not persist `current_b3` (story 1.3). `RemoteEntry` exposes a single getter, `r#ref()`; the duplicate `ref_()` alias was removed in review pass 2. Proto mirroring in 1.4 should map that field explicitly.

