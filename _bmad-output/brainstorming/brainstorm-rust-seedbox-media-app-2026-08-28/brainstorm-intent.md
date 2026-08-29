# mediaops — intent

**One-line:** CLI-first reconciler of desired state across a rented seedbox (working memory) and a home disk (archive and playback surface).

## Who and why

One operator, two machines. Replace the tribal script pile (`apply-stack.py`, media-sync, `encode_h264.py`, leftover reclaim timers, `SEEDBOX.md` that can lie) with one contained system that encodes the laws in types, tests, and generated docs.

The seedbox is disposable rented buffer. The home filesystem is the library of record. *arr is an optional grabber you tell the truth to — not the catalog.

## Jobs that matter

- **Tonight-playable** — title is monitored, grabbed, pulled, encoded if needed, ready at home.
- **Quiet box** — days without SSHing the panel; doctor/repair when the panel rewrites nginx.
- **Why isn't this here** — trace grab → import → hold → pull → encode → library.
- **Resume, not restart** — `.partial` is sacred; a transfer that dies at 90% continues.
- **Bootstrap either side** — a new seedbox *or* a new local media directory from desired-state. Not a media-server installer.

Also: holds as a real inbox (approve / reject / research), reclaim only after local hash proof, a planner that honors music-this-week, agent research/debug without twelve tabs.

## Product shape

A **CLI-first reconciler**. Desired-state lives in the home repo; the box is disposable. **Plan is the default verb**; run = plan + apply. Config is snapshotted at plan start.

systemd user **oneshot + `OnUnitInactiveSec` + flock** is the unattended product. TUI is an optional Saturday skin. `--json` on every command. Import `~/.ssh/config` Host `seedbox` — do not invent another alias format.

Every tribal script becomes a transaction type in a Plan: apply-stack, fix-nginx, copy, skip, review, unmonitor, delete-remote, encode, reclaim, reconcile.

## In scope

- **Bootstrap seedbox:** connection, provider, packages, desired-state, paths, indexer/client sets, edge invariants, version pins.
- **Bootstrap local media dir:** PathSchema dirs, watermarks, timer, lock, rclone remote, title-index state. `_ops` / `_incoming` are app-managed, never libraries.
- Seedbox **mediaopsd** (installed service): formal RPC API. *arr/SAB/qBit stay localhost-only.
- **mediaopsd binds gRPC** (mTLS, self-signed CA+certs generated at bootstrap). That is the overlay — the rathole *idea* (listen, reconnect, auth), not the rathole app. Tailscale / WireGuard optional (recommended). SSH is bootstrap only.
- Sync planner that honors transfer, encode, reclaim, and hold laws.
- Home-GPU encode queue (probe-capped NVENC).
- Doctor (scheduled = read-only) and edge repair (writes need confirm or pin).
- LLM agents as subprocesses with capability tokens and a title dossier.

## Out of scope

- **Configuring Jellyfin or Plex** (libraries, users, playback clients) — explicit non-goal.
- Multi-panel / multi-provider product in v1 (trait exists; unimplemented).
- Custom file-transfer protocol.
- Third-copy / second-cloud archive.
- Public WebUI, LAN-exposed forwards, extra always-on daemon if SSH + CLI suffice.
- Agents with SSH root or casual *arr POST.

## Hard laws

