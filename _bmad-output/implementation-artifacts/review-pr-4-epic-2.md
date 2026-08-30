# Review findings — PR #4 (Epic 2)

Source: `main...epic/2-seedbox-answers` (`d2d93318`), SPEC-mediaops + epics 2.1–2.3.
Layers: blind-hunter, edge-case-hunter, verification-gap, acceptance-auditor (split A net / B seedbox role / C bootstrap / D manifests).
Verification-gap on the manifests chunk returned empty; other chunks of that layer produced findings.

SPEC.md has no Tasks/Subtasks section; this file is the persisted review list.

### Review Findings

- [x] [Review][Decision] Seedbox `--bind` defaults to loopback — resolved 2026-08-30: default `--bind 0.0.0.0:50051` (promoted to patch).
- [x] [Review][Decision] Git ancestor walk can block `~/.config/mediaops` — resolved 2026-08-30: keep ancestor walk (dotfiles git blocks XDG). No code change.
- [x] [Review][Decision] Bootstrap probe runs in the CLI process — resolved 2026-08-30: keep CLI-side probe for Epic 2; home gateway move stays deferred to Epic 3.

- [x] [Review][Patch] Default seedbox `--bind` is `0.0.0.0:50051` (was loopback) [`bins/mediaopsd/src/main.rs:33`]
- [x] [Review][Patch] Channel pool does not pin one in-flight GetRange per TCP+TLS channel [`crates/net/src/lib.rs:91`]
- [x] [Review][Patch] Stat/GetRange follow directory symlinks off the allowlist [`crates/core/src/walker.rs:297`]
- [x] [Review][Patch] GetRange allocates `len` unbounded and treats a short `read` as success [`crates/net/src/seedbox.rs:155`]
- [x] [Review][Patch] TLS handshake and connect have no timeout; accept errors tear down the daemon [`crates/net/src/listen.rs:28`]
- [x] [Review][Patch] TLS private keys written with default umask; no `0700` on the tls dir [`crates/net/src/mint.rs:105`]
- [x] [Review][Patch] Swizzin install scps a stub path, not the musl artifact; does not place certs, mkdir, enable the unit, or pass serve flags [`bins/mediaops/src/bootstrap.rs:103`]
- [x] [Review][Patch] `--yes` always remints a new CA (second bootstrap is not a no-op) [`bins/mediaops/src/bootstrap.rs:91`]
- [x] [Review][Patch] Probe `--address` defaults to `127.0.0.1:50051` and ignores imported `Host seedbox` [`bins/mediaops/src/main.rs:51`]
- [x] [Review][Patch] `--yes` gate does not emit the BootstrapReport as JSON; git-work-tree refusal is exit 1 Runtime [`bins/mediaops/src/main.rs:205`]
- [x] [Review][Patch] `UnderlayMode::WireGuard` serdes as `wire_guard` but fingerprints as `wireguard` [`crates/core/src/probe.rs:7`]
- [x] [Review][Patch] New workspace crates are unpinned caret ranges [`Cargo.toml:48`]
- [x] [Review][Patch] `state.db` uses `~/.local/share` not AD-7 `~/.local/state`; rusqlite runs on the async worker [`bins/mediaops/src/bootstrap.rs:205`]
- [x] [Review][Patch] `reject_bulk_copy` misses combined scp flags; `Host seedbox other` is not imported [`crates/core/src/exec.rs:48`]
- [x] [Review][Patch] Home role is labeled a designed-unused mode (only reverse-connect is) [`crates/net/src/lib.rs:56`]
- [x] [Review][Patch] Seedbox `from_dir` requires the client private key [`crates/net/src/mint.rs:133`]
- [x] [Review][Patch] list/stat/df block the tonic worker [`crates/net/src/seedbox.rs:126`]
- [x] [Review][Patch] `bootstrap` hardcodes `SystemExec`, takes no flock, and is untested as an ExecPort transcript [`bins/mediaops/src/bootstrap.rs:57`]
- [x] [Review][Patch] Cached probe is reused even without `--skip-probe`; `--skip-probe` with no row silently uses N=1; live sweep caps at 4 [`bins/mediaops/src/bootstrap.rs:111`]
- [x] [Review][Patch] Tests do not observe P-256/DER fingerprints, mandatory client certs, N distinct TCP, write_to_dir, probe_range_n, mediaopsd serve, Stat, or applied bootstrap [`crates/net/src/lib.rs:175`]

- [x] [Review][Defer] Home gateway owns the WAN pool — deferred, pre-existing (Epic 3)
- [x] [Review][Defer] `store` repository traits in `core` — deferred, pre-existing (1.3 split)
- [x] [Review][Defer] OpenSSH `Include` / `Host *` / `Match` — deferred, pre-existing
- [x] [Review][Defer] GetRange streaming disk pipe — deferred, pre-existing
- [x] [Review][Defer] Concurrent TLS accept — deferred, pre-existing
- [x] [Review][Defer] Comment-preserving desired-state splice — deferred, pre-existing
- [x] [Review][Defer] `ControlPort::df` drops semver — deferred, pre-existing (Epic 1 trait)
- [x] [Review][Defer] musl-static aws-lc / `.cargo/config.toml` — deferred, pre-existing
