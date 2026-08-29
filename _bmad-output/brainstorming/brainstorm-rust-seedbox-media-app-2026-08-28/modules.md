# mediaops — module / abstraction map

Product intent for the next BMad session (**spec**, then **architecture**). This is the intended crate cut and API surface: **what each module is for** and **what “full API” means**. Not OpenAPI. Not rustc-ready types. Architecture owns HOW the graph is wired.

Later memlog entries override earlier ones. Defaults that matter here:

- Formal API is **mediaopsd gRPC + mTLS** (self-signed certs at bootstrap), not SSH + localhost HTTP. Overlay is **in-process** (rathole the *idea*, not the app). Tailscale / WireGuard optional underlay (recommended).
- Data plane: **parallel Range RPCs** on that gRPC. Not FTP, not rsync, not rclone-as-pipe, not SFTP.
- Grabber stack kept: Sonarr, Radarr, Prowlarr, SABnzbd, qBittorrent, Lidarr-pinned. Jackett out. Recyclarr absorbed as GrabPolicy data.
- Bootstrap **seedbox** (install mediaopsd + overlay) and **local media dir**. No Jellyfin/Plex.

---

## Philosophy

| Law | Meaning |
| --- | --- |
| Thick libraries, thin CLI | Completeness lives in crates (`ArrClient`, typed rclone flags, two SSH clients). CLI never hits a missing endpoint and grows a one-off `curl`. |
| CLI is the API | Every capability is a clap subcommand. Cron/systemd call the CLI. `--json` on every command. Completions and TUI are skins, not a second API. |
| TUI is a skin | Optional attach. Progress is structured log. Timers must not require a TUI. Saturday cockpit, not the product. |
| Reconciler, not script runner | Desired-state document in the home repo. Plan is the default verb; run = plan + apply. Twice is a no-op if Edge, Grab, and Paths already match. |
| Two machines, one operator | Seedbox = rented working memory. Home disk = archive and playback surface. App owns the pipe. |
| Local FS is library of record | *arr is an optional grabber you tell the truth to. `grabber=None` is legal: sync a folder, name it, encode it. |
| Identity ≠ path | `TitleId` (TMDB/TVDB/MBID + kind). Folders are a rendered view. Sqlite maps TitleId → path → inode/hash so identity survives rename. |
| One-way pull | Remote HTTP Range → staging `.partial` → verify → atomic install. Remote delete only after local proof or reclaim. Never two-way. Never a third cloud. |
| Formal API is mediaopsd | Home CLI never speaks Sonarr HTTP. SSH is bootstrap. |
| External binaries are deps | mediaopsd (in-process overlay), ssh (bootstrap), ffmpeg/NVENC, systemd-user. Optional tailscale/wireguard. No rathole/rclone/rsync required for pull. |

---

## Crate cut

Intended workspace:

| Crate | Layer | Owns |
| --- | --- | --- |
| `mediaops-core` | types / laws | `TitleId`, `PathSchema`, desired-state, `Plan`, RPC schema, policies, provider **trait** |
| `mediaops-net` | overlay | gRPC listen / reverse-connect, mTLS CA+certs, reconnect; optional Tailscale/WG underlay |
| `mediaops-ssh` | bootstrap | `~/.ssh/config`, copy binary, systemd user unit, mint/place certs, Swizzin package/nginx **root** |
| `mediaops-arr` | grabber HTTP | Full Servarr + SAB + qBit; **only linked into mediaopsd** |
| `mediaops-daemon` | seedbox process | `mediaopsd`: gRPC+mTLS server, localhost *arr, Range RPCs (allowlist) |
| `mediaops-transfer` | home bytes | `PullFile`; parallel Range RPCs; probe concurrency; `.partial` |
| `mediaops-sync` | planner | What to Copy/Skip/Review; holds; reclaim; never torrent roots |
| `mediaops-encode` | home GPU | `EncodePolicy` execution; NVENC; reversible transcode |
| `mediaops-agent` | typed subprocess | `AgentTask`, dossier, claude/codex/grok |
| `mediaops-cli` | home binary | clap; talks to **local** mediaopsd only |

Home also runs mediaopsd (unix socket) so the CLI never holds overlay credentials in every invocation.

### Why transfer ≠ sync ≠ ssh

