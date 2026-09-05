# mediaops

Home-disk library of record plus a disposable seedbox. The CLI (`mediaops`) talks only to a local `mediaopsd` over a unix socket. The seedbox daemon is the only process that opens the WAN. Pull is one-way Range RPC (not FTP, rsync, or SSH copy). `grabber=None` is a valid path: a schema folder on the box, a disk at home.

This repo is a Cargo workspace. The product contract lives under `_bmad-output/specs/spec-mediaops/`. This file is how to build and run it.

Story commits put the story key in the subject as `N-M` (hyphen, e.g. `2-1`), so `git_evidence.py --stories` can attribute. Do not rewrite published commits to backfill old subjects.

## Requirements

- Rust **1.98** (`rust-toolchain.toml` pins it)
- `protobuf-compiler` (`protoc`) — `crates/proto` builds the gRPC stubs
- A lockfile-aware Cargo (`make` passes `--locked`)
- For `make musl` (the static daemon the seedbox runs): `musl-tools` + `cmake` (`musl-gcc`). Not needed for `make test`.

On Debian/Arch:

```bash
sudo apt-get install -y protobuf-compiler   # Debian/Ubuntu
# sudo pacman -S protobuf                    # Arch
# make musl also needs:
sudo apt-get install -y musl-tools cmake    # Debian/Ubuntu (`musl-gcc`)
# sudo pacman -S musl cmake                  # Arch
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
make musl          # link musl-static mediaopsd (needs musl-gcc; not part of make test)
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

`--json` on every verb prints one `{ok,data,error}` envelope on stdout. Tracing goes to stderr. Human stdout is the operator UI: color and bold only when stdout is a tty, sizes as `7.1 GiB`, ages as `21m`. Plan JSON / lock / socket paths are the last line or omitted.

## How to use it

One operator, their machines. After `make install`, type these. The timer runs `mediaops run`; you peek with the rest.

`why` and `watch` take a spoken name (`Hearts`, `Mr Robot`) or a title id. A name only resolves when the library, a job, the hold inbox, or a listing already knows it. `hold approve` / `reject` take the id from `hold list` (`movie:tmdb:4539`).

### Daily

```bash
mediaops status
mediaops hold list
mediaops plan
mediaops run
mediaops why 'Mr Robot'
```

`status` — lock-free. Quiet means nothing in flight:

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

`hold list` — inbox. The id on the second line is what you approve:

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

`plan` — exclusive lock. One line per copy / skip / review. Reconciler bookkeeping (`grab_apply`, `edge_apply`) is hidden. Empty is `nothing to copy`.

```
copy      Mr Robot (2015) S01E02  4.0 GiB
copy      Mr Robot (2015) S04E01  2.6 GiB
review    usenet_tv / _UNPACK_Mr.Robot.S02…  unparseable

/home/coto/.local/state/mediaops/plans/….json
```

`run` — exclusive lock. Plan then apply that artifact in this process. Lock conflict is exit 3, never silent 0. A long pull prints progress on stderr (tty only):

```
pull    Mr Robot (2015) S01E02  1.2 GiB / 4.0 GiB
```

When it finishes:

```
copied    Mr Robot (2015) S01E02
copied    Mr Robot (2015) S04E01
```

`why` — lock-free. Headline, then id, then only the facts that exist:

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

### Occasional

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

`list` — files on the box, grouped by root:

```
tv
   4.0 GiB  Mr.Robot.(2015)/Season.01/Mr.Robot.(2015).S01E02.eps1.1_ones-and-zer0es.mpeg.mkv

usenet_movies
   6.8 GiB  Hearts.of.Darkness-…WATCHABLE.mkv
```

`watch` records a want and exits. It does not wait for playable:

```
watching  Foundation (2021)
          series:key:foundation.2021
```

`reclaim preview` is lock-free. `reclaim apply` is exclusive and unlinks box copies proved installed at home (index row + file on disk) and not seeding.

```
nothing to reclaim
```

or

```
reclaim   The Matrix (1999)  6.8 GiB
          seedbox / movies/The.Matrix.(1999)/The.Matrix.(1999).mkv
          ratio 2.1  public
