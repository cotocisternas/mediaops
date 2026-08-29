# Grabber inventory

v1 stack after looking at 2025–2026 alternatives. Current seedbox tools are a starting inventory, not a sacred set — these are the picks.

## Apps

| Role | Pick | Notes |
| --- | --- | --- |
| TV | Sonarr | No replacement for the job. |
| Movies | Radarr | Same. |
| Music | Lidarr, version-pinned | Beets is a tagger, not a grabber. glibc pin stays a provider matrix row (live lesson: Lidarr 2.14.5). |
| Indexers | Prowlarr | Jackett is the legacy proxy; out. NZBHydra2 is redundant with one Usenet indexer. |
| Usenet | SABnzbd | nzbgetcom is maintained; switching cost > gain on this box. |
| Torrents | qBittorrent | WebAPI for categories, pausedUP/stoppedUP, seed-limit, path query before unlink. rtorrent is a worse API for seeding guards. |
| Profile packs | GrabPolicy via our *arr client (TRaSH/Recyclarr *data*) | Do not require the recyclarr binary; PUT via mediaopsd. |
| Subtitles | Bazarr | Out. Not even a stub. |
| IRC snatch | Autobrr | Out. Not even a stub. RSS via Prowlarr is enough. |

*arr HTTP never leaves localhost. mediaopsd is the remote surface. `grabber=None` is valid at runtime; this inventory is still complete.

## Completeness

Not five cherry-picked endpoints. Shared Servarr surface plus per-app resources plus download-client APIs. Home CLI must not stall on a missing endpoint because the daemon already has the surface.

| Abstraction | Intent |
| --- | --- |
| ArrClient | Shared Servarr HTTP: auth via discovered API key, `url_base`, system/health, commands, filesystem, diskspace, tags, notifications, backups, host/UI config, naming, media-management, quality + quality-definitions, custom formats, delay profiles, indexers, download clients, import lists, queue, history, blocklist, wanted/missing/cutoff, calendar, manual import, release search/grab, root folders. |
| App facades | Sonarr: series / season / episode / episodefile / parse. Radarr: movie / moviefile / collections / parse. Lidarr: artist / album / track / trackfile / metadata profile / parse. Prowlarr: indexers, applications (URLs **must** include `url_base`; doctor checks `/prowlarr/{id}/` not `/{id}/`), app-sync, proxies, search. |
| SAB client | Queue/history, add, pause/resume, complete-dir, categories `tv`/`movies`/`music` as schema asserted on SAB and on each *arr client, servers. Used for “Usenet complete → Copy → DeleteRemote”. |
| qBit client | Torrents list/properties/files/trackers, pause/resume, delete-with-data vs torrent-only, categories, preferences (DHT/PeX/LSD = doctor privacy invariant). Typed guard: before any remote library unlink, query qBit for that path; if seeding, skip. Torrent delete belongs to reclaim, never to sync-after-copy. |
| Upsert sets | Indexers and download clients are sets keyed by name (+ priority). Duplicate add = conflict, not append. Apply = set-diff: PUT missing, delete extras. Same for CF packs: re-PUT on apply so panel drift cannot stick. NZBgeek twice is the named failure. |
| GrabPolicy apply | Delay, quality, indexer/client priority, CF score packs — via API PUTs. Explicit command + diff. Not casual agent POSTs. Packs include Prefer H.264, HEVC last, 10-bit last, AV1 last. Quality-profile CF scores drifting after a panel visit: treat CF packs as desired state. |
| EdgeInvariant (API half) | Bind address, `url_base`, Forms auth — through APIs where possible. Nginx Host is ssh. Reconcile fails if any drift. Bind-to-star is a failure. |
| Key discovery | mediaopsd reads `config.xml` / `sabnzbd.ini` on the box. Never store masked `********`. Never display secrets. UI says key present. Zero secrets in git. Presence-boolean, rotate command, never echo. Test uses the discovered key. |
| Unmonitor / reconcile | Local FS is truth. After manual copy or pull, tell *arr. Do not trust *arr “file exists” as local exists. |
| Version pins | First-class. Compatibility matrix per provider OS. `upgrade lidarr` explains glibc and can refuse. Freeze updates in config when the matrix says so. Panel click is never an upgrade path. After install or upgrade, queue an edge check before success. |

## Must not

- Install packages or rewrite nginx (ssh/provider).
- Copy bytes (transfer).
- Walk torrent save paths as library files.
- Cherry-pick “just queue + history” and call it done.
- Echo API keys.
- Require Jackett or the Recyclarr binary.
- Stub Autobrr or Bazarr.
