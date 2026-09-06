# mediaops — agent notes

Home-disk library of record plus a disposable seedbox. Rust 1.98 Cargo workspace. Operator docs: `docs/`. Read those before inventing a flow.

Pre-rewrite files under `_bmad-output/` are historical, including session prompts and sprint/deferred-work queues. Do not dispatch them as current work. Read `docs/documentation-status.md` when using that history; scoped rules are in `_bmad-output/AGENTS.md`. Do not mention epics or story keys in commits, comments, or new docs.

## Do not

- Put grabber HTTP (`reqwest`, *arr clients) in the CLI or in any crate except `crates/arr` (linked only into `mediaopsd`).
- Put encode / sqlite in `mediaopsd`. `store` and `encode` must stay out of the daemon's workspace closure.
- Add FTP, rsync, rclone, `ssh2`, `russh`, `ffmpeg-next`, or `native-tls` as a dependency. Pull is Range RPC only.
- Use SSH for bulk copy. Existing seedbox bootstrap/upgrade and confirmed edge repair use exec-only maintenance in `crates/ssh`, never a media-copy transport.
- Format a library dest path by hand. `crates/core` PathSchema is the only writer (`parse` / `render`).
- Treat a path string as identity. Use `TitleId` (`kind:source:id`).
- Mint or write PEMs inside a git work tree. Bootstrap and `new-machine import` refuse; keep that.
- Enable `live-box`, SSH to a seedbox, or run NVENC from a test or Make target. Default `make test` is the gate.
- Add a TUI, a prompt, or an auto-approve path.
- Hot-reload `config.toml` mid-copy. Each Pull Job snapshots library root, budgets, and Range settings at create.

## Do

- Talk to the seedbox only through the home unix-socket gateway. The CLI never stores `seedbox_address` as something it dials.
- Keep `grabber = "none"` a valid path: schema folder on the box, disk at home, no *arr HTTP.
- Honor the exclusive flock on explicit CLI maintenance (manual pull, encode run, reclaim apply, library, new-machine, seedbox apply, edge repair). Lock conflict is exit 3, never silent 0. Unattended Home API copy concurrency is Job bind + Job status, not that flock.
- Home API `-o json` is the raw object. Legacy `--json` is one `{ok,data,error}` envelope on stdout; tracing on stderr. Human stdout is the operator UI — update the exact-screen test when you change a formatter.
- Add a new workspace Cargo edge to `crates/arch-tests` first (`ALLOWED_WORKSPACE_EDGES`). `make test-arch` enforces it.
- Keep `crates/core` free of tokio, tonic, and rusqlite. `std::fs` in that crate is legal only in `walker.rs` and `install.rs`.
- Discover *arr / SAB keys from the box config at runtime. Never commit them, never echo `********`.

## Where things are

- CLI verbs: `bins/mediaops/src/main.rs` (clap), one module per verb beside it. Home API client: `crates/home-client`
- Home control plane: `bins/mediaops-home` execs `mediaops-api` / `scheduler` / `gateway` / `inventory` / `pull`
- Seedbox serve: `bins/mediaopsd/src/main.rs` → `crates/net`
- Wire: `proto/mediaops/v1/mediaops.proto` (`mediaops.v1`) and `proto/mediaops/home/v1/home.proto` (`mediaops.home.v1`); conversions only in `crates/proto`. `make proto` is `buf lint` + `buf format --diff`
- Config parse: `crates/core/src/desired_state.rs` (file on disk is `config.toml`). Runtime truth is the Cluster object (`api.db`)
- Path grammar: `crates/core/src/pathschema.rs`
- Title ids: `crates/core/src/title_id.rs`
- Home API: `crates/api` + `crates/home-client`. Pull Jobs: `bins/mediaops-pull`
- Leftover planner helpers: `crates/sync`
- Range pull: `crates/transfer`
- Encode policy: `crates/encode/src/policy.rs` (hardcoded matrix, not a config field)
- Default paths: `bins/mediaops/src/bootstrap.rs` (`default_config_dir`, `default_state_db`, `default_socket`)

## Running and verifying

`Makefile` is the entry point (`make test`, `make test-arch`, `make clippy`, `make fmt`, `make proto`). CI is `.github/workflows/ci.yml`: `make proto`, `cargo test --locked --offline --workspace`, then `make musl`. Default `make test` stays Cargo-only.

Iterate on one crate with `cargo test -p <crate> --locked`. The live test (`cargo test -p mediaops --features live-box --test live`) is `#[ignore]` and still does not SSH or encode.

`make musl` needs `musl-gcc` and is not part of `make test`.

## Conventions that differ from defaults

- `--config` is `config.toml` (the code still names the value `desired_state` in a few clap fields). Do not revive `desired-state.toml`.
- Config sizes are `*_gib` / `*_mib` in TOML, `Bytes` in Rust. Deny unknown fields.
- Library paths have no `{tmdb-…}` tokens. Identity from a path is `movie:key:…` / `series:key:…` / `album:key:…`.
- Empty human states are fixed English strings: `nothing happening`, `nothing on hold`, `nothing on the box`, `nothing to reclaim`, `nothing to encode`.
- systemd-user control plane is always-on `mediaops-home.service` (`ExecStart=mediaops-home`). There is no `mediaops-run.timer`.
- Do not revive `mediaops plan` / `mediaops run` from leftover helpers or old docs. Use `watch` / `apply` to record Wants and `get Job` to inspect copying.

## Known pitfalls

- If `~/.config` is a git work tree, the active config dir is `~/.local/share/mediaops`. Tests and docs that hardcode `~/.config/mediaops` will miss PEMs and `config.toml`.
- A name passed to `watch` / `why` only resolves when the library, a job, the hold inbox, or a listing already knows it. Prefer a `TitleId` in scripts and tests.
- `hold approve` records a decision and does not install. The Hold controller creates a Pull Job after `Approved`.
- Resume reads `range_len` from the `.partial` sidecar, not from current `range_len_mib`.