- Sync: **which** titles move.
- Transfer: **how** bytes move (gRPC Range RPCs).
- SSH: **how the daemon gets onto the box once.** Not the pipe.

`Push` is not a product surface.

### Intended dependency direction

```
core
 ├─ net
 ├─ ssh          (bootstrap only)
 ├─ arr          (linked into daemon, not CLI)
 ├─ daemon  ← core + arr + net     (seedbox + optional home)
 ├─ transfer ← core + net          (home Range RPCs)
 ├─ encode
 └─ agent
sync ← core + transfer
cli  ← core + daemon-client + sync + encode + agent + ssh-bootstrap
```

Architecture may shuffle impl crates; it must not put Sonarr HTTP in the CLI or FTP in transfer.

### Provider (no extra crate in v1)

| Piece | Where |
| --- | --- |
| `Provider` trait (`SwizzinBox`, `DockerCompose`, `AlreadyThere`) | `mediaops-core` |
| `AlreadyThere` | `mediaops-core` (no-op install; configure via APIs) |
| `SwizzinBox` | `mediaops-ssh` (root: packages + nginx) |
| `DockerCompose` | stub + tests only |
| Ultra.cc / QuickBox | **non-goal for v1** — unimplemented trait variants, not ports |

v1 provider = this SeedIt4Me/Swizzin box + this home disk. Version pins are first-class (e.g. Lidarr 2.14.5 / glibc matrix). Panel click is never an upgrade path.

---

## `mediaops-core`

**Purpose.** Shared language of the system: identity, paths, desired state, plans, budgets, policy **data**, job state. No SSH, no HTTP, no rclone child, no ffmpeg.

### Key abstractions

| Abstraction | Intent |
| --- | --- |
| `TitleId` | Kind + TMDB/TVDB/MBID. Folder/file names are `PathSchema` renderings, not identity. Music remaster year is an identity rule (Relayer 1974 vs Relayer.(2013)), not a clever year compare. |
| `PathSchema` | Versioned grammar → `PathBuf`s. **Single writer** for: generated docs (`mediaops docs render` / AGENTS.md), install gate, agent proposals, lint. Composable parsers (movie / episode / track) + explicit reject bins (`needs-split`, `needs-year`). Scene-tag strip list with tests from real sins (REPACJ, REPACK, PROPER). Round-trip law: `parse(render(id)) == id` (year in folder and file the same way). Remote **allowlist of roots**; unknown paths error. Walk never follows symlinks off the allowlist. Never torrent save paths. |
| Desired-state document | Only user-facing config. Lives in the home repo; the box is disposable. Every former `apply-stack.py` flag is a typed field. Snapshotted at plan start; no hot-reload mid-copy. Cloneable: same binary, different file. |
| `Plan` + actions | First-class artifact. Actions at least: `Copy`, `Skip`, `Review`, `Unmonitor`, `DeleteRemote`, `Encode`, `Reclaim`, plus edge/grab apply as reconcile steps. Plan default; run = plan + apply. Unified diffs of ini/xml/nginx before write. |
| Job state machine | Jobs have states. Timers only `make-progress-on-ready-jobs`. No overlapping hope-cron. Config snapshot is per-plan. |
| `ResourceBudget` | `max_copy_gib`, `min_free_gib` (watermark), `max_nvenc`, lock. No magic numbers in source. Preflight fail if a copy would breach watermark. |
| `GrabPolicy` | Data: delay, quality, indexer priority, client priority, versioned custom-format packs (Prefer H.264, HEVC last, 10-bit last, AV1 last). Applied by `mediaops-arr`, not clicks. Changes are explicit commands with a diff. |
| `EncodePolicy` | `(codec, depth, container, hdr)` → `Keep` \| `NvencH264` \| `Refuse`. HDR/DV and 2160p remux are Keep-forever. Playback matrix: client profiles × codec (config), without a Plex/Jellyfin **client**. Upgrade class is never \| if-better-profile \| if-user-marked-UHD; default never. |
| `ReclaimPolicy` | Constraints (private, seeding, imported) × objective (free-space percent). Replaces leftover timers. Never touches private-under-goal. |
| `Hold` | Typed inbox: Approve / Reject / Research. Not a folder a media server might ingest. Reject = never pull this release, let *arr try another. Approve = promote to library. Auto-approve forbidden; agent recommend needs a confidence floor. |
| `Provider` | Install/remove/pin packages. v1: Swizzin + AlreadyThere. |
| `EdgeInvariant` | bind `127.0.0.1` + `url_base` + Host `$host` + Forms auth. Types here; checks in arr (API) + ssh (nginx). |
| `Connection` | SSH bootstrap `{host, port, user, auth}` plus gRPC bind (host/port) and mTLS identity. Optional Tailscale/WG underlay. SeedIt4Me `2097` is bootstrap, not the API. |
| Title index | Local sqlite: TitleId ↔ path ↔ inode/hash. Export/import for new-machine bootstrap even before files exist. |
| Wants / priority | Music-first then videos under budget is a planner law (generalize to a wants queue). Not a hardcode inside `pull()`. |
| Lock | Machine-global flock: pid, started_at, command. Lock conflict = distinct exit code + skip-with-reason. |