```

`encode scan` / `run` are home GPU only (not in `mediaopsd`). Empty is `nothing to encode`. Scan of a movie that should go through NVENC:

```
encode    The Matrix (1999)
```

`doctor` prints `ok` when the edge invariant holds.

Empty states are English: `nothing happening`, `nothing on hold`, `nothing to copy`, `nothing on the box`, `nothing to reclaim`, `nothing to encode`.

### CLI verbs (`mediaops`)

| Command | What it does |
| ------- | ------------ |
| `seedbox bootstrap` | SSH Host `seedbox`: copy daemon, mint certs, probe Range N. Needs `--yes` to apply. |
| `library bootstrap` | Schema dirs, sqlite, lock, systemd-user units, NVENC probe. `--enable-timer` also enables the run timer and home unit. |
| `list` / `pull` | List remotes / pull one file through the home socket. |
| `watch TITLE` | Record a want. Title id or a name from the library / inbox. Exits 0; does not wait for playable. |
| `plan` / `run` | Exclusive lock. `run` is plan + apply in this process. Lock conflict is exit 3, never silent 0. Approved holds become Copy on this path. |
| `why TITLE` / `status` | Lock-free peek. `why` takes a title id or a name. Local FS is truth. |
| `reclaim preview\|apply` | Ranked surplus dry-run (lock-free); exclusive unlink after `install_b3` plus the library file. |
| `hold list\|approve\|reject` | Lock-free import-blocked inbox. List shows the title and the id; `approve movie:tmdb:…` / `reject movie:tmdb:…`. Approve records a decision (does not install). Reject is never-this-release. |
| `doctor` / `repair edge` | Read-only EdgeInvariant vs confirmed nginx + API repair. |
| `seedbox apply\|upgrade` | Grabber set-diff apply; re-copy musl `mediaopsd` and restart. |
| `encode scan\|run\|pause` | Home GPU only. Not linked into `mediaopsd`. |

`TITLE` is `movie:key:<title>.<year>`, `series:key:<title>.<year>`, `album:key:<artist>.<album>` (what a library path names; see [Library layout](#library-layout)), or an *arr authority id `movie:tmdb:…` / `series:tvdb:…` / `album:mbid:…` (what the hold inbox carries).

### Daemon (`mediaopsd serve`)

Both units are written for you (`seedbox bootstrap` on the box, `library bootstrap` at home). For reference:

```bash
# seedbox: what the generated user unit runs (bind port = the port in seedbox_address)
mediaopsd serve --role seedbox --bind 0.0.0.0:25410 \
  --tls-dir ~/.config/mediaops/tls --config ~/.config/mediaops/config.toml \
  --nginx-dir /etc/nginx/apps --root movies=/home/me/media/movies --root tv=/home/me/media/tv

# home gateway (unix socket; seedbox address comes from config.toml, not the CLI)
mediaopsd serve --role home --tls-dir <config-dir>/tls --config <config-dir>/config.toml
```

## Default paths

| What | Where |
| ---- | ----- |
| Config dir (`config.toml` + `tls/`) | `~/.config/mediaops`, **or** `~/.local/share/mediaops` when `~/.config` is a git work tree (dotfiles), **or** `$MEDIAOPS_CONFIG_DIR` |
| Config | `<config-dir>/config.toml` |
| mTLS PEMs | `<config-dir>/tls/` (never in a git work tree; bootstrap refuses) |
| sqlite + lock | `~/.local/state/mediaops/state.db` |
| Plan artifacts | `~/.local/state/mediaops/plans/` |
| Home socket | `$XDG_RUNTIME_DIR/mediaopsd.sock` |
| On the box | `~/.local/bin/mediaopsd`, `~/.config/mediaops/{config.toml,tls/}`, `~/.config/systemd/user/mediaopsd.service` |

Every verb and every generated unit derives its paths from the same defaults, so the config dir is one consistent choice per machine.

`config.toml` is deny-unknown-fields TOML. Sizes are `*_gib` / `*_mib` in the file and bytes in code.

### Config example (this operator's)

```toml
schema_version = 1
max_copy_gib = 80            # per-run copy budget on video (music first, same cap)
min_free_gib = 256           # never drop the library disk below this
range_len_mib = 32           # one Range RPC; the seedbox serves at most 64
range_concurrency = 8        # parallel Range streams; set it and the probe is skipped
max_nvenc = 3
lock = false
grabber = "servarr"          # or "none": a schema folder on the box, no *arr
provider = "swizzin_box"
# seedbox_address is written by `seedbox bootstrap`, never typed

