# Development

Cargo workspace. Rust 1.98 (`rust-toolchain.toml`). `make` passes `--locked`.

## Versioning

The Home API rewrite is application version `0.2.0`. All workspace packages inherit
`workspace.package.version` from the root `Cargo.toml`; update that single value
and refresh `Cargo.lock` when bumping the release. Binaries report their package
version, so installed copies must be rebuilt and reinstalled to show a new version.
The `mediaops.v1` and `mediaops.home.v1` wire packages and config `schema_version`
are independent compatibility markers, not application release numbers.

## Requirements

- `protobuf-compiler` (`protoc`) — `crates/proto` builds the gRPC stubs
- [Buf](https://buf.build/docs/installation) — `make proto` runs `buf lint` and `buf format --diff`. Not required for `make test`
- For `make musl` (the static daemon the seedbox runs): `musl-tools` + `cmake` (`musl-gcc`) and `file`. Not needed for `make test`

```bash
sudo apt-get install -y protobuf-compiler musl-tools cmake   # Debian/Ubuntu
# sudo pacman -S protobuf musl cmake                          # Arch
```

## Make targets

```bash
make help          # this list
make fetch         # cargo fetch --locked (needed before OFFLINE=1)
make build         # debug workspace (symbols on)
make release       # optimized binaries in target/release/
make test          # build workspace, then default suite: no GPU, no seedbox, no live-box
make test-arch     # crate-graph / I/O-boundary law
make coverage      # cargo-llvm-cov summary (needs llvm-tools-preview)
make clippy
make fmt
make proto         # buf lint + format --diff (needs Buf; CI runs this)
make mediaops ARGS='--help'
make daemon  ARGS='--help'
make ci            # proto lint, fetch + test --offline --locked, then make musl (same as GitHub Actions)
make musl          # link musl-static mediaopsd (needs musl-gcc; not part of make test)
make install       # CLI, daemon, supervisor and five home roles into ~/.cargo/bin
```

`make test OFFLINE=1` adds `--offline` after a fetch. Default `make test` may download crates.

`make musl` checks the resulting binary with `file` and refuses a dynamic executable before bootstrap or upgrade can upload it. Build and verification both use `CARGO_TARGET_DIR` (default `target`); set that variable rather than passing `--target-dir` in `CARGO_FLAGS`. The musl target uses static relocation so `musl-gcc` wrappers that do not handle `-static-pie` cannot silently leave a dependency on a remote musl loader.

Do not put `seedbox bootstrap --yes`, a real pull, or NVENC in a Make target. Those are live-box steps; see [Setup](setup.md).

## Tests

Default CI and `make test` never enable `live-box`, never talk to the box, and never need a GPU.

```bash
make test
make test-arch
make coverage      # cargo install cargo-llvm-cov && rustup component add llvm-tools-preview
cargo test -p mediaops --features live-box --offline --test live
```

The live test is `#[ignore]` and still does not SSH or encode. `MEDIAOPS_LIVE=1` is a second gate; turning it on without operator confirm still does not dial a rented box.

`crates/arr` talks to *arr through cassettes in `fixtures/arr`. Do not add a live HTTP client to a default test.

Human stdout is the operator UI. Changing a formatter means adding or updating an exact-screen test next to it.

Home API `-o json` is the raw object. Legacy `--json` envelopes stay stable unless the change needs a new field.

## Conventions

- [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, `perf:`, `style:`. Imperative, lowercase after the type.
- Thick libraries, thin binaries. Grabber HTTP stays in `mediaopsd` / `crates/arr`. Encode stays in leftover CLI verbs / `crates/encode`. Only `mediaops-api` opens `api.db`.
- New workspace Cargo edges belong in `crates/arch-tests` first. `make test-arch` fails otherwise.
- `core` stays free of tokio, tonic, and rusqlite. New `std::fs` in `crates/core/src` is only legal in `walker.rs` and `install.rs`.
- PathSchema (`crates/core/src/pathschema.rs`) is the only writer of library paths. Do not format a dest path by hand.
- Identity is a `TitleId`, never a path string.
- No TUI, no prompts, no auto-approve.

## Layout

```
bins/mediaops            CLI (Home API client)
bins/mediaops-home       supervisor (execs the five roles)
bins/mediaops-api        apiserver
bins/mediaops-scheduler  Job bind
bins/mediaops-gateway    mTLS attach
bins/mediaops-inventory  RemoteFile / Hold
bins/mediaops-pull       Range pull worker
bins/mediaopsd           seedbox daemon
crates/core              TitleId, PathSchema, Home objects, config.toml
crates/proto             gRPC (mediaops.v1 + mediaops.home.v1)
crates/store             sqlite (state.db + api.db). Only mediaops-api opens api.db
crates/home-client       typed Home API client
crates/api               serve, admission, reconcilers
crates/net               mTLS, channels, seedbox + gateway
crates/ssh               bootstrap exec only
crates/transfer          Range pull, .partial resume, BLAKE3
crates/sync              leftover planner helpers + unit text
crates/encode     EncodePolicy, ffprobe/ffmpeg
crates/arr        grabber HTTP (daemon only)
crates/arch-tests dependency-graph law
proto/            mediaops/v1/mediaops.proto (`mediaops.v1`) and mediaops/home/v1/home.proto (`mediaops.home.v1`)
docs/             this documentation
```

`_bmad-output/` is historical planning. Do not treat it as current docs or as the product contract.

## What is not in the tree

No TUI. No `docs render`. No in-process agent. Those are out of scope until they exist.
