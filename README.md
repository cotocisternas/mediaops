# mediaops

Home-disk library of record plus a disposable seedbox. The CLI (`mediaops`) talks only to a local `mediaopsd` over a unix socket. The seedbox daemon is the only process that opens the WAN. Pull is one-way Range RPC (not FTP, rsync, or SSH copy). `grabber=None` is a valid path: a schema folder on the box, a disk at home.

This repo is a Cargo workspace. The product contract lives under `_bmad-output/specs/spec-mediaops/`. This file is how to build and run it.

Story commits put the story key in the subject as `N-M` (hyphen, e.g. `2-1`), so `git_evidence.py --stories` can attribute. Do not rewrite published commits to backfill old subjects.

## Requirements

- Rust **1.98** (`rust-toolchain.toml` pins it)
- `protobuf-compiler` (`protoc`) — `crates/proto` builds the gRPC stubs
- A lockfile-aware Cargo (`make` passes `--locked`)
- `musl-tools` + `cmake` — only for `make musl` (not `make test`)

On Debian/Arch:

```bash
sudo apt-get install -y protobuf-compiler   # Debian/Ubuntu
# sudo pacman -S protobuf                    # Arch
# make musl only:
sudo apt-get install -y musl-tools cmake    # Debian/Ubuntu (`musl-gcc`)
# sudo pacman -S musl cmake                  # Arch (may ship x86_64-linux-musl-gcc as an alias)
```

## Make targets

```bash
make help          # this list
make fetch         # cargo fetch --locked (needed before OFFLINE=1)
make build         # debug workspace (symbols on)
make release       # optimized binaries in target/release/
make test          # default suite: no GPU, no seedbox, no network feature
make test-arch     # crate-graph / I/O-boundary law
make coverage      # cargo-llvm-cov summary (needs llvm-tools-preview)
make clippy
make fmt
make mediaops ARGS='--help'
make daemon  ARGS='--help'
make ci            # fetch + test --offline --locked, then make musl (same as GitHub Actions)
make musl          # link musl-static mediaopsd (needs musl-gcc + cmake; not part of make test)
make install       # both binaries into ~/.cargo/bin
```

`make test OFFLINE=1` adds `--offline` after a fetch. Default `make test` may download crates.

Do **not** put `seedbox bootstrap --yes`, a real pull, or NVENC in a Make target. Those are live-box steps; see [First demo](#first-demo).

## Binaries

| Binary     | Role |
| ---------- | ---- |
| `mediaops` | Home CLI. Plan/apply, watch/why/status, hold inbox, reclaim, pull, encode, bootstrap. |
| `mediaopsd` | Daemon. Seedbox: gRPC + mTLS on TCP. Home: unix-socket gateway to the seedbox. |

```bash
make build
./target/debug/mediaops --help
./target/debug/mediaopsd --help
```

`--json` on every verb prints one `{ok,data,error}` envelope on stdout. Tracing goes to stderr.

### CLI verbs (`mediaops`)

| Command | What it does |
| ------- | ------------ |
| `seedbox bootstrap` | SSH Host `seedbox`: copy daemon, mint certs, probe Range N. Needs `--yes` to apply. |
| `library bootstrap` | Schema dirs, sqlite, lock, systemd-user units, NVENC probe. `--enable-timer` also enables the run timer and home unit. |
| `list` / `pull` | List remotes / pull one file through the home socket. |
| `watch TITLE` | Record a want (`kind:source:id`). Exits 0; does not wait for playable. |
| `plan` / `run` | Exclusive lock. `run` is plan + apply in this process. Lock conflict is exit 3, never silent 0. Approved holds become Copy on this path. |
| `why TITLE` / `status` | Lock-free peek. Local FS is truth. |
| `reclaim preview\|apply` | Ranked surplus dry-run (lock-free); exclusive unlink after `install_b3` plus the library file. |
| `hold list\|approve\|reject` | Lock-free import-blocked inbox. Approve records a decision (does not install). Reject is never-this-release. |
| `doctor` / `repair edge` | Read-only EdgeInvariant vs confirmed nginx + API repair. |
| `seedbox apply\|upgrade` | Grabber set-diff apply; re-copy musl `mediaopsd` and restart. |
| `encode scan\|run\|pause` | Home GPU only. Not linked into `mediaopsd`. |

`TITLE` is `movie:tmdb:…`, `series:tvdb:…`, or `album:mbid:…`.

### Daemon (`mediaopsd serve`)

```bash
# seedbox (default bind 0.0.0.0:50051)
mediaopsd serve --role seedbox --tls-dir ~/.config/mediaops/tls --root media=/data/media

# home gateway (unix socket; seedbox address comes from desired-state, not the CLI)
mediaopsd serve --role home --tls-dir ~/.config/mediaops/tls \
  --desired-state ~/.config/mediaops/desired-state.toml
```

## Default paths

| What | Where |
| ---- | ----- |
| Desired-state | `~/.config/mediaops/desired-state.toml` |
| mTLS PEMs | `~/.config/mediaops/tls/` (gitignored; never commit) |
| sqlite + lock | `~/.local/state/mediaops/state.db` |
| Plan artifacts | `~/.local/state/mediaops/plans/` |
| Home socket | `$XDG_RUNTIME_DIR/mediaopsd.sock` |

Desired-state is deny-unknown-fields TOML. Sizes are `*_gib` / `*_mib` in the file and bytes in code.

## Tests

Default CI and `make test` never enable `live-box`, never talk to the box, and never need a GPU.

```bash
make test
make test-arch
make coverage      # needs `cargo install cargo-llvm-cov` and `rustup component add llvm-tools-preview`
cargo test -p mediaops --features live-box --offline --test live
```

The live test is `#[ignore]` and still does not SSH or encode. `MEDIAOPS_LIVE=1` is a second gate; turning it on without operator confirm still does not dial SeedIt4Me.

## First demo

Live bootstrap, pull, and NVENC are **not** automatic. The ordered runbook, including the destructive list that needs an explicit yes, is:

[`_bmad-output/implementation-artifacts/demo-epic-4.md`](_bmad-output/implementation-artifacts/demo-epic-4.md)

## Layout

```
bins/mediaops     CLI composition root
bins/mediaopsd    daemon composition root
crates/core       TitleId, PathSchema, desired-state, Plan, jobs (no I/O)
crates/proto      gRPC / prost (built from proto/mediaops.proto)
crates/store      sqlite
crates/net        mTLS, channels, seedbox + home serve
crates/ssh        bootstrap exec only
crates/transfer   Range pull, .partial resume, BLAKE3
crates/sync       grabber=None planner + apply
crates/encode     EncodePolicy, ffprobe/ffmpeg via ExecPort
crates/arr        grabber HTTP (daemon only; cassettes in fixtures/arr)
crates/arch-tests dependency-graph law
```

## Specs

- [`_bmad-output/specs/spec-mediaops/SPEC.md`](_bmad-output/specs/spec-mediaops/SPEC.md) — capabilities and constraints
- [`_bmad-output/planning-artifacts/architecture/architecture-mediaops-2026-08-29/ARCHITECTURE-SPINE.md`](_bmad-output/planning-artifacts/architecture/architecture-mediaops-2026-08-29/ARCHITECTURE-SPINE.md) — crate graph and ADs

Not in this tree yet: `library relocate` / `new-machine`, `docs render`, TUI.
