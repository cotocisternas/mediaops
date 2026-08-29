# Review — Version & Reality-Check Verification

- **Lens charge:** verify every committed decision was web-researched or reality-checked rather than asserted from training data — current versions, technology existence and fit, live upstream defaults.
- **Artifact:** `ARCHITECTURE-SPINE.md` (architecture-mediaops-2026-08-29)
- **Evidence source under audit:** `.memlog.md` lines 9–10 (crates.io verification 2026-08-29; tonic 0.14 mTLS/UDS web-confirmation)
- **Review date:** 2026-08-29
- **Method:** independent live checks against the crates.io API (all 18 stack crates, dependency graphs, per-version feature lists), rust-lang `RELEASES.md`, the tonic v0.14.0 GitHub release notes and changelog, docs.rs (tonic 0.14.6 transport, rcgen 0.14.10), and web search for the tonic mTLS/UDS API surface.

## Verdict

**PASS WITH FINDINGS.** The version table is genuinely web-verified — every one of the 18 pinned crate versions matches live crates.io max-stable exactly as of this review, and Rust 1.98.0 (2026-08-20) is exact. The capability claims (mTLS require-and-verify, UDS transport, rcgen CA + P-256, blake3 rayon, tracing JSON) all check out. **One claim is trained-data-stale despite carrying fresh version numbers: the tonic-build + prost codegen pairing.** tonic 0.14 extracted prost integration into `tonic-prost` / `tonic-prost-build`, and neither the spine nor the memlog reflects that.

---

## 1. Stack table vs. live crates.io (2026-08-29)

Every crate checked against `https://crates.io/api/v1/crates/<name>`; `max_stable_version` shown.

| Crate | Spine pin | Live max-stable | Match | Last publish |
| --- | --- | --- | --- | --- |
| tonic | 0.14.6 | 0.14.6 | ✅ | 2026-05-07 |
| tonic-build | 0.14.6 | 0.14.6 | ✅ | 2026-05-07 |
| prost | 0.14.4 | 0.14.4 | ✅ | 2026-06-07 |
| rustls | 0.23.43 | 0.23.43 | ✅ | 2026-07-29 |
| tokio-rustls | 0.26.4 | 0.26.4 | ✅ | 2025-09-26 |
| rcgen | 0.14.10 | 0.14.10 | ✅ | **2026-08-28** |
| blake3 | 1.8.7 | 1.8.7 | ✅ | 2026-08-20 |
| clap | 4.6.6 | 4.6.6 | ✅ | 2026-08-06 |
| rusqlite | 0.40.2 | 0.40.2 | ✅ | 2026-08-08 |
| tokio | 1.53.1 | 1.53.1 | ✅ | 2026-07-20 |
| reqwest | 0.13.4 | 0.13.4 | ✅ | 2026-05-25 |
| serde | 1.0.229 | 1.0.229 | ✅ | 2026-07-18 |
| toml | 1.1.4 | 1.1.4 (+spec-1.1.0) | ✅ | 2026-07-28 |
| tracing | 0.1.44 | 0.1.44 | ✅ | 2025-12-18 |
| tracing-subscriber | 0.3.23 | 0.3.23 | ✅ | 2026-03-13 |
| thiserror | 2.0.20 | 2.0.20 | ✅ | 2026-08-08 |
| anyhow | 1.0.104 | 1.0.104 | ✅ | 2026-07-18 |
| similar | 3.2.0 | 3.2.0 | ✅ | 2026-08-17 |

**Rust toolchain:** `RELEASES.md` on the `stable` branch of rust-lang/rust opens with `Version 1.98.0 (2026-08-20)` — exact match with the memlog, date included. Edition 2024 is long stable (since 1.85), so `edition 2024` on 1.98 is fine.

**Authenticity assessment:** several of these versions were published *days* before the memlog's claimed verification date (rcgen 0.14.10 on 2026-08-28, blake3 1.8.7 on 2026-08-20, similar 3.2.0 on 2026-08-17) and are far past any plausible training cutoff (reqwest 0.13.x, toml 1.x, similar 3.x, rusqlite 0.40.x). A hallucinated table would not land 18/18 including the `+spec-1.1.0` era of toml. The memlog line "(version) Verified on crates.io 2026-08-29" is **corroborated as a genuine live check**, not a trained-data assertion.

## 2. Spot-verified capability claims

### 2.1 tonic 0.14.6 mTLS: require-and-verify client certs — ✅ confirmed (naming nit, see F4)