[[paths.roots]]              # allowlisted roots on the box and what each holds
id = "movies"
path = "/home/seedit4me/media/movies"
kind = "movie"               # movie | series | album; omit to infer from path shape
[[paths.roots]]
id = "tv"
path = "/home/seedit4me/media/tv"
kind = "series"
[[paths.roots]]
id = "music"
path = "/home/seedit4me/media/music"
kind = "album"
[[paths.roots]]
id = "usenet_movies"
path = "/home/seedit4me/downloads/usenet/complete/movies"
kind = "movie"
```

An `[edge]` table (`bind`, `auth`, `url_bases`) turns on the nginx/Forms edge invariant check on every `plan`; without it `plan` never freezes on the panel. `[grab]` sets are optional: an empty set is a no-op, never a wipe.

## Library layout

The grammar is the dotted Jellyfin layout Radarr/Sonarr/Lidarr are configured to write on the box (`{Movie.CleanTitle}.({Release Year})`, `Season.{season:00}`, `{Artist Name}/{Album Title}.({Release Year})/...`). No id tokens live in paths; identity recovered from a path is the `key` form (normalised title + year; artist + album for music, so a remaster is the same album):

```
movies/The.Matrix.(1999)/The.Matrix.(1999).mkv                          movie:key:thematrix.1999
series/The.Wire.(2002)/Season.01/The.Wire.(2002).S01E01.The.Target.mkv   series:key:thewire.2002  (S01E01)
music/Yes/Relayer.(1974)/Relayer.(1974).01.The.Gates.Of.Delirium.flac    album:key:yes.relayer    (track 01)
music/Radiohead/OK.Computer.(1997)/Disc.02/OK.Computer.(1997).03.Airbag.flac
```

Identity is **per file**: an episode is `(show, season, episode)`, a track is `(album, disc, track)`. A show with one episode on disk still copies its other episodes; a half-copied album finishes. Parsing is lenient about spaces and `Title - Subtitle (Year)` folders (what *arr leaves on the box); rendering is strict dots. Media files the grammar cannot place show up in `plan` as `review` actions with a reason (`needs-year`, `unparseable`) instead of vanishing. `.nfo`, samples, `.par2`, subtitles are ignored.

## Setting it up (replacing `media-sync`)

1. `make install` — `mediaops` and `mediaopsd` into `~/.cargo/bin`.
2. Write `<config-dir>/config.toml` (example above).
3. Bootstrap the box. The daemon must bind a port the box actually forwards; a rented box usually only forwards its per-user ports (`~/.<app>_port` on Swizzin/SeedIt4Me) — pick a free one with `--address`:

   ```bash
   mediaops seedbox bootstrap --provider swizzin_box --address ftl28.seedit4.me:25410 \
     --root movies=/home/seedit4me/media/movies --root tv=/home/seedit4me/media/tv \
     --root music=/home/seedit4me/media/music \
     --root usenet_movies=/home/seedit4me/downloads/usenet/complete/movies \
     --root usenet_tv=/home/seedit4me/downloads/usenet/complete/tv \
     --root usenet_music=/home/seedit4me/downloads/usenet/complete/music        # plan
   mediaops seedbox bootstrap --yes ...same flags...                               # apply
   ```

   It mints certs, ships `mediaopsd` + server certs + `config.toml` + a user unit, writes `seedbox_address` into the config, and edge-checks the box.
4. `mediaops library bootstrap --library-root /mnt/storage/videos` — schema dirs, sqlite, NVENC probe (prefers `/usr/lib/jellyfin-ffmpeg/ffmpeg`), and the `mediaopsd-home.service` / `mediaops-run.{service,timer}` user units.
5. `systemctl --user daemon-reload && systemctl --user enable --now mediaopsd-home.service`, then `mediaops plan` (read-only) and `mediaops run`.
6. Cut over: `systemctl --user disable --now media-sync.timer && systemctl --user enable --now mediaops-run.timer`. The timer fires one hour after the previous run finishes, like the old one; the service is niced for the spinning disk. Do not run both timers: both would pull the same files.

See [How to use it](#how-to-use-it) for the daily commands and what they print. `status`, `why`, `doctor`, `hold list`, and `reclaim preview` are lock-free. `reclaim apply` unlinks box copies that are proved installed at home (index row + file on disk) and not seeding.

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
crates/core       TitleId (key/tmdb/tvdb/mbid), PathSchema v2, config.toml, Plan, jobs (no I/O)
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

Not in this tree yet: `docs render`, TUI.