1. **One-way pull.** Remote delete only for surplus after local proof. Never mirror local deletes to the box.
2. **Never torrent roots.** Allowlisted remote paths only; unknown paths error; never follow symlinks off the allowlist; never walk `torrents/incomplete`.
3. **Music-first**, then videos under `ResourceBudget`. Planner priority (later: wants queue) — not a hardcode in pull.
4. **Usenet: delete after copy. Torrents: leave while seeding.** Torrent delete belongs to reclaim, never to sync. Before any remote library unlink, query qBit; if seeding, skip.
5. **Holds are an inbox**, not a library. Approve / Reject / Research. No auto-approve; an agent recommend needs a confidence floor. Reject = never this release; let *arr try another.
6. **Scheduler:** oneshot + `OnUnitInactiveSec` + flock — all three. Lockfile has pid / started_at / command. Lock conflict is a distinct exit code, never silent 0.
7. **PathSchema is the single writer.** Versioned grammar → paths, install gate, generated docs / `AGENTS.md`, lint. `parse(render(id)) == id`. Scene-tag strip is schema (tests from real sins: REPACJ, REPACK, PROPER). Spaces refuse. Agents **propose**; schema **writes**.
8. **Identity is TitleId** (TMDB / TVDB / MBID + kind), not path. Local sqlite maps TitleId → path → inode/hash so identity survives renames. Music remasters: MBID, not folder year (Relayer 1974 vs Relayer.(2013)).
9. **Copy path:** remote → staging `.partial` → verify → atomic install into a schema path. Partials are sacred; empty-dir prune and GC never delete them. Crash recovery lists `.partial` and continues.
10. **Local FS is source of truth.** *arr file-exists is not local-exists; tell *arr to unmonitor. `grabber=None` is valid (sync / name / encode a folder).
11. **EdgeInvariant:** bind `127.0.0.1` + `url_base` + `Host $host` + Forms auth. Panel is an untrusted writer; hash nginx app confs. Repair is one transaction (diff, then fix-nginx + apply-stack). After install/upgrade, queue an edge check. Prowlarr app URLs must include `url_base`.
12. **mediaopsd is the formal API.** Home CLI → local mediaopsd → gRPC/mTLS → seedbox mediaopsd → localhost *arr. SSH is bootstrap (copy binary, systemd user unit, generate and place certs) only. No SSH `-L`. No rathole/frp/tailscale **binary** required. **Optional, recommended: Tailscale or WireGuard** as an underlay if you want a mesh without publishing a port. Not Cloudflare Tunnel for bulk.
13. **Secrets:** zero in git. Discover API keys from remote `config.xml` / `sabnzbd.ini` at runtime; never store or echo `********`. Test uses the discovered key; UI is a presence boolean.
14. **Encode at home GPU.** Seedbox is dumb disk + pipe. HDR/DV and 2160p remux are Keep-forever (refuse-to-encode). Encode is reversible: `.converting` → replace → original to backup; never delete until replace succeeds. Playback matrix (client × codec) yields keep / encode / refuse. Series-skip of HEVC-MP4 is an explicit rule. Upgrade default is **never** unless if-better-profile or user-marked-UHD.
15. **Budgets in config**, no magic numbers: `max_copy_gib`, `min_free_gib`, NVENC from GPU probe (this box caps at 3), watermark preflight fail. Root only for nginx and package install.
16. **Indexers/clients are sets** (upsert-by-name; duplicate = conflict). Categories `tv` / `movies` / `music` asserted on SAB and each *arr client. Custom-format packs are desired state (re-PUT on apply). Version pins are first-class (Lidarr 2.14.5 / glibc matrix); upgrade is a conscious transaction that can refuse. qBit DHT/PeX/LSD off is a doctor invariant. Reclaim is a real policy (private × seeding × imported × free-space objective) or it is removed — never a silent leftover timer.
17. **Agents:** subprocess `{prompt_template, inputs, output_schema, timeout, cwd sandbox, binary}`. Capability tokens (ReadFs, ProbeMedia, ArrGet, ArrPost, SshExecAllowlist). Read-only probes by default; Apply is an explicit grant. Dossier, not the universe; max-bytes budget. Dry-run default.

## Control vs data plane

**Control:** gRPC on `mediaopsd` with **mTLS**. Self-signed CA, server cert, client cert generated at bootstrap and stored in the home keyring / desired-state (never git). A bearer token is allowed as a second factor; mTLS is the preference. The *arr HTTP APIs stay behind the daemon.

**Data:** parallel **Range RPCs** on the same gRPC/mTLS (the rclone lesson: many TCP streams, HTTP Range-shaped chunks). Implemented in `mediaops-transfer` + mediaopsd. Not a second HTTP server, not `rclone serve`, not FTP, not rsync.

**Picked (research, not "what's already running"):**

| Job | Pick | Not |
| --- | --- | --- |
| Overlay | **mediaopsd gRPC + mTLS** (in-process). Reverse-connect mode if the bind side is NATed. Tailscale / WG optional underlay | rathole/frp/cloudflared **apps**, SSH `-L` |
| Bulk copy | Parallel Range RPCs on that gRPC | FTP, rsync, SFTP, rclone-as-the-pipe |
| TV / movies / indexers | Sonarr, Radarr, Prowlarr | Jackett |
| Music grabber | Lidarr, version-pinned | beets as grabber |
| Usenet | SABnzbd | NZBGet (alive as nzbgetcom, switching cost > gain on this box) |
| Torrents | qBittorrent | rtorrent (worse API for seeding guards) |
| Profile packs | GrabPolicy via our *arr client (TRaSH/Recyclarr *data*) | requiring the recyclarr binary |
| Autobrr / Bazarr | optional | not core |

Probe concurrent Range RPCs at bootstrap; persist N. Many FLACs → many files in flight; huge MKV → many ranges on one file. Each range is its own HTTP/2 stream or connection — do not collapse onto one TCP and idle the WAN.

The job is **home archive**, not a third cloud copy.

## Implementation philosophy (intent)

**Thick libraries, thin CLI.** Complete Servarr API clients (used **on the seedbox** by mediaopsd), gRPC/mTLS in mediaopsd, parallel Range client on home, SSH **bootstrap** only.

Orchestrator, not vendor: ssh (bootstrap), ffmpeg/NVENC, systemd are probed. Overlay is **in-process**. Optional tailscale/wireguard. rclone is not required for the seedbox pull.

Tests: cassette fixtures of *arr JSON and directory trees; the live box is not required for unit tests. Failure history (panel Host, ControlMaster, HEVC-MP4, holds, glibc, masked keys, duplicate indexers, docs vs code, REPACJ) **is the test suite**.

Crate graph and full API HOW belong to **architecture**, after spec.

## v1 boundary

**This SeedIt4Me / Swizzin box + this home disk.** Provider trait (`SwizzinBox`, `AlreadyThere`, later DockerCompose / other panels) — v1 ships Swizzin + AlreadyThere only.

The new crate is the repo of record; the live `~/videos` tree is a deploy target.

Connection is `{host, port, user, auth}`; SeedIt4Me port 2097 is an instance, not architecture.

## Next

1. **`bmad-spec`** — machine contract. The idea is decided; a product-brief would rediscover known jobs.
2. **`bmad-architecture`** — crate graph, mediaopsd RPC, overlay, HTTP Range transfer HOW.
