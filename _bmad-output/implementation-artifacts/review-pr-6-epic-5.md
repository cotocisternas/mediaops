# Review findings — PR #6 (Epic 5)

Source: `main...epic/5-quiet-box`, chunked by story. SPEC-mediaops + `failure-history-tests.md` + `grabber-inventory.md` + spine AD-15 + `epics.md` 5.1–5.4.
Layers: blind-hunter, edge-case-hunter, verification-gap, acceptance-auditor.

SPEC.md / epics.md have no Tasks/Subtasks section; this file is the persisted review list.

## Chunk 5.1 — `001d80f` arr crate over HttpTransport

22 files (Cargo.lock omitted), +2764 / −1. Remaining chunks: 5.2 grab apply, 5.3 doctor/repair, 5.4 upgrade/pins.

5.1 patches applied 2026-09-01. `cargo test --workspace --locked --offline` green.

### Review Findings

- [x] [Review][Patch] SAB puts the real API key in the cassette path; CassetteMiss and HTTP errors echo it [`crates/arr/src/sab.rs:36`]
- [x] [Review][Patch] `queue_replays_cassette_without_echoing_key_in_errors` never builds an error [`crates/arr/src/sab.rs:112`]
- [x] [Review][Patch] `ReqwestTransport` follows redirects and has no timeout or body cap [`crates/arr/src/reqwest_impl.rs:14`]
- [x] [Review][Patch] qBit login treats `200 Fails.` as success and synthesizes `SID=cassette` [`crates/arr/src/qbit.rs:31`]
- [x] [Review][Patch] `test_indexer` only refuses root `apiKey`; Servarr `fields[].value` masked keys still POST [`crates/arr/src/servarr.rs:347`]
- [x] [Review][Patch] Named 401/409 workspace cassettes never run through `ArrClient` [`crates/arr/src/lib.rs:48`]
- [x] [Review][Patch] `post_indexer` is GET-then-POST by name; 409 is not mapped; `post_download_client` has no duplicate guard [`crates/arr/src/servarr.rs:237`]
- [x] [Review][Patch] `ArrClient` omits `wanted/cutoff` [`crates/arr/src/servarr.rs:307`]
- [x] [Review][Patch] Query values are interpolated unescaped (`parse`, `search`, `filesystem`, `release`, SAB extras) [`crates/arr/src/servarr.rs:327`]
- [x] [Review][Patch] `http_error` redacts only all-star bodies; `truncate` panics on a UTF-8 char boundary [`crates/arr/src/servarr.rs:359`]
- [x] [Review][Patch] qBit privacy defaults are fail-closed in code but untested when `dht`/`pex`/`lsd` are omitted [`crates/arr/src/qbit.rs:143`]
- [x] [Review][Patch] qBit discovery uses `is_file` (IO errors look absent) and stores the sentinel `"present"` [`crates/arr/src/keys.rs:145`]
- [x] [Review][Patch] `xml_tag` treats an unclosed `ApiKey` as absence; `ini_value` takes the first `api_key=` in any section [`crates/arr/src/keys.rs:161`]
- [x] [Review][Patch] Empty Servarr/SAB API keys pass `refuse_masked` and are sent [`crates/arr/src/servarr.rs:89`]
- [x] [Review][Patch] SAB `200` + `status: false` is treated as success; error bodies are unredacted [`crates/arr/src/sab.rs:52`]
- [x] [Review][Patch] Indexer/client list items without `name` are silently dropped [`crates/arr/src/servarr.rs:380`]
- [x] [Review][Patch] `application_url_ok` uses substring `contains`; `url_base` `/` matches every URL [`crates/arr/src/prowlarr.rs:52`]
- [x] [Review][Patch] `Sonarr::seasons` calls `/api/v3/season`, which is not a Servarr v3 resource [`crates/arr/src/sonarr.rs:31`]
- [x] [Review][Patch] `HttpRequest`/`HttpResponse` `Debug` prints `X-Api-Key`, form bodies, and `Set-Cookie` in the clear [`crates/arr/src/transport.rs:6`]
- [x] [Review][Patch] Cassette `read_dir` skips entry errors via `.ok()` [`crates/arr/src/cassette.rs:71`]
- [x] [Review][Patch] `parse_host_config` turns missing bind/`urlBase`/auth fields into empty strings [`crates/arr/src/servarr.rs:414`]
- [x] [Review][Patch] `queue`/`history`/`blocklist`/`wanted_missing` issue a single unpaged GET [`crates/arr/src/servarr.rs:295`]

## Chunk 5.2 — `bf37f05` grab apply as set-diff

19 files, +1391 / −112. 5.2–5.4 patches applied 2026-09-02. `cargo test --workspace --locked --offline` green.

### Review Findings

- [x] [Review][Patch] Require full indexer/client resources in desired-state (fields/configContract/implementation); GET-merge-PUT live objects so updates do not wipe secrets [`crates/arr/src/apply.rs:204`] — decided 2026-09-02: schema change, not identity stubs
- [x] [Review][Patch] Nginx repair reads live conf and splices `Host $host` only; do not replace the whole Swizzin snippet [`crates/ssh/src/lib.rs:148`] — decided 2026-09-02

