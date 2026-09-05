# Config

`config.toml` is deny-unknown-fields TOML, `schema_version = 1`. Sizes are `*_gib` / `*_mib` in the file and bytes in code.

## Default paths

Every verb and every generated unit derives paths from the same functions, so the config dir is one consistent choice per machine.

| What | Where |
| ---- | ----- |
| Config dir | `$MEDIAOPS_CONFIG_DIR`, else `~/.config/mediaops`, else `~/.local/share/mediaops` when `~/.config` is a git work tree |
| Config | `<config-dir>/config.toml` |
| mTLS PEMs | `<config-dir>/tls/` (never in a git work tree; bootstrap refuses) |
| sqlite + lock | `~/.local/state/mediaops/state.db`, `mediaops.lock` beside it |
| Plan artifacts | `~/.local/state/mediaops/plans/` |
| Home socket | `$XDG_RUNTIME_DIR/mediaopsd.sock` |
| On the box | `~/.local/bin/mediaopsd`, `~/.config/mediaops/{config.toml,tls/}`, `~/.config/systemd/user/mediaopsd.service` |

`--config PATH`, `--config-dir`, `--tls-dir`, `--state-db`, `--socket`, and `--library-root` override the defaults on a single invocation.

## Fields

Required:

| Field | Meaning |
| ----- | ------- |
| `schema_version` | Must be `1` |
| `max_copy_gib` | Per-run copy budget (music first, then video, same cap) |
| `min_free_gib` | Never drop the library disk below this |
| `range_len_mib` | Bytes requested per `GetRange`. Seedbox serves at most 64 MiB |
| `max_nvenc` | Ceiling; the probe at library bootstrap may be lower |
| `lock` | When `true`, copies are frozen: `plan` emits skip-lock, `run` does not pull, `pull` is exit 5 |

Optional:

| Field | Meaning |
| ----- | ------- |
| `range_concurrency` | Parallel Range streams. Set it and the probe is skipped |
| `grabber` | `"none"` (default) or `"servarr"` |
| `provider` | `"swizzin_box"` or `"already_there"` in v1. Others parse and refuse |
| `seedbox_address` | Written by `seedbox bootstrap`. Do not type it |
| `underlay` | Designed; unused by default |
| `tls` | Fingerprints + paths. Written by bootstrap. Never PEMs |
| `[[paths.roots]]` | Allowlisted roots on the box (`id`, `path`, optional `kind`) |
| `[edge]` | `bind`, `auth`, `url_bases`. Turns on the nginx/Forms check on every `plan` |
| `[grab]` | Indexer/client/custom-format sets. Empty is a no-op, never a wipe |
| `[pins]` | Lidarr / glibc matrix. A pin above `refuse_above` is exit 5 |

Without `[edge]`, `plan` never freezes on the panel.

## Example

```toml
schema_version = 1
max_copy_gib = 80
min_free_gib = 256
range_len_mib = 32
range_concurrency = 8
max_nvenc = 3
lock = false
grabber = "servarr"
provider = "swizzin_box"

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
[[paths.roots]]
id = "usenet_movies"
path = "/home/you/downloads/usenet/complete/movies"
kind = "movie"
```

## Library layout

The grammar is the dotted Jellyfin layout Radarr/Sonarr/Lidarr are configured to write on the box. No id tokens live in paths. Identity recovered from a path is the `key` form (normalised title + year; artist + album for music, so a remaster is the same album).

```
movies/The.Matrix.(1999)/The.Matrix.(1999).mkv
series/The.Wire.(2002)/Season.01/The.Wire.(2002).S01E01.The.Target.mkv
music/Yes/Relayer.(1974)/Relayer.(1974).01.The.Gates.Of.Delirium.flac
music/Radiohead/OK.Computer.(1997)/Disc.02/OK.Computer.(1997).03.Airbag.flac
```

Rendering is strict dots, no spaces. Parsing is lenient about spaces and `Title - Subtitle (Year)` folders (what *arr leaves on the box). Scene tags (`REPACK`, `PROPER`, …) are stripped. Media the grammar cannot place shows up in `plan` as `review` (`needs-year`, `unparseable`, `needs-split`) instead of vanishing. `.nfo`, samples, `.par2`, and subtitles are ignored.

`_ops/` and `_incoming/` are app-managed, never libraries. Do not add them to Jellyfin or Plex. Staging is `_incoming/<kind-source-id>/<filename>` plus a `.partial` / `.partial.b3` sidecar. GC never deletes a partial.

## Identity

`TitleId` is `kind:source:id`. Never a raw path.

| Form | Who uses it | Example |
| ---- | ----------- | ------- |
| `movie:key:thematrix.1999` | What a library path names | planner, `watch` / `why` by folder |
| `series:key:mrrobot.2015` | Same, per show | episodes still copy independently |
| `album:key:yes.relayer` | Artist + album, not folder year | remasters are one album |
| `movie:tmdb:603` | *arr authority | hold inbox, unmonitor |
| `series:tvdb:…` / `album:mbid:…` | Same | |

Identity is per **file**: an episode is `(show, season, episode)`, a track is `(album, disc, track)`. A show with one episode on disk still copies its other episodes; a half-copied album finishes.

A spoken name (`Hearts`, `Mr Robot`) only resolves when something already knows it: the library, a job, the hold inbox, or a listing.