- `ServerTlsConfig` in tonic 0.14 exposes `client_ca_root(Certificate)` ("sets a certificate against which to validate client TLS certificates") and `client_auth_optional(bool)`, **default `false`** — i.e. once a client CA root is set, client certs are required and verified by default. The official tonic `tls_client_auth` example demonstrates exactly the spine's AD-14 shape (server identity + client CA root, `request.peer_certs()` available in handlers).
- Client side: `ClientTlsConfig` with `ca_certificate(...)` + `identity(...)` — matches "client trusts only that CA" in AD-14.
- tonic 0.14.6 feature list confirms rustls-only TLS (`tls-ring`, `tls-aws-lc`, `tls-native-roots`, `tls-webpki-roots`); its optional TLS dep is `tokio-rustls ^0.26.1`, compatible with the pinned tokio-rustls 0.26.4 and rustls 0.23.43. AD-14's "rustls everywhere; native-tls forbidden" is consistent with what tonic 0.14 actually offers — there is no native-tls path in tonic 0.14 at all.
- Note: "RequireAndVerify" (memlog line 10) is Go gRPC terminology (`tls.RequireAndVerifyClientCert`), not a tonic API name. The behavior exists; the identifier does not. See finding F4.

### 2.2 tonic 0.14.6 UDS transport — ✅ capability confirmed; one memlog sub-claim uncorroborated

- `impl Connected for tokio::net::UnixStream` with `UdsConnectInfo { peer_addr, peer_cred }` exists in tonic 0.14.6 (`tonic/transport/server/unix.rs` on docs.rs latest). Server: `Server::builder().serve_with_incoming(UnixListenerStream)`. Client: `Endpoint::connect_with_connector(service_fn(|_| UnixStream::connect(...)))` — both shown in the current official examples. AD-4/AD-5's UDS gateway design is implementable exactly as written.
- **Not corroborated:** the memlog's "unix name resolver landed in 0.14.x". Current tonic 0.14.6 docs and examples still route UDS clients through `connect_with_connector` with a dummy URI; I found no `unix://` URI resolver in tonic's `Channel`/`Endpoint`. (Name-resolution work exists in the separate `grpc` crate being incubated in the same repo — a plausible conflation source.) See finding F2.

### 2.3 tonic-build 0.14.6 + prost 0.14.4 as the codegen pair — ❌ STALE (finding F1)

- The tonic **v0.14.0 release notes** (2025-07-28) state: *"Prost has been extracted to their own crates … anything that used prost has now been moved into either `tonic-prost` or `tonic-prost-build`."*
- Live dependency graphs confirm the split is total: **tonic-build 0.14.6 depends only on prettyplease/proc-macro2/quote/syn — no prost-build, not even optional.** It cannot compile `.proto` files via prost. **tonic 0.14.6 itself has no prost dependency** (its codec-runtime role moved to `tonic-prost`).
- The correct pair for the spine's AD-3: build-dep **`tonic-prost-build` 0.14.6** (deps: `prost-build ^0.14`, `prost-types ^0.14`, `tonic-build ^0.14.6`) and runtime dep **`tonic-prost` 0.14.6** ("Prost codec implementation for tonic"). The pinned prost 0.14.4 is version-compatible with both — the versions are right; the crate names are pre-0.14.
- The memlog's decision line ("tonic-build/prost codegen at build time") mirrors the tonic ≤0.13 architecture. This is precisely the trained-data-asserted-with-fresh-versions failure mode this lens exists to catch.

### 2.4 rcgen 0.14.10: CA + ECDSA P-256 leaf minting — ✅ confirmed

- docs.rs for rcgen 0.14.10: `CertificateParams::self_signed()` / `CertificateParams::signed_by()` (issuer-signed leafs), `IsCa` enum (CA certificates), `Issuer` / `CertifiedIssuer`, PEM serialization (`cert.pem()`, `signing_key.serialize_pem()`).
- `KeyPair::generate()` is documented as *"Generate a new random **PKCS_ECDSA_P256_SHA256** key pair"* — ECDSA P-256 is literally the default. AD-14's [ASSUMPTION] tag on P-256 is safely conservative; the mechanism is fully supported on both the `ring` and `aws_lc_rs` backends (features confirmed on the 0.14.10 crates.io record).

### 2.5 blake3 1.8.7 `rayon` feature (AD-19) — ✅ confirmed

Feature list of the exact published 1.8.7 version includes `rayon` (alongside `mmap`, `zeroize`, etc.). The `Hasher::update_rayon` whole-file-hashing plan is real.

### 2.6 rusqlite 0.40.2 (AD-8) — ✅ confirmed

Exists as pinned; feature list includes `bundled` (worth pinning explicitly in the workspace later, but nothing in the spine depends on the choice yet).

### 2.7 tracing JSON lines (AD-18) — ✅ confirmed (reviewer-verified, not memlog-evidenced)

tracing-subscriber 0.3.23's published feature list includes `json` — the "JSON lines when stderr is not a tty" convention is implementable with the pinned version. The memlog carries only the version, not this fit check; it happens to hold.

### 2.8 `similar` 3.2.0 diff crate — ✅ exists; fit asserted, not evidenced

