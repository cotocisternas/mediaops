# Usage

After [setup](setup.md), type these. The timer runs `mediaops run`; you peek with the rest.

`why` and `watch` take a spoken name (`Hearts`, `Mr Robot`) or a title id. A name only resolves when the library, a job, the hold inbox, or a listing already knows it. `hold approve` / `reject` take the id from `hold list` (`movie:tmdb:4539`).

`--json` on every verb prints one `{ok,data,error}` envelope on stdout. Tracing goes to stderr.

## Daily

```bash
mediaops status
mediaops hold list
mediaops plan
mediaops run
mediaops why 'Mr Robot'
```

### `status`

Lock-free. Quiet means nothing in flight:

```
nothing happening

disk      693.1 GiB free
home      3.8 TiB free
```

Work looks like:

```
want      The Matrix (1999)
pull      The Matrix (1999)  pulling

disk      693.1 GiB free
```

### `hold list`

Inbox of import-blocked releases. The id on the second line is what you approve or reject:

```
1.  Hearts of Darkness A Filmmaker's Apocalypse (1991)  7.1 GiB  75m
    movie:tmdb:4539
    Found matching movie via grab history, but release was matched to movie by ID. Manual Import required.
    Hearts.of.Darkness-Reise.ins.Herz.der.Finsternis.GERMAN.DL.DOKU.1080P.BLURAY.X264-WATCHABLE

approve   mediaops hold approve movie:tmdb:4539
```

```bash
mediaops hold approve movie:tmdb:4539   # records a decision; does not install
mediaops hold reject movie:tmdb:4539    # never this release
```

```
approved  Hearts of Darkness A Filmmaker's Apocalypse (1991)
          movie:tmdb:4539
```

Approve does not copy. The next exclusive `run` will. There is no auto-approve.

### `plan` / `run`

Exclusive lock. One line per copy / skip / review. Reconciler bookkeeping is hidden. Empty is `nothing to copy`.

```
copy      Mr Robot (2015) S01E02  4.0 GiB
copy      Mr Robot (2015) S04E01  2.6 GiB
review    usenet_tv / _UNPACK_Mr.Robot.S02…  unparseable

/home/you/.local/state/mediaops/plans/….json
```

`run` is plan then apply of that artifact in this process. Lock conflict is exit 3. A long pull prints progress on stderr (tty only):

```
pull    Mr Robot (2015) S01E02  1.2 GiB / 4.0 GiB
```

When it finishes:

```
copied    Mr Robot (2015) S01E02
copied    Mr Robot (2015) S04E01
```

Music copies before video, under the same `max_copy_gib` cap. A copy that would breach `min_free_gib` is skipped, not partial-written onto a full disk. Already-installed titles are skipped; there is no auto-upgrade from 1080p to a 4K remux.

### `why`

Lock-free. Headline, then id, then only the facts that exist:

```
$ mediaops why Hearts
Hearts of Darkness A Filmmaker's Apocalypse (1991)
movie:tmdb:4539

hold      Found matching movie via grab history, but release was matched to movie by ID. Manual Import required.  7.1 GiB
grab      wanted, not on the box

$ mediaops why 'Mr Robot'
Mr Robot (2015)
series:key:mrrobot.2015

grab      wanted, not on the box
import    Mr Robot (2015) S01E02

$ mediaops why series:key:foundation.2021
Foundation (2021)
series:key:foundation.2021

quiet
```

## Occasional

```bash
mediaops list
mediaops watch 'Mr Robot'                   # or series:key:mrrobot.2015
mediaops pull --root tv --path 'Mr.Robot.(2015)/Season.01/Mr.Robot.(2015).S01E02.mkv' \
  --title-id series:key:mrrobot.2015 --name 'Mr.Robot.(2015).S01E02.mkv' --install
mediaops reclaim preview
mediaops reclaim apply
mediaops encode scan
mediaops encode run
mediaops doctor
```

### `list`

Files on the box, grouped by root. Empty is `nothing on the box`.

```
tv
   4.0 GiB  Mr.Robot.(2015)/Season.01/Mr.Robot.(2015).S01E02.eps1.1_ones-and-zer0es.mpeg.mkv

usenet_movies
   6.8 GiB  Hearts.of.Darkness-…WATCHABLE.mkv
```

### `watch`

Records a want and exits 0. It does not wait for playable.

```
watching  Foundation (2021)
          series:key:foundation.2021
```

### `reclaim`

