# Config

`config.toml` is deny-unknown-fields TOML, `schema_version = 1`. Sizes are `*_gib` / `*_mib` in the file and bytes in code.

## Default paths

Every verb and every generated unit derives paths from the same functions, so the config dir is one consistent choice per machine.

| What | Where |
| ---- | ----- |
| Config dir | `$MEDIAOPS_CONFIG_DIR`, else `$XDG_CONFIG_HOME/mediaops` (default `~/.config/mediaops`); use `$XDG_DATA_HOME/mediaops` (default `~/.local/share/mediaops`) if that config path is inside a git work tree |
| Config | `<config-dir>/config.toml` |
| mTLS PEMs | `<config-dir>/tls/` (never in a git work tree; bootstrap refuses) |
| Home database | `$XDG_STATE_HOME/mediaops/api.db` (default `~/.local/state/mediaops/api.db`); only the API opens it |
| Legacy capabilities + maintenance lock | `state.db` and `mediaops.lock` beside `api.db` |
| Home API socket | `$XDG_RUNTIME_DIR/mediaops-api.sock` |
| Range gateway socket | `$XDG_RUNTIME_DIR/mediaopsd.sock` |
| On the box | `~/.local/bin/mediaopsd`, `~/.config/mediaops/{config.toml,tls/}`, `~/.config/systemd/user/mediaopsd.service` |

`--config PATH`, `--config-dir`, `--tls-dir`, `--state-db`, `--socket`, and `--library-root` override the defaults on a single invocation.

Without an absolute `XDG_RUNTIME_DIR`, both sockets live in the application state directory. `--socket` selects the API for object commands and the gateway for low-level `list` / `pull`; the protocols are different. The explicit offline `--state-db` workflow is for isolated legacy state, not a fallback when the API is unavailable.

After bootstrap/import, runtime settings live in the Cluster object. Editing `config.toml` does not change an active Job. Inspect and update the Cluster through `get` / `apply`; each new Job snapshots its library root, budgets, and Range settings.

## Fields

Required:

| Field | Meaning |
| ----- | ------- |
| `schema_version` | Must be `1` |
| `max_copy_gib` | Maximum bytes in bound, unfinished Pull Jobs (music first, then video); completed Jobs release capacity |
| `min_free_gib` | Never drop the library disk below this |
| `range_len_mib` | Bytes requested per `GetRange`. Seedbox serves at most 64 MiB |
| `max_nvenc` | Ceiling; the probe at library bootstrap may be lower |
| `lock` | When `true` on the Cluster object, controllers create no new Pull Jobs |

Optional:

| Field | Meaning |
| ----- | ------- |
| `range_concurrency` | Parallel Range streams; Home Jobs default to one when omitted |
| `grabber` | `"none"` (default) or `"servarr"` |
| `provider` | `"swizzin_box"` or `"already_there"` in v1. Others parse and refuse |
| `seedbox_address` | Written by `seedbox bootstrap`. Do not type it |
| `underlay` | Designed; unused by default |
| `tls` | Fingerprints + paths. Written by bootstrap. Never PEMs |
| `[[paths.roots]]` | Allowlisted roots on the box (`id`, `path`, optional `kind`) |
| `[edge]` | `bind`, `auth`, `url_bases`. Configures the nginx/Forms security check in `doctor` |
| `[grab]` | Indexer/client/custom-format sets. Empty is a no-op, never a wipe |
| `[pins]` | Lidarr / glibc matrix. A pin above `refuse_above` is exit 5 |

Without `[edge]`, the optional panel check is skipped. Certificate and credential checks still apply.

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

Rendering is strict dots, no spaces. Parsing is lenient about spaces and `Title - Subtitle (Year)` folders (what *arr leaves on the box). Scene tags (`REPACK`, `PROPER`, …) are stripped. Inspect `get RemoteFile -o wide` for files the grammar cannot place (`parseOk = false`); they do not become automatic Pull Jobs. `.nfo`, samples, `.par2`, and subtitles are ignored.

`_ops/` and `_incoming/` are app-managed, never libraries. Do not add them to Jellyfin or Plex. Staging is `_incoming/<kind-source-id>/<filename>` plus a `.partial` / `.partial.b3` sidecar. GC never deletes a partial.

## Identity

`TitleId` is `kind:source:id`. Never a raw path.

| Form | Who uses it | Example |
| ---- | ----------- | ------- |
| `movie:key:thematrix.1999` | What a library path names | `watch` / `why` by folder |
| `series:key:mrrobot.2015` | Same, per show | episodes still copy independently |
| `album:key:yes.relayer` | Artist + album, not folder year | remasters are one album |
| `movie:tmdb:603` | *arr authority | hold inbox, unmonitor |
| `series:tvdb:…` / `album:mbid:…` | Same | |

Identity is per **file**: an episode is `(show, season, episode)`, a track is `(album, disc, track)`. A show with one episode on disk still copies its other episodes; a half-copied album finishes.

A spoken name (`Hearts`, `Mr Robot`) only resolves when something already knows it: the library, a job, the hold inbox, or a listing.

## Unmonitor

Inventory owns best-effort movie/album unmonitor. After a successful listing heartbeat, with `grabber = "servarr"`, it calls ControlPort `wanted_missing` then `unmonitor` for TitleIds that have a non-drifted local file and appear in that snapshot. Series are never unmonitored. `grabber = "none"` makes zero Control calls. Failures log and continue; they do not roll back `list_generation` or write Jobs.