3.2.0 is live (published 2026-08-17). Its fit for the "one core diff module" convention (unified text diffs over ini/xml/nginx) is sound — it is a general-purpose text-diffing library with unified-diff output — but no memlog line evidences that fit; it rides in the version list only. Low risk: the crate's purpose has been stable across major versions.

## 3. Findings

### F1 — HIGH — AD-3/Stack name the wrong codegen crates for tonic 0.14 (trained-data-stale claim)

AD-3 mandates ".proto files … generated by `tonic-build` at build time" and the Stack table pairs tonic-build 0.14.6 + prost 0.14.4. Under tonic 0.14 (per the official v0.14.0 release notes and live dependency graphs), prost codegen lives in **`tonic-prost-build`** and the runtime prost codec in **`tonic-prost`**; tonic-build 0.14.6 alone cannot compile protos and tonic 0.14.6 does not depend on prost. A builder following AD-3 literally hits a wall at first build, or worse, silently downgrades to a ≤0.13 tutorial stack. The memlog verified the version numbers but asserted the pre-0.14 codegen shape from training data.
**Fix:** amend AD-3 to "generated by `tonic-prost-build` at build time" and add `tonic-prost` / `tonic-prost-build` 0.14.6 rows to the Stack table (prost 0.14.4 stays — it satisfies their `^0.14` requirements). Not critical only because the error is loud and mechanically fixable at first `cargo build`.

### F2 — MEDIUM — "unix name resolver landed in 0.14.x" (memlog) is uncorroborated

The load-bearing UDS capability is confirmed (Connected impl for `UnixStream`, `serve_with_incoming`, `connect_with_connector`), but no `unix://` URI resolver exists in tonic 0.14.6's channel that I could find; current official examples still use the dummy-URI + connector pattern. A builder budgeting for `Endpoint::from_static("unix:///run/mediaops.sock")` will find it doesn't resolve.
**Fix:** strike or correct the memlog sub-claim; the `net` crate should plan on `connect_with_connector` for the CLI→home-daemon leg. (Likely conflation with resolver work in the incubating `grpc` crate in the tonic repo.)

### F3 — MEDIUM — Spine-mandated mechanisms with no pinned implementation vehicle in the Stack

Unevidenced, unpinned choices where two builders could diverge:

- **serde_json** — the Plan JSON artifact (AD-9), the `.partial.b3` JSON sidecar (AD-11), and the `--json` envelope (AD-18) all need it; it appears nowhere in Stack or memlog.
- **XDG dirs** (AD-7) — the spine hardcodes `~/.config/mediaops/` and `~/.local/state/mediaops/` and calls it XDG, but no crate (`etcetera`, `directories`, `xdg`) is named and honoring `$XDG_CONFIG_HOME`/`$XDG_STATE_HOME` overrides vs. literal tilde paths is undecided.
- **flock** (AD-4's machine-global flock) — needs `rustix`/`nix`/`fs4` or raw libc; unnamed.
- **cargo_metadata** (AD-2's CI dependency-rule test) — the crate that makes the test practical is unnamed.

None are version-risky; all are asserted mechanisms whose vehicle was never checked or fixed. **Fix:** one Stack addendum row each, or an explicit "builder's choice" note.

### F4 — LOW — "RequireAndVerify" is Go gRPC terminology, not the tonic API

The actual tonic 0.14 surface is `ServerTlsConfig::client_ca_root(...)` with `client_auth_optional(false)` as the default. Behavior identical to the claim; the identifier is borrowed from Go's `tls.RequireAndVerifyClientCert`. Cosmetic in the memlog, but a builder grepping tonic docs for "RequireAndVerify" finds nothing. **Fix:** memlog wording only.

### F5 — LOW — `similar` and tracing-JSON fit rode in on the version list without fit-evidence lines

Both check out on inspection (`json` feature present in tracing-subscriber 0.3.23; `similar` is a stable general text-diff crate), so this is a process note, not a defect: the memlog's evidence discipline covered versions and the tonic deep-dive but left these two fit claims implicit.

## 4. What was checked and found clean

- All 18 crate versions: exact live matches (§1), including post-training-cutoff versions (reqwest 0.13.x, toml 1.x, similar 3.x, rusqlite 0.40.x) that a trained-data table would have gotten wrong.
- Rust 1.98.0 / 2026-08-20 / edition 2024: exact.
- tonic 0.14.6 mTLS require-and-verify and UDS server+client: real and shaped as AD-4/AD-14 need.
- tokio-rustls 0.26.4 and rustls 0.23.43 sit inside tonic 0.14.6's declared ranges.
- rcgen 0.14.10 CA + ECDSA P-256 (default algorithm, both crypto backends).
- blake3 1.8.7 `rayon`, tracing-subscriber 0.3.23 `json`, rusqlite 0.40.2 `bundled`.
- prost 0.14.4 satisfies `tonic-prost`/`tonic-prost-build` 0.14.6's `^0.14` requirements — the version pins survive the F1 fix unchanged.
