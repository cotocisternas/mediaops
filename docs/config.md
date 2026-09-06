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

Commands expose the path overrides they use, such as `--config`, `--config-dir`,
`--tls-dir`, `--state-db`, `--socket`, and `--library-root`. These are not universal
flags; check the specific command's `--help`.

Without an absolute `XDG_RUNTIME_DIR`, both sockets live in the application state directory. The protocols and flag routing are different:

| Commands | Home API address | Gateway address |
| -------- | ---------------- | --------------- |
| `get`, `apply`, `delete`, `watch`, `reconcile`, `import-legacy` | `--socket` | Not selected by these commands |
| `list`, `pull` | `pull` uses the default Home API address | `--socket`; these commands do not accept `--api-socket` |
| `status`, `why`, `hold` | `--api-socket` on the Home path | `--socket` on the explicit legacy path |
| `doctor` | `--api-socket` for its Home readiness checks | `--socket` for its edge/credential checks |

`status`, `why`, and `hold` do not redirect their Home API connection when only
`--socket` is supplied. The explicit offline `--state-db` workflow is for isolated
legacy state, not a fallback when the API is unavailable. It does not turn
gateway-dependent commands into offline operations.

After bootstrap/import, runtime settings live in the Cluster object. Editing `config.toml` does not change an active Job. Inspect and update the Cluster through `get` / `apply`; each new Job snapshots its library root, budgets, and Range settings.

## Config files versus Home objects

The field tables below describe `config.toml`, not an `apply -f` object document.
Home object documents have `kind`, `metadata`, and `spec`. Raw API JSON uses
camelCase fields and byte counts, such as `spec.minFree` and `spec.rangeLen`, not
the file-format `min_free_gib` and `range_len_mib` units.

To change an existing Cluster, retrieve the current version, edit its spec, then
apply it while preserving `metadata.resourceVersion`:

```bash
mediaops get Cluster home -o json > cluster.json
# Edit cluster.json, then:
mediaops apply -f cluster.json
```

Creation uses resourceVersion zero; updates must use the exact current version.
Bootstrap and `import-legacy` translate `config.toml` into Home objects. Secret
holds the gateway endpoint and credentials. The seedbox daemon, `seedbox apply`,
and explicit edge maintenance still use `config.toml`; updating the Home Cluster
does not rewrite the box's config or push grabber configuration.

## Fields

Required:

| Field | Meaning |
| ----- | ------- |
| `schema_version` | Must be `1` |
| `max_copy_gib` | Maximum bytes in bound, unfinished Pull Jobs (music first, then video); completed Jobs release capacity |
| `min_free_gib` | Free-space reserve checked for staging and again before installation, including a separate destination filesystem |
| `range_len_mib` | Bytes requested per `GetRange`. Seedbox serves at most 64 MiB |
| `max_nvenc` | Ceiling; the probe at library bootstrap may be lower |
| `lock` | When `true` on the Cluster object, controllers create no new Pull Jobs and the scheduler does not bind Pending Jobs; it is not cancellation of already bound work |

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

Rendering is strict dots, no spaces. Parsing is lenient about spaces and `Title - Subtitle (Year)` folders (what *arr leaves on the box). Scene tags (`REPACK`, `PROPER`, …) are stripped. Inspect `mediaops get RemoteFile -o json` and `status.parseOk` for files the grammar cannot place; `parseOk = false` prevents automatic Want-driven Pull Jobs. Wide output does not expose this flag. `.nfo`, samples, `.par2`, and subtitles are ignored.

`_ops/` and `_incoming/` are app-managed, never libraries. Do not add them to Jellyfin or Plex. Transfer bytes are staged in `_incoming/<kind-source-id>/<filename>.partial`, with proofs in `<filename>.partial.b3`. A completed transfer uses `<filename>` until installation. Recovery removes the owned completed source and proof sidecar only after destination verification; unfinished partials are not garbage-collected.

## Identity

`TitleId` is `kind:source:id`. Never a raw path.

| Form | Who uses it | Example |
| ---- | ----------- | ------- |
| `movie:key:thematrix.1999` | What a library path names | `watch` / `why` by folder |
| `series:key:mrrobot.2015` | Same, per show | episodes still copy independently |
| `album:key:yes.relayer` | Artist + album, not folder year | remasters are one album |
| `movie:tmdb:603` | *arr authority | hold inbox, unmonitor |
| `series:tvdb:…` / `album:mbid:…` | Same | |

`TitleId` identifies the movie, show, or album. Copy and installation identity is
per **file**, combining that TitleId with the placement key: `(season, episode)`
for an episode, `(disc, track)` for a track, or the movie's whole-file key. A show
with one episode on disk still copies its other episodes; a half-copied album finishes.

A spoken name (`Hearts`, `Mr Robot`) only resolves when something already knows it: the library, a job, the hold inbox, or a listing.

## Unmonitor

Inventory owns best-effort movie/album unmonitor. After a successful listing heartbeat, with `grabber = "servarr"`, it calls ControlPort `wanted_missing` then `unmonitor` for TitleIds with recorded installation proof, a non-drifted local regular file, and an exact match in that response. Series are never unmonitored. `grabber = "none"` skips these unmonitor-related calls; normal listing still runs through the gateway. No Want is required for unmonitor, and Cluster lock does not suppress it. Failures log and retry on later refreshes; they do not roll back `list_generation` or write Jobs.