`preview` is lock-free. `apply` is exclusive and unlinks box copies that are proved installed at home (index row + file on disk) and not seeding. Empty is `nothing to reclaim`.

```
reclaim   The Matrix (1999)  6.8 GiB
          seedbox / movies/The.Matrix.(1999)/The.Matrix.(1999).mkv
          ratio 2.1  public
```

Usenet-complete is deletable after a successful copy. Torrents stay while they seed; torrent delete is reclaim only. `--max N` caps how many ranked candidates `apply` takes.

### `encode`

Home GPU only. Not linked into `mediaopsd`. Empty is `nothing to encode`.

```
encode    The Matrix (1999)
```

`encode pause` / `encode pause --off` sets a machine flag polled between jobs. It is not a signal to a running ffmpeg.

Policy (hardcoded, not a config field):

| Input | Decision |
| ----- | -------- |
| HDR, Dolby Vision, or height ≥ 2160 | keep forever (`Refuse` on `encode run TITLE` is exit 5) |
| Series + HEVC + MP4 | keep |
| Movie + HEVC + 10-bit + MP4 + not HDR | NVENC → H.264 8-bit (`yuv420p`) |
| Already H.264 8-bit | keep |

The live file is replaced only after the convert succeeds. The original moves to `_ops/backup-hevc-originals/…`. `install_b3` (the pull digest) stays; `current_b3` updates.

### `doctor` / `repair edge`

`doctor` is read-only: edge invariant, key presence, PEM-in-git scan. Prints `ok` when the invariant holds. `repair edge` is a confirmed nginx + API write.

## Command map

| Command | What it does |
| ------- | ------------ |
| `seedbox bootstrap` | SSH Host `seedbox`: copy daemon, mint certs, probe Range. Needs `--yes` to apply. |
| `seedbox apply` | Grabber set-diff from `config.toml`. Empty `[grab]` is a no-op, never a wipe. |
| `seedbox upgrade` | Re-copy musl `mediaopsd` and restart. Needs `--yes`. |
| `library bootstrap` | Schema dirs, sqlite, lock, systemd-user units, NVENC probe. |
| `library relocate` | Retarget `library_root` and units. Does not copy media. |
| `library reindex` | Rebuild title-index proof by hashing on-disk schema files. |
| `list` / `pull` | List remotes / pull one file through the home socket. |
| `watch TITLE` | Record a want. Exits 0; does not wait for playable. |
| `plan` / `run` | Exclusive lock. `run` is plan + apply. Lock conflict is exit 3. |
| `why TITLE` / `status` | Lock-free peek. Local FS is truth. |
| `reclaim preview\|apply` | Ranked surplus dry-run; exclusive unlink after local proof. |
| `hold list\|approve\|reject` | Lock-free import-blocked inbox. |
| `encode scan\|run\|pause` | Home GPU only. |
| `doctor` / `repair edge` | Read-only check vs confirmed edge write. |
| `new-machine export\|import` | Bundle `config.toml` + `tls/` + title-index. |

`TITLE` is `movie:key:<title>.<year>`, `series:key:<title>.<year>`, `album:key:<artist>.<album>`, or an *arr id `movie:tmdb:…` / `series:tvdb:…` / `album:mbid:…`. See [identity](config.md#identity).

## Empty states

English, no decoration: `nothing happening`, `nothing on hold`, `nothing to copy`, `nothing on the box`, `nothing to reclaim`, `nothing to encode`.

## Exit codes

| Code | Name | Typical cause |
| ---- | ---- | ------------- |
| 0 | ok | |
| 1 | runtime | I/O, RPC, unexpected failure |
| 2 | usage | bad flags or args |
| 3 | lock_conflict | another `plan` / `run` / `reclaim apply` holds the flock |
| 4 | drift_verify | edge or grabber drifted from `config.toml` |
| 5 | policy_refusal | encode refuse, PEM-in-git, pin matrix, watermark at bootstrap |

## Daemon (reference)

Both units are written for you. You do not type these on a normal day.

```bash
# seedbox — bind port = the port in seedbox_address
mediaopsd serve --role seedbox --bind 0.0.0.0:25410 \
  --tls-dir ~/.config/mediaops/tls --config ~/.config/mediaops/config.toml \
  --nginx-dir /etc/nginx/apps --root movies=/home/you/media/movies

# home gateway — seedbox address comes from config.toml, not the CLI
mediaopsd serve --role home --tls-dir <config-dir>/tls --config <config-dir>/config.toml
```
