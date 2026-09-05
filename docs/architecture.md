# Architecture

Two processes, one wire. The home filesystem is the catalog. The seedbox is dumb disk plus a pipe.

```
  home                                              seedbox
 ─────                                             ────────
  mediaops  ──unix socket──►  mediaopsd            mediaopsd
  (CLI)                       (gateway)  ──mTLS──► (gRPC)
                                 │                    │
                                 │                    ├── TransferService.GetRange
                                 │                    ├── ControlService.*  (*arr on 127.0.0.1)
                                 │                    └── allowlisted roots only
                                 └── seedbox_address
                                     from config.toml
```

The CLI never contains a seedbox address and never speaks *arr HTTP. SSH exists only for bootstrap (copy the binary, mint certs, write a user unit). After enroll, doctor / apply / copy do not use SSH. There is no rsync-ssh fallback, no FTP, no rclone pipe.

## Roles

**`mediaops`** — home CLI. Plan/apply, watch/why/status, hold inbox, reclaim, pull, encode, bootstrap. Encode and sqlite live here, not in the daemon.

**`mediaopsd --role home`** — unix-socket gateway. Owns the upstream address and the channel pool. Serves a small `GatewayService` (`ConfigurePool`, `PoolStatus`, `ProbeRange`) that the seedbox role does not.

**`mediaopsd --role seedbox`** — the only process that opens the WAN and the only process that opens grabber HTTP on localhost. Serves `TransferService` (`List`, `Stat`, `GetRange`) and `ControlService` (df, unmonitor, delete-remote, grab/edge apply, holds, …).

Wire contract: `proto/mediaops/v1/mediaops.proto`, package `mediaops.v1`. Handshake refuses an unknown package; minor version skew is a warning.

## Pull

One-way. Remote → `_incoming/…/*.partial` → per-range BLAKE3 in the sidecar → whole-file BLAKE3 → atomic install onto a schema path.

- A range is at most 64 MiB (`range_len_mib`, clamped).
- Many ranges in flight on one file; concurrency is `range_concurrency` or a probed plateau.
- Kill at 90% and run again: completed ranges stay; resume reads the sidecar's `range_len`, not current config.
- Size/mtime is not proof. Reclaim local-proof is the install digest plus the file on disk.
- Skip ≠ surplus. Skip means do not copy. Surplus means the remote may go after local proof.

## Library of record

Local FS wins. If *arr still thinks a file is missing while it exists here, the system unmonitors. `grabber = "none"` is a first-class path: no HTTP, no holds from *arr, still plan/run/encode.

Path grammar and `TitleId` are pure functions in `crates/core`. That crate is the only renderer of library paths. See [library layout](config.md#library-layout).

## Crate graph

Cargo workspace. Edges are allowlisted and tested in `crates/arch-tests` (`make test-arch`). Adding a workspace dependency means adding the edge there first.

| Crate | Role |
| ----- | ---- |
| `bins/mediaops` | CLI composition root |
| `bins/mediaopsd` | Daemon composition root |
| `crates/core` | `TitleId`, PathSchema, `config.toml`, Plan, jobs. No tokio, no tonic, no rusqlite. Only `walker` and `install` touch the filesystem, through caller-supplied roots |
| `crates/proto` | gRPC stubs from `proto/mediaops/v1/mediaops.proto`; the only wire↔domain conversions |
| `crates/store` | sqlite (`state.db`). The only crate that may depend on `rusqlite` |
| `crates/net` | mTLS, channel pool, seedbox + home serve |
| `crates/ssh` | Bootstrap exec only. No bulk copy |
| `crates/transfer` | Range pull, `.partial` resume, BLAKE3. The CLI's door into `net` |
| `crates/sync` | Planner + apply (including `grabber=none`) |
| `crates/encode` | EncodePolicy, ffprobe/ffmpeg. Linked only into the CLI |
| `crates/arr` | Grabber HTTP. Linked only into `mediaopsd`. The only crate that may depend on `reqwest` |
| `crates/arch-tests` | Dependency-graph and I/O-boundary law |

Banned as direct deps: `rsync`, `rclone`, `ftp`, `ssh2`, `russh`, `ffmpeg-next`, `native-tls`. `mediaopsd` must not reach `store` or `encode`.

## Lock and scheduler

`plan`, `run`, and `reclaim apply` take an exclusive flock (`mediaops.lock` next to `state.db`). Conflict is exit 3, never silent 0. `status` shows the holder.

The timer is systemd-user oneshot + `OnUnitInactiveSec` + that flock. It fires after the previous run finishes. Not overlapping `OnCalendar` cron. Config is not hot-reloaded mid-copy.

## What this is not

- A Jellyfin/Plex installer or plugin
- Two-way sync, a third-cloud archive, or a push path
- A public WebUI or a LAN bind of *arr
- Encode on the seedbox
- Auto-approve holds, auto-upgrade HD → UHD
- An in-process LLM
