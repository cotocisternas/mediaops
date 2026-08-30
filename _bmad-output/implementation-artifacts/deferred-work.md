# Deferred Work

Items raised by review workflows and consciously deferred. Each entry names the
review that raised it so a later story can pick it up with context.

## Deferred from: code review of spec-1-2-pathschema-titleid-and-the-walker (2026-08-29)

Story 1.2 review is **closed** (2026-08-30). Spec `followup_review_recommended: false`. These eight items remain ledger-only; they are not a second 1.2 implementation pass.

- **No `NAME_MAX` or path-length guard on rendered components** (`crates/core/src/pathschema.rs:324`) — `validate_display_token` accepts an arbitrarily long title, so a 300-character title renders a folder or file name past the 255-byte limit. The failure surfaces as an opaque `InstallError::Io` at `fs::rename` time instead of a `PathSchemaError` at render time.
- **Walker recursion depth is unbounded** (`crates/core/src/walker.rs:181`) — `walk_dir` recurses per directory level with no depth cap. A deeply nested remote tree can exhaust the stack and abort the process. Needs either a depth limit or an explicit statement that allowlisted roots are trusted.
- **`RemoteEntry` carries `nlink` but no `dev`/`ino`** (`crates/core/src/walker.rs:36`) — a caller can tell *that* a file is hardlinked but never *to what*, which is what hardlink-aware handling of seeded torrents actually needs. Deferred because the epic pins `RemoteEntry` to `{ref, len, mtime, nlink}` and story 1.4 mirrors it field-for-field on the wire; adding fields is a contract change.
- **Non-unix builds fabricate `nlink = 1`** (`crates/core/src/walker.rs:229`) — the `#[cfg(not(unix))]` branch returns a literal `1`, so hardlink detection silently always reports "not linked". Both test modules also call `std::os::unix::fs::symlink` without a cfg gate, so the crate claims a non-unix build its own tests cannot compile. Deferred: there is no non-unix target today.
- **Legitimate titles containing "Proper" or "Repack" are unrenderable** (`crates/core/src/pathschema.rs:377`) — `is_scene_tag` matches case-insensitively on any dot/dash/underscore-separated token, and `validate_display_token` turns a match into a hard `LeftoverSceneTag` failure. A film titled `A.Proper.Violence` can never enter the library. Deferred: the spec mandates stripping these tags; making the match positional (trailing release segment only) is a product call.
- **No `fsync` of the parent directory after rename** (`crates/core/src/install.rs:146`) — `install` is documented as atomic placement, but without syncing the parent dir the rename can be lost on power failure. Deferred as beyond a story scoped to types and offline tests.
- **Nothing constrains the staged source to a staging root** (`crates/core/src/install.rs:76`) — `VerifiedStagingHandle::verify` only checks `source.ends_with(staging_path(...))`, and `Path::ends_with` matches trailing components, so any directory on disk ending in `_incoming/<TitleId>/<final name>` verifies. `library_root` is not a parameter of `verify`, and `install` never compares the source against it. Deferred: adding a staging-root parameter changes the install-gate signature that stories 1.3 and Epic 3 will bind to.
- **`REPACJ` is frozen into a public const, doc comment, and test** (`crates/core/src/pathschema.rs:15`) — it reads as a typo of `REPACK`, but both the 1.2 spec and epic-1-context list it explicitly, so the implementation is faithful. Deferred: confirm upstream whether real releases carry the misspelling, and add a comment either way. Common tags (`INTERNAL`, `RERIP`, `REAL`) are absent with no note on why the list stops here.

## Deferred from: spec split of spec-1-3-desiredstate-plan-jobs-and-store (2026-08-29)

Explored 2026-08-30 (after PR #4). Proposed follow-ons: slice A `core::jobs` (pure), then slice B store v2 (traits + `title_index`/`jobs`, migrate `user_version` 1→2). See spec-1-3 "Split remainder". Do not greenfield sqlite: Epic 2 already owns `probes` at version 1.

- source_spec: `_bmad-output/implementation-artifacts/spec-1-3-desiredstate-plan-jobs-and-store.md`
  summary: `core::jobs` state machines — JobKind, per-kind states, `advance`, and Encode-ready-when-parent-Installed.
  evidence: Split from 1.3 so this spec can stay inside the token budget; AD-10 types are independently shippable from DesiredState/Plan and were inflating the I/O matrix and Design Notes. Still unbuilt. Can land before sqlite (Epic 4 is the first consumer).
- source_spec: `_bmad-output/implementation-artifacts/spec-1-3-desiredstate-plan-jobs-and-store.md`
  summary: `store` v2 persistence — rusqlite adapter already exists for `probes`; add `title_index`/`jobs` tables and repository traits in `core`.
  evidence: Split from 1.3; AD-8. Epic 2 inverted the original "do not create probes yet" AC. Next story migrates 1→2, wraps probes in the same trait family, keeps mediaopsd store-free.
- source_spec: `_bmad-output/implementation-artifacts/spec-1-3-desiredstate-plan-jobs-and-store.md`
  summary: `install.rs` still says digest persistence is story 1.3, but this slice deferred `title_index`/`store`.
  evidence: `crates/core/src/install.rs` crate docs still claim replace is the only writer of `current_b3` and that persistence is story 1.3. Fold the comment (and the persistence door) into slice B.

## Deferred from: code review of PR #4 / SPEC.md (2026-08-30)

- **Home gateway owns the WAN pool and the CLI never contains a seedbox address** (`bins/mediaops/src/bootstrap.rs:123`) — AD-4 / NFR3 / Epic 3. Bootstrap today calls `probe_range_n` from the CLI. Long-term the home unix-socket gateway is the only process that knows the seedbox address; moving the probe is Epic 3 work, not a silent Epic 2 rewrite.
- **`store` repository traits live in `core`** (`crates/store/src/lib.rs:21`) — AD-8. This slice added a concrete `Store` and `probes` table; the 1.3 split already deferred the trait + `title_index`/`jobs`.
- **`parse_ssh_config` does not honor `Include`, `Host *`, or `Match`** (`crates/ssh/src/lib.rs:34`) — v1 imports `Host seedbox` as an exact block. Full OpenSSH semantics can wait until a real config fails.
- **GetRange is not a streaming disk pipe** (`crates/net/src/seedbox.rs:146`) — after a size cap and a full read, true chunked `AsyncRead` with client-cancel backpressure is still missing. Deferred as a follow-up to the cap/read fix.
- **TLS accept is sequential (`.then`)** (`crates/net/src/listen.rs:28`) — a handshake timeout is the important guard; concurrent accept is a later listen-loop refinement.
- **`upsert_tls_table` round-trips TOML and drops comments** (`crates/core/src/desired_state.rs:227`) — AD-6 wants a diff before rewriting user-edited desired-state. Comment-preserving splice is a larger TOML-edit problem than this review should invent.
- **`ControlPort::df` returns only `Bytes` and drops daemon semver** (`crates/proto/src/lib.rs:292`) — Story 2.2 wants the CLI to see semver + proto package. The Epic 1 `ControlPort` trait shape only returns free bytes; changing it is a contract story, not a net-crate patch.
- **musl-static `mediaopsd` vs `tls-aws-lc` / `.cargo/config.toml`** (`crates/net/Cargo.toml:20`) — Story 2.3 / AD-22 require `x86_64-unknown-linux-musl`. Whether aws-lc-sys links on that target is unproven here; pin the target and crypto backend when the first musl build is actually run.