### Must NOT

- Talk to the box, *arr, rclone, or ffmpeg.
- Embed path strings as identity.
- Know Swizzin panel URLs or rclone flag names.
- Configure Jellyfin/Plex (schema is a **naming** contract, not a media-server installer).

### Replaces (as types, not as I/O)

Laws currently trapped in comments of `apply-stack.py`, `sync.py`, `encode_h264.py`, reclaim, review, `SEEDBOX.md` / AGENTS.md. **Docs are generated from `PathSchema` so they cannot lie.**

---

## `mediaops-net`

**Purpose.** Listen, accept, reconnect — **inside mediaopsd**. This is the rathole *idea* (managed bind + auth + stay-up). **Do not ship or install the rathole app.**

| Mode | When |
| --- | --- |
| **gRPC + mTLS listen (default)** | mediaopsd **binds**. Self-signed CA, server cert, client cert generated at bootstrap. Bearer token optional second factor; **mTLS is the preference**. Seedbox (reachable) is the usual bind side; home is the client. |
| **Reverse-connect (same binary)** | If the bind side is NATed, the far side dials out and the local side accepts — still gRPC/mTLS. Not a third-party client. |
| **Tailscale / WireGuard** | Optional, recommended **underlay**. Mesh without publishing a port. Userspace Tailscale if no TUN. Not required. |
| Cloudflare Tunnel / rathole / frp **apps** | **Not used.** |
| `ssh -L` / `-R` | Bootstrap last-ditch, then gone. Not the product. |

Identity is the bootstrap CA. Reconnect and health live here.

### Must NOT

- Shell out to rathole, frp, or cloudflared.
- Expose *arr ports. Only mediaopsd gRPC.
- Carry 80 GiB through Cloudflare.
- Be a SOCKS wrapper the CLI uses to curl Sonarr.

---

## `mediaops-ssh`

**Purpose.** Bootstrap and Swizzin **root** operations only: copy `mediaopsd`, systemd user unit, overlay enroll, `box install` / nginx files. After enroll, doctor/apply/copy do not use SSH.

Mux ControlMaster is fine for short bootstrap exec. **Never** bulk copy over SSH.

### Key abstractions

| Abstraction | Intent |
| --- | --- |
| `SshConfig` import | `~/.ssh/config` `Host seedbox` |
| Bootstrap exec | Install daemon + overlay. Root only for nginx + packages |
| `SwizzinBox` | Provider impl: packages, pins, nginx. Edge check after install |
| Edge files | Hash nginx app confs; `EdgeRepair` transaction |

### Must NOT

- Be the control API.
- Local-forward *arr for apply.
- Own `.partial` or rclone.

### Replaces

First-time `scp` of `apply-stack.py` / `fix-nginx.sh`. Not the daily path.

---

## `mediaops-daemon`

**Purpose.** The seedbox (and home) long-running service. **This is the formal API.**

| Surface | Intent |
| --- | --- |
| gRPC | Plan, apply, doctor, holds, df, qBit guard, key discovery, GrabPolicy — **mTLS required** |
| Range RPCs | Parallel `GetRange` for **allowlisted** library/usenet-complete paths. Same listener, same certs. No torrent-root listing |
| Local *arr | `mediaops-arr` against `127.0.0.1` + url_base. Never bind *arr off localhost |

systemd `--user` on the seedbox. Home CLI talks to home mediaopsd over a unix socket; home mediaopsd is the overlay client.

### Must NOT

