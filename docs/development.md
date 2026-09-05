# Development

Cargo workspace. Rust 1.98 (`rust-toolchain.toml`). `make` passes `--locked`.

## Requirements

- `protobuf-compiler` (`protoc`) — `crates/proto` builds the gRPC stubs
- [Buf](https://buf.build/docs/installation) — `make proto` runs `buf lint` and `buf format --diff`. Not required for `make test`
- For `make musl` (the static daemon the seedbox runs): `musl-tools` + `cmake` (`musl-gcc`). Not needed for `make test`

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
make test          # default suite: no GPU, no seedbox, no network feature
make test-arch     # crate-graph / I/O-boundary law
make coverage      # cargo-llvm-cov summary (needs llvm-tools-preview)
make clippy
make fmt
make proto         # buf lint + format --diff (needs Buf; CI runs this)
make mediaops ARGS='--help'
make daemon  ARGS='--help'
make ci            # proto lint, fetch + test --offline --locked, then make musl (same as GitHub Actions)
make musl          # link musl-static mediaopsd (needs musl-gcc; not part of make test)
make install       # both binaries into ~/.cargo/bin
```

`make test OFFLINE=1` adds `--offline` after a fetch. Default `make test` may download crates.

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

`--json` envelopes and plan JSON stay stable unless the change needs a new field.

## Conventions

- [Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`, `perf:`, `style:`. Imperative, lowercase after the type.
- Thick libraries, thin binaries. Grabber HTTP stays in `mediaopsd` / `crates/arr`. Encode stays in the CLI / `crates/encode`.
- New workspace Cargo edges belong in `crates/arch-tests` first. `make test-arch` fails otherwise.
- `core` stays free of tokio, tonic, and rusqlite. New `std::fs` in `crates/core/src` is only legal in `walker.rs` and `install.rs`.
- PathSchema (`crates/core/src/pathschema.rs`) is the only writer of library paths. Do not format a dest path by hand.
- Identity is a `TitleId`, never a path string.
- No TUI, no prompts, no auto-approve.

## Layout

```
bins/mediaops     CLI composition root
bins/mediaopsd    daemon composition root
crates/core       TitleId, PathSchema, config.toml, Plan, jobs
crates/proto      gRPC / prost (built from proto/mediaops/v1/mediaops.proto)
crates/store      sqlite
crates/net        mTLS, channels, seedbox + home serve
crates/ssh        bootstrap exec only
crates/transfer   Range pull, .partial resume, BLAKE3
crates/sync       planner + apply
crates/encode     EncodePolicy, ffprobe/ffmpeg
crates/arr        grabber HTTP (daemon only)
crates/arch-tests dependency-graph law
proto/            mediaops/v1/mediaops.proto (package mediaops.v1)
docs/             this documentation
```

`_bmad-output/` is historical planning. Do not treat it as current docs or as the product contract.

## What is not in the tree

No TUI. No `docs render`. No in-process agent. Those are out of scope until they exist.
