# Usage

After [setup](setup.md), `mediaops-home` keeps the control plane up. You apply a Want and peek with the rest.

`why` and `watch` take a spoken name (`Hearts`, `Mr Robot`) or a title id. A name only resolves when the library, a job, the hold inbox, or a listing already knows it. Prefer a `TitleId` in scripts. `hold approve` / `reject` take the id from `hold list` (`movie:tmdb:4539`).

Home API verbs (`get`, `apply`, `delete`) print a tab-separated pipeline table by default (TitleId when available, otherwise object name, in `$1`). `-o json` is the **raw object** (no `{ok,data,error}` envelope). `-o wide` adds columns, aligned with spaces for terminal reading. Legacy verbs still accept `--json` as one `{ok,data,error}` envelope. Tracing goes to stderr.

The default Home-backed workflows require the Home API. An outage is an error,
not an implicit switch to `state.db`. An isolated custom `--state-db` selects the
supported legacy path; commands that need the gateway still need it. The default
state file or a file beside `api.db` uses Home. Object commands use `--socket` for
the API. `list` / `pull` use `--socket` for the gateway and do not accept
`--api-socket`; manual pull uses the default Home API address. Home-backed
`status` / `why` / `hold` use `--api-socket` and do not use `--socket` to select
their API. `doctor` uses both endpoints for different checks. See the
[socket-routing table](config.md#default-paths).

## Daily

```bash
mediaops watch movie:key:thematrix.1999
mediaops get Job
mediaops get Title movie:key:thematrix.1999 -o json
mediaops reconcile
mediaops why 'Mr Robot'
```

A new Want document can be TOML:

```toml
kind = "Want"
[metadata]
name = "movie:key:thematrix.1999"
[spec]
title_id = "movie:key:thematrix.1999"
```

Updates require the current `metadata.resourceVersion`. Retrieve the object with `get -o json`, edit its spec, and apply it; a stale version is refused. `watch TITLE` is idempotent. `get Job --watch -o json` streams one object per line until interrupted; an optional object name filters that stream.

`apply -f` takes a Home object document, not the bootstrap `config.toml` format.
See [editing Cluster settings](config.md#config-files-versus-home-objects).

### `status`

Lock-free. Quiet means nothing in flight:

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

Approve does not copy. The Hold controller creates a Pull Job after `Approved`. There is no auto-approve.

The numbered inbox includes only the fresh completed listing; archived decisions remain in `get Hold`. Rejection is recorded first and reconciled to the box by inventory. Revoking approval refuses an unstarted Job; once its Job is bound, decision changes are refused until that Job finishes. Approval requires a live release with a usable placement.

### `get` / `apply` / `watch`

`watch` writes a Want and exits. Inventory + the Want controller create a snapshotted Pull Job for each file. The scheduler binds albums first. `get Job` / `get Title` peek. Empty Job list is a blank table. A series Want continues to include later episodes; Title observations retain separate proofs for every episode and track.

```
watching  The Matrix (1999)
          movie:key:thematrix.1999
```

Music copies before video, under the same `max_copy` cap. Work that does not fit
the scheduler's budget stays Pending; the worker rechecks capacity and can refuse
a Job if the reserve is no longer available. Manual installation also checks the
destination filesystem before publication. Recorded placements are not silently
replaced; there is no auto-upgrade from 1080p to a 4K remux.

Failed or refused Jobs remain visible. Inspect `why TITLE` or `get Job NAME -o wide`, fix the cause, then delete that terminal Job to let the still-open Want create a fresh attempt. Completed ranges can resume; existing library content is never overwritten by this retry. Deleting a bound active Job is refused.

Revoking a Want refuses its unbound work. A temporarily absent source can remain
Pending and bind when it returns without blocking other Jobs. After destination
publication, verification, cleanup, or API I/O failures leave the Job Verifying
for recovery rather than declaring an unproved live file complete.

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

$ mediaops why movie:key:matrix.1999
Matrix (1999)
movie:key:matrix.1999

library   drifted
want      open, listed on the box

$ mediaops why series:key:foundation.2021
Foundation (2021)
series:key:foundation.2021

quiet
```

Home `why` prints only facts that exist: `hold` (open inbox reason + size), `grab` (open Want, not listed), `want` (open Want, listed on the box), `pull` (Job phase), `library` (observed path or drifted), or `quiet`. There is no legacy `import` line.

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

Manual `pull` uses the current Cluster settings and requires the API. It pauses scheduling and refuses while a bound Job is active. With `--install`, success includes a verified Title file record; it does not create a legacy `job_id`. The normal unattended workflow is `watch` plus the pull worker.

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

`encode pause` / `encode pause --off` changes `Cluster.spec.encodePause`, checked between jobs. It is not a signal to a running ffmpeg. Explicit offline legacy mode keeps its machine flag.

Policy (hardcoded, not a config field):

| Input | Decision |
| ----- | -------- |
| HDR, Dolby Vision, or height ≥ 2160 | keep forever (`Refuse` on `encode run TITLE` is exit 5) |
| Series + HEVC + MP4 | keep |
| Movie + HEVC + 10-bit + MP4 + not HDR | NVENC → H.264 8-bit (`yuv420p`) |
| Already H.264 8-bit | keep |

The live file is replaced only after the convert succeeds. The original moves to `_ops/backup-hevc-originals/…`. `install_b3` (the pull digest) stays; `current_b3` updates.

### `doctor` / `repair edge`

`doctor` is read-only: edge invariant, key presence, and PEM-in-git scan. By
default it also requires the Home API and Ready scheduler, inventory, and pull
Nodes. `ok` means all checks passed. An explicit `--state-db` skips the additional
Home readiness check unless `--api-socket` is also supplied; it does not skip the
gateway's edge and credential checks.

`mediaops repair edge --repair --confirm` performs nginx maintenance over SSH and
edge apply/verification through the gateway Control API. Bare `repair edge`
refuses with usage exit 2. `doctor --repair` is not a repair shortcut. Only run the
explicit repair command when intending a write.

## Command map

| Command | What it does |
| ------- | ------------ |
| `seedbox bootstrap` | SSH Host `seedbox`: copy daemon, mint certs, probe Range. Needs `--yes` to apply. |
| `seedbox apply` | Grabber set-diff from `config.toml`. Empty `[grab]` is a no-op, never a wipe. |
| `seedbox upgrade` | Re-copy musl `mediaopsd` and restart. Needs `--yes`. |
| `library bootstrap` | Schema dirs, sqlite, lock, systemd-user units, NVENC probe. |
| `library relocate` | Retarget `library_root` and units. Does not copy media. |
| `library reindex` | Rebuild title-index proof by hashing on-disk schema files. |
| `get` / `apply` / `delete` | Home API. `-o json` is the raw object. |
| `watch TITLE` | Record a Want. Exits 0; does not wait for playable. |
| `reconcile` | Kick in-process controllers. |
| `import-legacy` | One-shot Apply of old `config.toml` + `state.db`. |
| `why TITLE` / `status` | Peek at Home API state. |
| `list` | Allowlisted remotes through the home unix-socket gateway. |
| `pull` | One remote file into `_incoming/` with `.partial` resume. Maintenance: pauses scheduling, refuses a bound Job. |
| `reclaim preview\|apply` | Ranked surplus dry-run; exclusive unlink after local proof. |
| `hold list\|approve\|reject` | Lock-free import-blocked inbox. |
| `encode scan\|run\|pause` | Home GPU only. |
| `doctor` / `repair edge --repair --confirm` | Read-only edge/credential/Home-readiness check vs explicit SSH + Control API edge repair. |
| `new-machine export\|import` | Private bundle with config, credentials, runtime objects and every file's installation proof. |

Relocation refuses incompatible nonterminal Jobs, including unbound Pending Jobs: their library root is an immutable snapshot. Let that work finish before relocating. A failed maintenance command can leave `Cluster.spec.lock` set; inspect and resolve the cause, then unlock explicitly with `get` / edit / `apply`. Do not treat a leftover lock as auto-cleared.

`new-machine import` requires an empty Home Job list (including terminal Jobs). A retry of the same bundle is success: it publishes only missing Title rows and keeps newer `current_b3` / drift. A foreign Title, an extra path, or a changed `install_b3` is refused and the previous Cluster lock is restored. `--library-root` must already match `Cluster.spec.libraryRoot` when Titles exist.

### Output shapes

| Mode | What stdout is |
| ---- | -------------- |
| default table | TSV pipeline (TitleId when available, otherwise object name, in `$1`). Empty Job list is a blank table. |
| `-o wide` | Space-aligned object columns for a terminal; maintenance counters retain their TSV output. |
| `-o json` | Raw object or `{items:[…]}` list. No `{ok,data,error}` envelope. |
| `--json` | One `{ok,data,error}` envelope. Do not combine with `-o`. |

`reconcile` table is `reconcileGeneration\tN`. `import-legacy` table is `imported\tN`. `-o json` for those is `{"reconcileGeneration":N}` / `{"imported":N}`. Tracing stays on stderr.

`TITLE` is `movie:key:<title>.<year>`, `series:key:<title>.<year>`, `album:key:<artist>.<album>`, or an *arr id `movie:tmdb:…` / `series:tvdb:…` / `album:mbid:…`. See [identity](config.md#identity).

## Empty states

English, no decoration: `nothing happening`, `nothing on hold`, `nothing on the box`, `nothing to reclaim`, `nothing to encode`.

## Exit codes

| Code | Name | Typical cause |
| ---- | ---- | ------------- |
| 0 | ok | |
| 1 | runtime | I/O, RPC, unexpected failure |
| 2 | usage | bad flags or args |
| 3 | lock_conflict | exclusive CLI maintenance, such as manual pull, encode run, reclaim apply, library, new-machine, seedbox apply, or edge repair, already holds the flock |
| 4 | drift_verify | edge or grabber drifted from `config.toml` |
| 5 | policy_refusal | encode refuse, PEM-in-git, pin matrix, watermark at bootstrap |

## Daemon (reference)

Units are written for you. You do not type these on a normal day.

```bash
# seedbox — bind port = the port in seedbox_address
mediaopsd serve --role seedbox --bind 0.0.0.0:25410 \
  --tls-dir ~/.config/mediaops/tls --config ~/.config/mediaops/config.toml \
  --nginx-dir /etc/nginx/apps --root movies=/home/you/media/movies

# home control plane — execs api / scheduler / gateway / inventory / pull
mediaops-home
```
