# First demo on this box — Epic 4 runbook

**Status:** historical; superseded by the Home API rewrite.
Do **not** execute this old runbook, even as the current live-test gate. Use
[Setup](../../docs/setup.md) and [Development](../../docs/development.md) for
current instructions. The original demonstration steps below are retained as
history; default CI does not execute them.

**Grabber:** `none` (a schema-valid folder on the seedbox, a disk at home).

---

## Prerequisites (read-only)

Confirm these exist before asking to go live. Checking them is not destructive.

| Item | Where |
| --- | --- |
| SSH Host `seedbox` | `~/.ssh/config` |
| Desired-state | `~/.config/mediaops/desired-state.toml` (`grabber` omitted or `none`, `lock = false`) |
| Home disk | library root from `mediaops library bootstrap` (`movies` / `series` / `music` / `_ops` / `_incoming`) |
| Ada GPU | home box; NVENC cap is probed at library bootstrap (`ffmpeg -encoders` → 0/1) |
| Toolchain | Rust 1.98, this repo on `epic/4-tonight-playable` (or merged `main`) |

Home CLI talks **only** to the home unix-socket gateway. It never contains a seedbox address. The home daemon owns `--upstream` / `seedbox_address`.

---

## Destructive / live list (must be confirmed)

Do not run any of these until the operator explicitly confirms. Each step is irreversible or spends real bytes / GPU time.

1. **`mediaops seedbox bootstrap --yes`**
   - musl-static `mediaopsd` build (`x86_64-unknown-linux-musl`; aws-lc link is still unproven — deferred)
   - `scp` of the binary + systemd --user unit to Host `seedbox`
   - systemd --user enable of seedbox `mediaopsd`
   - mTLS cert mint into the config dir (**refuses if that dir is a git work tree**)
2. **Range probe** against the live box (writes `probes` in `state.db`)
3. **Pull of real bytes** (`plan` / `run` Copy, or manual `pull`) — WAN + disk
4. **Encode replace** of a library file — original moves to `_ops/backup-hevc-originals/<staging_token>/<filename>`

Also do **not** without confirm:

- `mediaops library bootstrap --enable-timer` against production disk
- any NVENC encode on the Ada GPU

---

## After confirm — ordered demo

Live execution pending operator confirm. When that confirm lands, run in this order.

### 1. Home gateway

`library bootstrap` already writes `mediaopsd-home.service` (`Type=simple`, `Restart=on-failure`, `ExecStart=mediaopsd serve --role home --tls-dir … --desired-state …`). It is **not** enabled unless `--enable-timer` was passed.

Start it:

```bash
systemctl --user start mediaopsd-home.service
# or:
mediaopsd serve --role home --tls-dir ~/.config/mediaops/tls \
  --desired-state ~/.config/mediaops/desired-state.toml
```

Seedbox `mediaopsd` must already bind gRPC/mTLS (bootstrap --yes).

### 2. Library (if not already)

```bash
mediaops library bootstrap --library-root /path/to/library --json
```

Do not pass `--enable-timer` unless confirmed.

### 3. Place one schema-valid HEVC-MP4 movie on an allowlisted seedbox root

Folder on the box — **not** torrent save paths, not `torrents/incomplete`. PathSchema file shape, for example:

```text
movies/The.Matrix.(1999).{tmdb-603}/The.Matrix.(1999).mp4
```

HEVC 10-bit MP4, not HDR/DV, height < 2160. That is the Chrome-dropped-frames encode target.

### 4. Enqueue and plan

```bash
mediaops watch movie:tmdb:603 --json
mediaops plan --json
```

Expect a `Copy` (music-first does not apply to a lone movie). Installed titles are `Skip { reason: upgrade_never }` — auto-upgrade 1080p → 4k remux because disk is bored never happens.

### 5. Apply, kill, resume

```bash
mediaops run --json
```

Kill the process at ~90% of the file. `_incoming/<title>/….partial` + `.partial.b3` must remain. Run again:

```bash
mediaops run --json
```

Resume uses the sidecar `range_len`, not current config. Completed ranges are skipped.

### 6. Confirm install

- Schema path under `movies/…` (spaces and leftover scene tags would have failed PathSchema)
- `title_index` row: `install_b3` = whole-file BLAKE3, `path` = schema-relative dest
- `why movie:tmdb:603 --json` / `status --json`: pull Installed, watermark (free vs min_free), lock holder if any, encode queue

### 7. Encode

If the file classified `NvencH264` (movie + HEVC + 10-bit + MP4 + not HDR):

```bash
mediaops encode run movie:tmdb:603 --json
```

Confirm:

- live file is H.264 8-bit (`yuv420p`)
- original under `_ops/backup-hevc-originals/movie-tmdb-603/`
- `current_b3` updated; `install_b3` unchanged

HDR/DV/2160p is Refuse (exit 5 on `encode run TITLE` only). Inside `run`, encode refuse is data (AD-17). Series HEVC-MP4 is Keep (named series-skip).

`encode pause` / `encode pause --off` is machine kv `encode_pause`, polled between jobs, never a signal to the lock holder.

### 8. Success bar

Beat **“need FTP-PASV”** as the transfer path. This runbook does **not** claim a published MiB/s SLA, and CI must not. A measured live throughput number is only valid after a confirmed live run.

---

## Test gate (AD-20)

```bash
cargo test --workspace          # default: no live-box, no GPU, no network
cargo test -p mediaops-arch-tests
```

Live tests compile only with:

```bash
cargo test -p mediaops --features live-box
```

and still no-op unless `MEDIAOPS_LIVE=1`. They **do not** SSH or encode. Turning the env on without operator confirm still does not talk to SeedIt4Me.

---

## Out of this demo

arr HTTP, holds inbox, reclaim, doctor/repair, `docs render`, relocate, TUI, agents, GrabApply/EdgeApply apply, musl-static aws-lc proof, GetRange streaming disk pipe.
