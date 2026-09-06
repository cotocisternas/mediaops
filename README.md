# mediaops

[![CI](https://github.com/cotocisternas/mediaops/actions/workflows/ci.yml/badge.svg)](https://github.com/cotocisternas/mediaops/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Home disk is the library. The seedbox is disposable buffer. The CLI talks to a local Home API over a unix socket. Range traffic goes through a local unix-socket gateway that holds the mTLS pool. Only `mediaopsd` on the box opens the WAN. Pull is one-way Range RPC, not FTP, rsync, or SSH copy.

`grabber=none` is a valid setup: a folder on the box, a disk at home. *arr is optional.

## Docs

| Page | For |
| ---- | --- |
| [Setup](docs/setup.md) | First install on a new box and a new library |
| [Usage](docs/usage.md) | Daily commands and what they print |
| [Config](docs/config.md) | `config.toml`, default paths, library layout |
| [Architecture](docs/architecture.md) | Two machines, the wire, the crate graph |
| [Development](docs/development.md) | Build, test, conventions |
| [AGENTS.md](AGENTS.md) | Rules for agents working in this repo |

## Install

Needs Rust **1.98** (`rust-toolchain.toml`), `protoc`, and a lockfile-aware Cargo. The static seedbox daemon also needs `musl-gcc` and `cmake`.

```bash
# Debian/Ubuntu
sudo apt-get install -y protobuf-compiler musl-tools cmake
# Arch
# sudo pacman -S protobuf musl cmake

make install
```

`make install` puts these on `~/.cargo/bin`: `mediaops`, `mediaopsd`, `mediaops-home`, `mediaops-api`, `mediaops-scheduler`, `mediaops-gateway`, `mediaops-inventory`, `mediaops-pull`.

`make help` lists the rest. Default `make test` never talks to a seedbox and never needs a GPU.

## Daily

```bash
mediaops status
mediaops hold list
mediaops get Job
mediaops why 'Mr Robot'
```

`mediaops-home` is the always-on supervisor. It execs api, scheduler, gateway, inventory, and pull. You apply a Want (`watch` or `apply -f`) and peek. There is no daily `plan` / `run`.

Quiet `status` means nothing in flight. Home API status is one library disk line (no separate home-disk row):

```
nothing happening

disk      693.1 GiB free
```

Work looks like:

```
want      The Matrix (1999)
pull      The Matrix (1999)  pulling

disk      693.1 GiB free
```

Home API `-o json` is the raw object (no envelope). Legacy `--json` is one `{ok,data,error}` envelope on stdout; tracing on stderr. Human stdout is the operator UI: color only on a tty, sizes as `7.1 GiB`, ages as `21m`.

See [Usage](docs/usage.md) for the rest of the verbs and [Setup](docs/setup.md) to bring up a new pair of machines.

## License

[MIT](LICENSE). See [CONTRIBUTING](CONTRIBUTING.md) and [SECURITY](SECURITY.md).