- Require the operator to `ssh -L 8989`.
- Serve `_incoming` or torrent incomplete.
- Run encode (home GPU).

### Replaces

“SSH in and curl localhost”; ad-hoc forwards; treating nginx `/sonarr` as the automation API.

---

## `mediaops-arr`

**Purpose.** Full HTTP clients for the grabber stack, **linked only into mediaopsd**. The home CLI must not stall on a missing endpoint because the daemon already has the surface. *arr is optional at runtime (`grabber=None`); this crate is still complete.

### Apps in scope

| App | Role |
| --- | --- |
| Sonarr | Series grabber |
| Radarr | Movies grabber |
| Lidarr | Music grabber |
| Prowlarr | Indexer manager / app sync |
| SABnzbd | Usenet download-client API |
| qBittorrent | Torrent download-client API |

Not five cherry-picked endpoints. Shared **Servarr** surface plus per-app resources plus download-client APIs.

### Key abstractions

| Abstraction | Intent |
| --- | --- |
| `ArrClient` | Shared Servarr HTTP: auth via discovered API key, `url_base`, system/health, commands, filesystem, diskspace, tags, notifications, backups, host/UI config, naming, media-management, quality + quality-definitions, **custom formats**, delay profiles, **indexers**, **download clients**, import lists, queue, history, blocklist, wanted/missing/cutoff, calendar, manual import, release search/grab, root folders. |
| App facades | Sonarr: series / season / episode / episodefile / parse. Radarr: movie / moviefile / collections / parse. Lidarr: artist / album / track / trackfile / metadata profile / parse. Prowlarr: indexers, **applications** (URLs **must** include `url_base`; doctor checks `/prowlarr/{id}/` not `/{id}/`), app-sync, proxies, search. |
| SAB client | Queue/history, add, pause/resume, complete-dir, **categories** `tv`/`movies`/`music` as schema asserted on SAB and on each *arr client, servers. Used for “Usenet complete → Copy → `DeleteRemote`”. |
| qBit client | Torrents list/properties/files/trackers, pause/resume, **delete-with-data vs torrent-only**, categories, preferences (**DHT/PeX/LSD** = doctor privacy invariant). **Typed guard:** before any remote library unlink, query qBit for that path; if seeding, skip. Torrent delete belongs to **reclaim**, never to sync-after-copy. |
| Upsert **sets** | Indexers and download clients are sets keyed by name (+ priority). Duplicate add = conflict, not append. Apply = set-diff: PUT missing, delete extras. Same for CF packs: re-PUT on apply so panel drift cannot stick. |
| `GrabPolicy` apply | Delay, quality, indexer/client priority, CF score packs — via API PUTs. Explicit command + diff. Not casual agent POSTs. |
| `EdgeInvariant` (API half) | Bind address, `url_base`, Forms auth — **through APIs where possible**. Nginx Host is ssh. Reconcile fails if any drift. |
| Key discovery | mediaopsd reads `config.xml` / `sabnzbd.ini` on the box. **Never** store masked `********`. Never display secrets. UI says **key present**. Zero secrets in git. |
| Unmonitor / reconcile | Local FS is truth. After manual copy or pull, tell *arr. Do not trust *arr “file exists” as local exists. |

### Must NOT

- Install packages or rewrite nginx (ssh/provider).
- Copy bytes (transfer).
- Walk torrent save paths as library files.
- Cherry-pick “just queue + history” and call it done.
- Echo API keys.

### Replaces

Configure-apps half of `apply-stack.py` (indexers, clients, quality, CF, categories, host/url_base). Panel clicking. Duplicate NZBgeek append. Masked-key Test.

---

## `mediaops-transfer`

**Purpose.** One-way pull of files from seedbox to home that actually fills the WAN pipe.

**Picked:** parallel **Range RPCs** on mediaopsd gRPC/mTLS. Same lesson as rclone multi-thread, implemented in this crate + the daemon. FTP PASV, SFTP, rsync, and rclone-as-the-pipe were researched and rejected.

### Trait

`PullFile` (one-way). Staging `.partial`, verify, atomic rename. `*.partial` sacred. Resume lists and continues.

`Push` is out of product scope.

### Client (home)

