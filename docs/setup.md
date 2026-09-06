# Setup

Two machines: a home disk (library of record) and a seedbox (disposable buffer). Daily object commands talk to the local Home API; Range and Control traffic reach the box through the local gateway. The gateway owns the seedbox connection. SSH is reserved for explicit enrollment, daemon upgrade, and edge-repair maintenance, never bulk media copy.

Live bootstrap, a real pull, and NVENC spend WAN, disk, and GPU. They are not part of `make test` or CI. Run them on purpose.

## Prerequisites

- This repo built (`make install`) so the CLI, seedbox daemon, supervisor, and five home roles are on `PATH`
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
range_concurrency = 8        # snapshotted on each Home Pull Job; omitted means 1
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

`grabber = "none"` is enough for a first run: drop a schema-valid file on an allowlisted root, then `watch` / `apply` a Want. *arr is optional.

Expose only completed files: finish writing outside the allowlisted tree, then move them into place on the same filesystem. Do not modify a source in place while it is being copied.

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
mediaops library bootstrap --library-root /mnt/storage/videos --enable-timer
```

Creates `movies/`, `series/`, `music/`, `_ops/`, `_incoming/`, and systemd-user `mediaops-home.service` (supervisor for api / scheduler / gateway / inventory / pull).

The historical `--enable-timer` flag enables the always-on service; it does not create a timer. Bootstrap starts the API before publishing the Cluster and seedbox Secret. Without the flag, an API must already be running (for example, `mediaops-home` in another terminal).

To control the installed service later:

```bash
systemctl --user daemon-reload
systemctl --user enable --now mediaops-home.service
```

Import existing Wants, hold decisions, and installation proofs with `mediaops import-legacy`. Repeating the import fills missing objects and preserves newer runtime decisions and settings. `config.toml` remains an import/export format; runtime truth is the Cluster object and Title file observations.

When upgrading an existing installation, first stop any retired units that exist: `mediaops-run.timer`, `mediaops-run.service`, and `mediaopsd-home.service`. Use `systemctl --user disable --now UNIT` for each installed unit before starting `mediaops-home`. Removing an old unit file does not stop an already running process; the old gateway would still own its socket.

## 4. First copy

Put one schema-valid file on an allowlisted root (see [library layout](config.md#library-layout)). Then:

```bash
mediaops watch movie:key:thematrix.1999
mediaops reconcile
mediaops get Job -o wide
mediaops why movie:key:thematrix.1999
```

`watch` records a Want and exits. It does not wait for a playable file. Alternatively, use the [Want document example](usage.md#daily) with `apply -f`. The pull worker Ranges into `_incoming/…/*.partial`; resume uses the sidecar, not current Cluster `rangeLen`.

## 5. Optional: encode and grabber

HEVC 10-bit MP4 movies (not HDR/DV, under 2160p) classify as NVENC → H.264 8-bit. Encode is home GPU only; the original moves under `_ops/backup-hevc-originals/` after a successful replace.

```bash
mediaops encode scan
mediaops encode run
```

When you trust the loop:

```bash
systemctl --user enable --now mediaops-home.service
```

With `grabber = "servarr"`, the seedbox daemon speaks *arr on `127.0.0.1`. API keys are discovered from the box configs, never pasted. Import-blocked releases land in `mediaops hold list`.

## Later machines

```bash
mediaops new-machine export --out /path/to/bundle
# on the new home, with mediaops-home/API running and an empty Job list:
mediaops new-machine import --from /path/to/bundle --library-root /mnt/storage/videos
```

Exports `config.toml`, `tls/`, the title-index, and exact runtime `cluster.json` / `secret.json`. The bundle is private and contains credentials. Missing media on import is marked drifted; imported records alone never prove a disposable box copy is safe to reclaim. Import refuses a git work tree the same way bootstrap does.

The first import normally starts with no Titles. Retrying a compatible bundle is
supported: existing Title paths and installation digests must match the bundle,
and only missing observations are published. Newer current digests and drift
state are retained. Unrelated Titles or any existing Job, including a terminal
Job, refuse import. When Titles exist, the requested library root must match the
current Cluster. See [Usage](usage.md#command-map) for maintenance-lock recovery.

## Check the edge

```bash
mediaops doctor
```

Default `doctor` checks the edge, credentials, and PEM locations, then requires
the Home API and Ready scheduler, inventory, and pull Nodes. `ok` means all these
checks passed, not just that nginx is healthy.

Write repair is separate and explicit:

```bash
mediaops repair edge --repair --confirm
```

This uses SSH for nginx maintenance and the gateway Control API for edge apply
and verification. It is not a read-only health check and is never part of tests.
