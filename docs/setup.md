# Setup

Two machines: a home disk (library of record) and a seedbox (disposable buffer). SSH is used once, to enroll the box. After that, the CLI never SSHs and never dials the seedbox — it talks to a local unix-socket gateway.

Live bootstrap, a real pull, and NVENC spend WAN, disk, and GPU. They are not part of `make test` or CI. Run them on purpose.

## Prerequisites

- This repo built (`make install`) so `mediaops` and `mediaopsd` are on `PATH`
- `Host seedbox` in `~/.ssh/config` (the alias is fixed; do not invent another)
- A home disk with enough free space for `min_free_gib` plus whatever you will copy
- A port the box actually forwards. A rented Swizzin/SeedIt4Me box usually only forwards its per-user ports — pick a free one and pass it as `--address host:port`

## 1. Write `config.toml`

Create the config dir, then a file. Defaults:

| Preference | Path |
| ---------- | ---- |
| Override | `$MEDIAOPS_CONFIG_DIR` |
| Normal | `~/.config/mediaops` |
| If `~/.config` is a git work tree (dotfiles) | `~/.local/share/mediaops` |

Bootstrap refuses to mint TLS into a git work tree. Treat `config.toml` and `tls/` as secrets.

```toml
schema_version = 1
max_copy_gib = 80
min_free_gib = 256
range_len_mib = 32           # one Range RPC; the seedbox serves at most 64
range_concurrency = 8        # set it and the probe is skipped
max_nvenc = 3
lock = false
grabber = "none"             # or "servarr"
provider = "swizzin_box"
# seedbox_address is written by `seedbox bootstrap`, never typed

[[paths.roots]]
id = "movies"
path = "/home/you/media/movies"
kind = "movie"
[[paths.roots]]
id = "tv"
path = "/home/you/media/tv"
kind = "series"
[[paths.roots]]
id = "music"
path = "/home/you/media/music"
kind = "album"
```

`kind` is `movie` | `series` | `album`. Omit it on a mixed folder. Full field list: [Config](config.md).

`grabber = "none"` is enough for a first run: drop a schema-valid file on an allowlisted root, then plan/run. *arr is optional.

## 2. Enroll the seedbox

Without `--yes` this only prints the plan and refuses. With `--yes` it mints certs, copies a musl-static `mediaopsd`, writes a user unit, probes Range, and writes `seedbox_address` into the config.

```bash
mediaops seedbox bootstrap --provider swizzin_box --address box.example:25410 \
  --root movies=/home/you/media/movies \
  --root tv=/home/you/media/tv \
  --root music=/home/you/media/music

mediaops seedbox bootstrap --yes --provider swizzin_box --address box.example:25410 \
  --root movies=/home/you/media/movies \
  --root tv=/home/you/media/tv \
  --root music=/home/you/media/music
```

The daemon must bind the port you passed. Roots are an allowlist: walks never leave them, and never follow a symlink off them. Torrent save paths and `torrents/incomplete` are not roots.

## 3. Bootstrap the home library

```bash
mediaops library bootstrap --library-root /mnt/storage/videos
```

Creates `movies/`, `series/`, `music/`, `_ops/`, `_incoming/`, the sqlite index, the lock, an NVENC probe (prefers `/usr/lib/jellyfin-ffmpeg/ffmpeg`), and three systemd-user units:

- `mediaopsd-home.service` — unix-socket gateway
- `mediaops-run.service` / `mediaops-run.timer` — plan+apply after the previous run finishes (`OnUnitInactiveSec`)

`--enable-timer` also enables the home unit and the timer. Without it, start the gateway yourself:

```bash
systemctl --user daemon-reload
systemctl --user enable --now mediaopsd-home.service
```

Do not run an old `media-sync` timer alongside `mediaops-run.timer`. Both would pull the same files.

## 4. First copy

Put one schema-valid file on an allowlisted root (see [library layout](config.md#library-layout)). Then:

```bash
mediaops watch 'The Matrix'          # or movie:key:thematrix.1999
mediaops plan                        # exclusive lock; read-only
mediaops run                         # plan + apply in this process
mediaops why 'The Matrix'
```

`watch` records a want and exits. It does not wait for a playable file. `run` holds the exclusive lock; a conflict is exit 3, never a silent 0. Kill a copy mid-file and run again — resume uses `_incoming/…/*.partial` and its sidecar, not a restart.

`status`, `why`, `hold list`, `reclaim preview`, and `doctor` do not take the lock.

## 5. Optional: encode, timer, grabber

HEVC 10-bit MP4 movies (not HDR/DV, under 2160p) classify as NVENC → H.264 8-bit. Encode is home GPU only; the original moves under `_ops/backup-hevc-originals/` after a successful replace.

```bash
mediaops encode scan
mediaops encode run
```

When you trust the loop:

```bash
systemctl --user enable --now mediaops-run.timer
```

With `grabber = "servarr"`, the seedbox daemon speaks *arr on `127.0.0.1`. API keys are discovered from the box configs, never pasted. Import-blocked releases land in `mediaops hold list`.

## Later machines

```bash
mediaops new-machine export --out /path/to/bundle
# on the new home:
mediaops new-machine import --from /path/to/bundle --library-root /mnt/storage/videos
```

Exports `config.toml`, `tls/`, and the title-index. Import refuses a git work tree the same way bootstrap does.

## Check the edge

```bash
mediaops doctor
```

Prints `ok` when the nginx/Forms invariant holds. Write repair is a separate, confirmed command: `mediaops repair edge`.