| Surface | Intent |
| --- | --- |
| Remote spec | gRPC endpoint + client cert from bootstrap; allowlist prefix |
| Operations | `Stat`, `GetRange`, listing **via RPC** (not HTML indexes, not torrent trees) |
| Concurrency | N parallel Range RPCs; many files vs many ranges on one file; do not collapse onto one TCP |
| Probe + persist | Raise N until throughput plateaus; persist |
| Staging | Destination `.partial` then rename |

Emergency rsync-ssh may exist unadvertised. Not FTP. Not SFTP. Not rclone.

### Must NOT

- Decide Copy vs Skip vs Hold (sync).
- Delete torrents.
- Use FTP PASV, per-file SFTP, or shell rclone for the overlay pull.
- Copy to a second cloud.
- Follow symlinks into torrent trees.

### Replaces

Byte-move half of `sync.py` (the live FTP `rclone copyto` is the thing we are **leaving**).

---

## `mediaops-sync`

**Purpose.** Reconcile remote buffer vs local archive. **Planner**, not the pipe. Consumes `PullFile`. *arr/unmonitor/SAB/qBit go through mediaopsd RPC, not a local HTTP tunnel.

### Key abstractions

| Abstraction | Intent |
| --- | --- |
| Planner | Walk allowlisted remote roots only. Never torrent roots, never `incomplete`. Produce a `Plan`. Music-first then video under `--max-gb` / remaining-home-disk. |
| Copy path | Remote → staging `.partial` → verify → atomic install. Usenet complete after Copy: `DeleteRemote`. Library hardlink of a torrent: **leave the torrent**. |
| Skip vs surplus | Skip ≠ surplus. Surplus = remote may go **after local hash proof**. |
| Holds inbox | `importBlocked` etc. is a product feature: *arr message + ffprobe + approve/reject/research. Age, size, reason. CLI/TUI inbox. Needs-split is a workflow (agent), not a pile. |
| Install gate | Schema parser is the only writer. Spaces, leftover scene tags, `Season 1` folders: lint-on-install rejects. `library lint` finds survivors. |
| Reclaim execution | Policy from core. Preview ranked by ratio, private, age. qBit guard before unlink. Dry-run or it does not exist (no silent `seedbox-reclaim.timer`). |
| Resume | `sync resume`: list `.partial`, continue. GC never deletes them. |
| Reconcile | After manual copy: tell *arr the truth (`Unmonitor` / imported). |

### Must NOT

- Embed rclone flag strings.
- Two-way sync or mirror local deletes to the seedbox.
- Delete qBit data on Copy.
- Walk `/torrents` as archive.
- Auto-approve holds.
- Encode (enqueue `Encode` actions only).
- Speak Sonarr HTTP from the planner.

### Replaces

`sync.py` / media-sync planner laws; **review** hold inbox; **reclaim** execution (not the leftover no-op timer).

---

## `mediaops-encode`

**Purpose.** Home-GPU transcode so Chrome/TV can play. Seedbox is dumb disk + pipe — **never encode on seedbox CPU**. Policy packs drive the encoder so docs cannot lie (`encode_h264.py` skipped H.264 10-bit while AGENTS.md said otherwise).

### Key abstractions

| Abstraction | Intent |
| --- | --- |
| Policy execution | Core `EncodePolicy` + playback matrix in config. Scan rules are explicit (e.g. `movies/**/*.mp4` HEVC10; **series-skip is a named rule**, not an accident). |
| Hardware probe | NVENC concurrency from probe, not hardcoded 8. This box caps at 3; budget `max_nvenc` is the ceiling. Semaphore is visible/pausable queue, not a surprise after sync. |
| Reversible transaction | Write `.converting`, replace, move original to backup-hevc-originals. Never delete original until replace succeeds. |
| Refuse class | HDR/DV, 2160p remux: Keep-forever. Do not transcode Avatar DV to H.264. |
| Runtime deps | ffmpeg / jellyfin-ffmpeg / NVENC as **probed externals**, not vendored. |

### Must NOT

- Configure Plex/Jellyfin clients, users, or libraries. Client profile names in config are encode inputs, not a media-server API.
- Run on the seedbox.
- Invent quality upgrades when disk is bored (upgrade is per-title class).
- Be the schema writer (install already happened).

### Replaces

`encode_h264.py`.

---

## `mediaops-agent`