- [x] [Review][Patch] Custom-format packs PUT live JSON unchanged and ignore `pack.scores`; missing CFs POST `{"name"}` only [`crates/arr/src/apply.rs:304`]
- [x] [Review][Patch] `GrabPolicy.quality_profile` is never applied; delay reads `delay` instead of `usenetDelay`/`torrentDelay` [`crates/arr/src/apply.rs:341`]
- [x] [Review][Patch] Missing *arr keys skip that app and `GrabApply` reports `noop: true` [`crates/arr/src/apply.rs:92`]
- [x] [Review][Patch] Seedbox `GrabApply` handshake-noops when `grab_ops` is `None`; CLI second-apply test never injects Servarr `GrabOps` [`crates/net/src/seedbox.rs:152`]
- [x] [Review][Patch] Indexer names are unique globally, not `(app, name)`; empty want for an app deletes all live indexers on that app [`crates/core/src/desired_state.rs:288`]
- [x] [Review][Patch] Indexer create uses `post_json`, so HTTP 409 is not `DuplicateIndexer` [`crates/arr/src/apply.rs:204`]
- [x] [Review][Patch] `LocalhostGrabOps` freezes `url_bases`/bind at daemon start; `HOME` unset uses `/` [`bins/mediaopsd/src/main.rs:250`]
- [x] [Review][Patch] Live indexer/client rows without `id` are skipped, not failed [`crates/arr/src/apply.rs:195`]
- [x] [Review][Patch] Duplicate `download_clients` / `custom_format_packs` / `paths.roots.id` names are untested in `from_toml` [`crates/core/src/desired_state.rs:284`]
- [x] [Review][Patch] Download-client set-diff, priority updates, and locked-servarr `GrabApply` have no observing assertions [`crates/arr/src/apply.rs:240`]

## Chunk 5.3 — `bcbfc79` EdgeInvariant, doctor, repair

20 files, +1183 / −38.

### Review Findings

- [x] [Review][Patch] `seedbox_exec_start` never passes `--nginx-dir`; empty fingerprint cannot freeze apply [`bins/mediaops/src/bootstrap.rs:356`]
- [x] [Review][Patch] `seedbox_apply_freezes_when_fingerprint_drifted` never calls `seedbox_apply`; doctor/run freeze is untested at the command [`bins/mediaops/src/apply_cmd.rs:131`]
- [x] [Review][Patch] Prowlarr application URLs are checked against Prowlarr’s own `url_base` (`/prowlarr`), so a normal Sonarr URL is drift [`crates/arr/src/apply.rs:443`]
- [x] [Review][Patch] `apply_edge_host` ignores request `DesiredState` and PUTs only bind/`urlBase`/auth stubs [`crates/arr/src/apply.rs:498`]
- [x] [Review][Patch] Repair runs API apply before nginx write, does not `nginx -t`/reload, and `write_remote_file` treats a failed `sudo cat` as empty [`bins/mediaops/src/repair.rs:62`]
- [x] [Review][Patch] API host writes never render `core::unified_diff` before PUT; repair prints fingerprint after mutation [`crates/arr/src/apply.rs:535`]
- [x] [Review][Patch] `prepare` treats `edge_check` errors as not frozen, so copies proceed when EdgeInvariant cannot be verified [`bins/mediaops/src/run.rs:273`]
- [x] [Review][Patch] Repair never PUTs Prowlarr application URLs, so detect-only drift cannot be cleared [`crates/arr/src/apply.rs:515`]
- [x] [Review][Patch] PEM scan is `.pem` only, depth 4, swallows `read_dir` errors, and only runs if `config_dir`/`tls_dir` themselves are git work trees [`bins/mediaops/src/doctor.rs:37`]
- [x] [Review][Patch] `nginx_host_ok` is a raw substring; `::` bind is not bind-to-star; missing keys skip host checks with no drift line [`crates/core/src/diff.rs:30`]
- [x] [Review][Patch] Doctor DriftVerify, PEM-through-`doctor()`, repair `put_machine`, and `edge_api_check` for `0.0.0.0`/auth/Prowlarr `/{id}/` are untested [`bins/mediaops/src/doctor.rs:135`]

## Chunk 5.4 — `ba27b66` seedbox upgrade and pin matrix

13 files, +695 / −18.

### Review Findings

- [x] [Review][Patch] `pin_matrix_refuse` only compares TOML `pins.lidarr` to `refuse_above`; `os`/`glibc_min` are format strings; unparseable pins `continue` [`crates/core/src/desired_state.rs:179`]
- [x] [Review][Patch] `minor_skew_warning` is called only from `upgrade()`, not when the CLI connects (`doctor`/`apply`/`run`) [`crates/proto/src/lib.rs:34`]
- [x] [Review][Patch] Every `upgrade()` test sets `skip_edge: true`; production `skip_edge: false` (df, skew, post-copy edge check) is untested [`bins/mediaops/src/bootstrap.rs:289`]
- [x] [Review][Patch] Upgrade helper test does not assert the musl `cargo` argv; `install_provider` does [`crates/ssh/src/lib.rs:189`]
- [x] [Review][Patch] `parse_semver` coerces a non-numeric patch to `0` and drops extra segments [`crates/core/src/desired_state.rs:170`]
- [x] [Review][Patch] Upgrade does not retry connect after restart; a failed `edge_check` is `BootstrapError::Io` with the new binary already live [`bins/mediaops/src/bootstrap.rs:289`]
- [x] [Review][Patch] Successful upgrade never writes `EDGE_FINGERPRINT_KEY`; bootstrap install still has no edge check before success [`bins/mediaops/src/bootstrap.rs:314`]
