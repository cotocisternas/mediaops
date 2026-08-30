# Deferred Work

Items raised by review workflows and consciously deferred. Each entry names the
review that raised it so a later story can pick it up with context.

## Deferred from: code review of spec-1-2-pathschema-titleid-and-the-walker (2026-08-29)

- **No `NAME_MAX` or path-length guard on rendered components** (`crates/core/src/pathschema.rs:324`) — `validate_display_token` accepts an arbitrarily long title, so a 300-character title renders a folder or file name past the 255-byte limit. The failure surfaces as an opaque `InstallError::Io` at `fs::rename` time instead of a `PathSchemaError` at render time.
- **Walker recursion depth is unbounded** (`crates/core/src/walker.rs:181`) — `walk_dir` recurses per directory level with no depth cap. A deeply nested remote tree can exhaust the stack and abort the process. Needs either a depth limit or an explicit statement that allowlisted roots are trusted.
- **`RemoteEntry` carries `nlink` but no `dev`/`ino`** (`crates/core/src/walker.rs:36`) — a caller can tell *that* a file is hardlinked but never *to what*, which is what hardlink-aware handling of seeded torrents actually needs. Deferred because the epic pins `RemoteEntry` to `{ref, len, mtime, nlink}` and story 1.4 mirrors it field-for-field on the wire; adding fields is a contract change.
- **Non-unix builds fabricate `nlink = 1`** (`crates/core/src/walker.rs:229`) — the `#[cfg(not(unix))]` branch returns a literal `1`, so hardlink detection silently always reports "not linked". Both test modules also call `std::os::unix::fs::symlink` without a cfg gate, so the crate claims a non-unix build its own tests cannot compile. Deferred: there is no non-unix target today.
- **Legitimate titles containing "Proper" or "Repack" are unrenderable** (`crates/core/src/pathschema.rs:377`) — `is_scene_tag` matches case-insensitively on any dot/dash/underscore-separated token, and `validate_display_token` turns a match into a hard `LeftoverSceneTag` failure. A film titled `A.Proper.Violence` can never enter the library. Deferred: the spec mandates stripping these tags; making the match positional (trailing release segment only) is a product call.
- **No `fsync` of the parent directory after rename** (`crates/core/src/install.rs:146`) — `install` is documented as atomic placement, but without syncing the parent dir the rename can be lost on power failure. Deferred as beyond a story scoped to types and offline tests.
- **Nothing constrains the staged source to a staging root** (`crates/core/src/install.rs:76`) — `VerifiedStagingHandle::verify` only checks `source.ends_with(staging_path(...))`, and `Path::ends_with` matches trailing components, so any directory on disk ending in `_incoming/<TitleId>/<final name>` verifies. `library_root` is not a parameter of `verify`, and `install` never compares the source against it. Deferred: adding a staging-root parameter changes the install-gate signature that stories 1.3 and Epic 3 will bind to.
- **`REPACJ` is frozen into a public const, doc comment, and test** (`crates/core/src/pathschema.rs:15`) — it reads as a typo of `REPACK`, but both the 1.2 spec and epic-1-context list it explicitly, so the implementation is faithful. Deferred: confirm upstream whether real releases carry the misspelling, and add a comment either way. Common tags (`INTERNAL`, `RERIP`, `REAL`) are absent with no note on why the list stops here.

## Deferred from: spec split of spec-1-3-desiredstate-plan-jobs-and-store (2026-08-29)

- source_spec: `_bmad-output/implementation-artifacts/spec-1-3-desiredstate-plan-jobs-and-store.md`
  summary: `core::jobs` state machines — JobKind, per-kind states, `advance`, and Encode-ready-when-parent-Installed.
  evidence: Split from 1.3 so this spec can stay inside the token budget; AD-10 types are independently shippable from DesiredState/Plan and were inflating the I/O matrix and Design Notes.
- source_spec: `_bmad-output/implementation-artifacts/spec-1-3-desiredstate-plan-jobs-and-store.md`
  summary: `store` first persistence — rusqlite adapter, `title_index`/`jobs` tables, and repository traits.
  evidence: Split from 1.3; AD-8 sqlite is a second shippable deliverable (migrate, digest immutability, daemon-must-not-link-store) that does not have to land with the parse/snapshot types.

- source_spec: `_bmad-output/implementation-artifacts/spec-1-3-desiredstate-plan-jobs-and-store.md`
  summary: `install.rs` still says digest persistence is story 1.3, but this slice deferred `title_index`/`store`.
  evidence: `crates/core/src/install.rs` crate docs still claim replace is the only writer of `current_b3` and that persistence is story 1.3. This story's Never list forbids rewriting install; the comment is now wrong relative to the split.