**Purpose.** Typed LLM tools for research/debug so the operator does not open 12 tabs. An LLM is a **subprocess**, not an in-process brain. May **propose**; `PathSchema` is the only writer.

### Key abstractions

| Abstraction | Intent |
| --- | --- |
| `AgentTask` | `{prompt_template, inputs, output_schema, timeout, cwd sandbox, binary}`. Binary = claude \| codex \| grok (pick per task: research may web-search; debug-media may be local-file). App only cares that output matches schema. |
| Dossier | Per-title bundle: ffprobe, nfo, *arr payload, neighbors — **not** the universe. Max-bytes budget per task. |
| Propose vs apply | Default dry-run. Apply is an explicit grant. Schema parser rejects spaces/scene tags even if the model invents them. |
| Capabilities | Tokens from ssh/arr/fs/probe. Default read-only: ffprobe, ls, *arr GET. `ArrPost` / `SshExecAllowlist` are grants. Never ssh root. GrabPolicy changes are not casual POSTs. |
| Jobs | `research` (scene name → TitleId without download; theatrical-cut / edition / runtime vs ffprobe); `debug-media` (ffprobe + mediainfo + a **local** client profile → Direct Play vs transcode **advice**, no Jellyfin API); hold Research. |

### Must NOT

- Write library paths except through schema-validated apply.
- Dump the whole tree into context.
- Hold a ControlMaster as root.
- Be the TUI.

### Replaces

Tribal “open 12 tabs” research; informal review of mystery filenames. Does not replace `review` as a queue — it is a **verb** on a hold.

---

## `mediaops-cli`

**Purpose.** One binary on PATH (`mediaops`). Composition root. Thin: parse, snapshot config, take lock, call libraries, print `--json` or human, optional TUI attach.

### Command surface (every capability is a subcommand)

| Area | Subcommands (intent) | Modules |
| --- | --- | --- |
| Seedbox | `seedbox bootstrap`, `doctor`, `repair edge`, `apply`, `df`, `upgrade`, `ui <app>` | ssh bootstrap, net, daemon RPC |
| Library | `library bootstrap`, `lint`, `relocate` | core, encode probe, transfer remote, systemd |
| Plan/run | `plan`, `run`, `status`, `sync resume` | sync, transfer, arr |
| Holds | `hold list\|approve\|reject\|research` | sync, agent |
| Reclaim | `reclaim preview\|apply` | sync, arr, ssh |
| Encode | `encode scan\|run\|pause` | encode |
| Transfer | `transfer probe` | transfer, net, daemon file HTTP |
| Why / watch | `why TITLE`, `watch TITLE` | sync + arr + encode (trace grab→import→hold→pull→encode→library path; no Jellyfin URL as a product requirement) |
| Agent | `agent research\|debug` | agent |
| Docs | `docs render` | core `PathSchema` |
| Cockpit | `tui` | skin over the same commands |

Doctor **scheduled** is read-only. Write repairs need `--repair` + local confirm flag or pin. `doctor --repair` from a public laptop unattended is a failure mode.

Idempotent apply. Distinct lock exit code. Structured log. systemd-user adapter: **oneshot + `OnUnitInactiveSec` + flock**, all three. Do not invent cron math. v1 scheduler = that adapter only.

### Must NOT

- Contain *arr HTTP or overlay internals. Talk to local mediaopsd.
- Be the only UI (TUI-only is a failure mode).
- Start a public status daemon. `status --json` is localhost or `ssh mediaops status`.

### Replaces

Python venv, `PYTHONPATH`, one-shot wizards that only work once (every wizard step is a reconcile).

---

## Bootstrap surfaces

Two first-class commands. **Not** a media-server installer.

| Concern | `seedbox bootstrap` | `library bootstrap` |
| --- | --- | --- |
| Goal | New box: connection, provider, packages, desired-state, mint mTLS certs, bind gRPC | New home disk: schema, watermarks, timer, lock, client cert, state |
| `core` | Validate/write desired-state; remote PathSchema roots allowlist; version pins | Create schema dirs (`movies`/`series`/`music` plus app-managed `_ops` / `_incoming` — **not** libraries); watermarks; sqlite index; generate docs; refuse if disk below watermark |
| `ssh` | Bootstrap: install mediaopsd, mint/place mTLS certs, `SwizzinBox` packages/nginx | — |
| `net` | gRPC listen (optional reverse-connect; optional Tailscale/WG underlay) | gRPC client to seedbox |
| `daemon` | mediaopsd running; gRPC + Range RPCs | Home mediaopsd unix socket |
| `arr` | Inside mediaopsd: discover keys, upsert indexers/clients, GrabPolicy, EdgeInvariant API | Optional: empty grabber is fine |
| `transfer` | Probe Range RPC concurrency over gRPC; persist N | staging dirs |
| `encode` | — | ffmpeg/NVENC probe; persist `max_nvenc` |
| `sync` | — | Ready to plan (no copy during bootstrap unless asked) |
| `agent` | — | — |
| `cli` | Lock, `--json`, diffs, confirm | systemd-user timer + flock; `library --relocate` rewrites schema roots, systemd, title-index paths |
| Explicitly out | Jellyfin/Plex, Ultra.cc, public WebUI | Adding `_incoming` as a media-server library; wizard-once |

`new-machine`: export desired-state + title index; import bootstraps layout even before files exist.

---

## Tribal scripts → modules

| Tribal | Becomes |
| --- | --- |
| `apply-stack.py` | **core** desired-state fields + **ssh** packages/nginx + **arr** full apply (sets, CF, host, categories) |
| `fix-nginx.sh` | **ssh** edge files + hash/drift; **cli** `seedbox repair edge` transaction with apply-stack unit |
| `sync.py` / media-sync | **sync** planner + **transfer** Range RPCs + **daemon** gRPC |
| `encode_h264.py` | **encode** (policy-driven; 10-bit included when policy says so) |
| reclaim (incl. leftover timer) | **core** `ReclaimPolicy` + **sync** execution + **arr** qBit guard; dry-run or remove |
| review / holds | **sync** hold queue + **cli** inbox + **agent** research verb |
| `SEEDBOX.md` / AGENTS.md hand-edits | **core** `PathSchema` → `docs render` |

Every script is a **transaction type in a Plan**, not a wrapped subprocess of the old Python.

---

## Test strategy

**Unit tests never require the live box.**

| Kind | What |
| --- | --- |
| Cassettes | Recorded *arr / SAB / qBit JSON fixtures. Client tests replay HTTP. |
| Tree fixtures | Directory layouts for parser, lint, planner (Copy/Skip/Hold/surplus), `.partial` resume, symlink-off-allowlist. |
| Schema CI | `parse(render(id)) == id`; scene-tag strip cases from real sins (REPACJ, …). |
| Transfer | Range RPC concurrency; probe persistence; `.partial`; no network. |
| Net | Overlay enroll dry-run; never publish *arr ports. |
| SSH | Bootstrap-only; bulk copy over SSH is a test failure. |
| Provider | `DockerCompose` / Ultra stubs: unimplemented + tests, not silent no-ops. |
| Failure history as suite | Panel Host 302, ControlMaster starvation, HEVC-MP4 Chrome, holds, Lidarr glibc, masked keys, duplicate indexers, docs vs code, qBit seeding delete, `_incoming` as library, overlapping timers, watermark, lock conflict exit code. |
| Integration (optional, not unit) | Live box behind an explicit feature/env; never default CI. |

---

## Non-goals (this map)

| Out | Why |
| --- | --- |
| **No Plex/Jellyfin client** | No libraries, users, playback-client APIs, or “prints a Jellyfin URL” as a requirement. Naming schema still exists; media server is out of scope. |
| **No rathole/frp/rclone/rsync as the pipe** | Overlay and Range RPCs are mediaopsd. No third-party tunnel binary. |
| **No v1 Ultra.cc provider** | One provider well (`SwizzinBox` + `AlreadyThere`). Others = unimplemented trait + tests. |
| No two-way sync / third cloud | Home archive only. |
| No SaaS / extra public daemon | One binary; localhost or SSH. |
| No TUI-only product | CLI is the API. |
| No secrets in git | Discover or keyring. |

---

## Spec / architecture handoff

- **Spec** session: machine contract for mediaopsd **gRPC + mTLS** (certs at bootstrap), in-process overlay (no rathole app), Range RPCs, bootstrap both sides, Edge/Grab/Path laws. Use this map for *what exists*.
- **Architecture** session: crate graph HOW, RPC schema, cassette layout, CLI tree. Do not re-open SSH-as-API or FTP-as-default.
