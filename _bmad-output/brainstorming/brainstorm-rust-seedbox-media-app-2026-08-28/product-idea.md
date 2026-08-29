# Product idea: mediaops

A contained Rust CLI that reconciles a desired-state document across a rented seedbox and a home archive disk. It replaces the live tribal scripts (`apply-stack.py`, media-sync, `encode_h264.py`, reclaim, `SEEDBOX.md`) with types, tests, generated docs, and full API clients.

This file is the complete written idea. The next BMad session is **spec**, not product-brief. Spec should absorb the load-bearing claims, constraints, non-goals, and success signals here; architecture after spec owns crate graph detail and full API HOW.

**Name (working):** `mediaops` — one binary on `PATH` via cargo. No Python venv, no `PYTHONPATH`.

---

## Why

### The world

Two machines, one operator.

The **seedbox** is rented working memory: a Swizzin/SeedIt4Me box that grabs via the *arr stack (Sonarr, Radarr, Lidarr, Prowlarr), Usenet (SABnzbd), and torrents (qBittorrent). It is disposable. Its disk is a buffer, not the library. Desired state does not live there.

The **home disk** is the archive and the playback surface. The local filesystem is the library of record. Folder names are a rendered view of title identity, written so a media server *could* ingest them. The app does not install, configure, or manage Jellyfin or Plex.

**Roles, not illusions:**

| Role | What it is | What it is not |
| --- | --- | --- |
| Seedbox | Rented buffer + grabber host | Library of record, playback GPU, source of desired state |
| *arr stack | Grabber (search, grab, import, unmonitor) | Truth about what exists at home |
| Local filesystem | Library of record | A mirror that pushes deletes back to the seedbox |
| Home GPU | Encode capability (NVENC) | Something the seedbox CPU should do |
| SSH | Bootstrap only (install daemon, enroll overlay) | The formal API |
| mediaopsd | Formal control API on the box | A thin SSH+curl to *arr |
| Overlay (mediaopsd gRPC/mTLS; Tailscale / WireGuard optional) | Daemon binds; bootstrap-generated certs | Ad-hoc `ssh -L`; rathole/frp **apps** |
| nginx URL-bases | Already the UI edge on the box | Something the home app should bind on `0.0.0.0` |
| Desired-state file | The only user-facing config; lives in the home repo | A wizard that only works once; a panel click history |

The operator is not hiring a seedbox control panel. They are hiring tonight-playable media, a quiet box (days without SSHing to the panel), a way to ask *why* when something is weird, and resume-not-restart when a 200 GiB copy dies at 90%. Install and remove of *arr/usenet/torrent is in service of those jobs, not the product.

### Why scripts fail as a genre

The live tree under `~/videos` plus `apply-stack.py`, media-sync, encode, reclaim, and `SEEDBOX.md` encode tribal knowledge in comments. Failure history is already the test suite, not folklore: panel Host rewrites that 302 to localhost; ControlMaster starvation of a 200 GiB copy; HEVC-MP4 Chrome dropped frames; holds rotting as a junk drawer; Lidarr glibc traps; masked `********` keys pasted into Test; duplicate NZBgeek; `SEEDBOX.md` lying about scan paths; leftover no-op reclaim timers; REPACJ in library names; encode policy that does not match docs.

The app encodes the same knowledge in types, idempotent transactions, cassette-tested clients, and docs generated from `PathSchema` so the markdown cannot lie.

The workspace `/home/coto/work/dev/media-sync` is empty of the live app. This crate workspace is the **repo of record**. The live tree under `~/videos` becomes a deploy target.

### What the app is

A **reconciler of desired state across two machines**, not a seedbox installer with a sync script taped on. Two stores (`RemoteBuffer`, `LocalArchive`) plus an optional `Grabber`. The library of record can run with `grabber=None`: sync a folder, name it, encode it. *arr is optional.

Every tribal script becomes a transaction type in a **Plan**. `plan` is the default verb. `run` is plan plus apply.

---

## Product

### Shape

- **CLI-first.** The CLI is the API. Every capability is a clap subcommand. `--json` on every command so other tools can compose. Completions are a skin.
- **TUI optional.** Panes for disk, queue, holds, last plan, agent output. Cron and systemd must call the CLI. The TUI must not block the timer. Progress is structured log plus optional TUI attach.
- **One binary** wrapping existing runtime tools, not vendoring them. External deps with probes: `ssh`, `rclone`, `rsync`, `ffmpeg` / `jellyfin-ffmpeg`, `ffprobe`, systemd user, and the CLI LLM binaries (`claude`, `codex`, `grok`, …). The app is an orchestrator.
- **Cloneable ops, not a SaaS.** Same binary, different desired-state file. “Set up a friend” is a file, not an account.

### Desired-state document

The only user-facing config. Lives with the **home repo**; the box is disposable. Every flag that today lives in `apply-stack.py` becomes a typed field. Config is **snapshotted at plan start**; mid-job edits wait for the next job. No hot-reload during a copy.

A wizard is allowed only as a UX over reconcile: every wizard step is an apply of desired state, so running it twice is a no-op when Edge, Grab, and Paths already match.

Apply and repair print **unified diffs** of ini, xml, and nginx **before** writing.

### Identity

Title identity is **TMDB / TVDB / MBID plus kind**, not a path string. Folder and file names are a rendered view of identity.

Local sqlite maps `TitleId` → path → inode/hash so identity survives renames.

Round-trip law, enforced in CI: `parse(render(id)) == id`. Year lives in the folder and the file the same way. Music remasters are a documented identity rule, not a year compare: Relayer MBID stays Relayer; `Relayer.(2013)` is not a missing 1974 album.

Scene tags are stripped by schema (tests from real library sins: `REPACJ`, `REPACK`, `PROPER`). `mediaops library lint` finds names that violate schema, including leftover scene tags and `Season 1` folders. **Install is the quality gate**, not a later mop. Spaces in names are refused; the schema parser is a gate.

### Connection, overlay, and formal API

SSH `{host, port, user, auth}` (from `~/.ssh/config` `Host seedbox`) is **bootstrap**: install `mediaopsd`, systemd user unit, enroll the overlay. SeedIt4Me port **2097** is an instance, not the API.

**Formal API:** `mediaopsd` **gRPC with mTLS**. Self-signed CA, server cert, and client cert are **generated at bootstrap** and kept in the home keyring / desired-state (never git). A bearer token may exist as a second factor; **mTLS is the preference**. Home `mediaops` CLI talks to a **local** mediaopsd (unix socket). That process is a gRPC client to seedbox mediaopsd. Seedbox mediaopsd is the **only** process that opens HTTP to Sonarr/Radarr/Lidarr/Prowlarr/SAB/qBit on `127.0.0.1`.

**Bind, do not install a tunnel app.** “Rathole” was the *idea* (a managed listen + auth + reconnect). **Do not install rathole, frp, or cloudflared.** mediaopsd **binds the gRPC server**.

1. **Default — mediaopsd listens.** Seedbox (already reachable) binds gRPC/mTLS. Home connects with the client cert. Optional **reverse-connect** mode in the same binary if the bind side is NATed (seedbox dials home, still gRPC/mTLS — not a third-party client).
2. **Optional, recommended — Tailscale or WireGuard** as an *underlay* (mesh, no published port). Not required. Userspace Tailscale if no TUN. Headscale OK.
3. **Not** Cloudflare Tunnel for bulk.
4. **Not** `ssh -L` / `ssh -R` as the product. SSH is bootstrap: install the unit, generate certs, place them, start mediaopsd.

**Human WebUI** (`mediaops ui sonarr`) is overlay-published localhost, session-scoped, not how apply works.

Root on the seedbox is only for nginx and package install. mediaopsd is user-level.

### Thick libraries, thin CLI (engineering intent)

Implementation is a **detailed modular system with abstractions and full API clients**, not thin scripts that hit five endpoints and hope.

- **mediaopsd** on the seedbox: **gRPC + mTLS** (certs minted at bootstrap). This is the formal API. Complete **Servarr HTTP clients** run *inside* the daemon against localhost — the home CLI never speaks Sonarr HTTP.
- **Parallel Range RPCs** on that same gRPC for bulk bytes (the rclone lesson, implemented by us). Probe concurrency, resume via `.partial`, progress.
- **Overlay is in-process.** SSH exists only to install the daemon and place certs. Tailscale / WG optional underlay.
- **Desired-state apply as idempotent transactions with diffs**, executed by seedbox mediaopsd. Running apply twice is a no-op if Edge, Grab, and Paths already match.

This is product/engineering intent for spec and architecture, not a fake OpenAPI dump. Architecture after spec owns the exact crate graph and endpoint lists. The crate cut below is **intent**, not frozen architecture.

### Crate graph (intent)

| Crate | Intent |
| --- | --- |
| `mediaops-core` | Desired-state types, TitleId, PathSchema, Plan/action types, ResourceBudget, identity sqlite, diffs, RPC schema |
| `mediaops-net` | gRPC listen / reverse-connect, mTLS identity (CA+certs), reconnect, optional Tailscale/WG underlay |
| `mediaops-ssh` | Bootstrap only: copy binary, systemd user unit, mint and place certs. Not the API |
| `mediaops-arr` | Full Servarr + SABnzbd + qBittorrent clients **used by mediaopsd on the box** |
| `mediaops-daemon` | `mediaopsd`: gRPC server, mTLS, localhost *arr, Range RPCs for allowlisted paths |
| `mediaops-transfer` | Home-side parallel Range RPCs; probe concurrency; `.partial` |
| `mediaops-sync` | Planner: what to Copy/Skip/Review; holds; reclaim; never torrent roots |
| `mediaops-encode` | EncodePolicy, playback matrix, NVENC probe/semaphore, reversible encode transaction |
| `mediaops-agent` | AgentTask, dossier, capability tokens, subprocess CLI LLMs, propose-only writes |
| `mediaops-cli` | clap subcommands, `--json`, optional TUI, talks to **local** mediaopsd |

Tests use **cassette fixtures** of *arr JSON and directory trees. Unit tests never require the live box.

---

## Capabilities-as-jobs

These are the jobs the operator is hiring the product to do. Spec should treat them as capability slices, not as a backlog of cute names.

### Watch (tonight-playable)

`watch TITLE` ensures the title is monitored, grabbed, pulled, and encoded if the playback matrix requires it. The outcome is a file on the home disk that Chrome/TV can play, under the schema path. The app does **not** configure a media server or print a dependency on Jellyfin/Plex setup. Playback is a property of the archive (codec, depth, container, HDR).

`--max-gb` with a live remaining-home-disk figure so tonight does not fill the spinning disk.

Upgrade of an already-local title is **not** automatic: upgrade policy is `never` | `if-better-profile` | `if-user-marked-UHD`. Default **never**, matching current skip. HD vs Ultra-HD is an explicit per-title profile class. Auto-upgrading 1080p to 4k remux “because disk is bored” is forbidden.

### Why (trace a title)

`why TITLE` traces grab → import → hold → pull → encode → library. Local filesystem is source of truth; if *arr still thinks the file is missing, the app tells *arr to unmonitor. Trusting *arr “file exists” as local exists is a failure mode.

`seedbox df` plus a reclaim **preview** ranked by ratio, private, age answers “why is disk full.” Before any remote delete, show **local hash proof**. Reclaim never touches private-under-goal (seed-goal dashboard / don’t-get-banned).

### Doctor / repair

`seedbox doctor` is the command you remember when the panel ate nginx.

- Panel is an **untrusted writer**. Detect panel fingerprint (Host rewrite, 302 to localhost after a panel install). Hash nginx app confs for drift.
- Reconcile **fails** if `EdgeInvariant` drifts (see Constraints).
- `repair edge` is **one transaction**: dry-run a diff, then apply fix-nginx plus apply-stack as a single unit.
- After any box install or upgrade, automatically queue an edge check before declaring success.
- Freeze apply on panel fight unless `--repair-edge`, and alert.
- **Scheduled doctor is read-only.** Write repairs need a local confirm flag or a pin. `doctor --repair` must not run unattended from a public laptop.
- Doctor also checks: Prowlarr app URLs include `url_base` (`/prowlarr/{id}/` not `/{id}/`); qBit DHT/PeX/LSD against privacy/tracker policy; SAB categories `tv` / `movies` / `music` asserted on SAB and on each *arr client; if a media server is already present, **warn** (do not reconfigure) when its libraries include `_incoming` or `_ops`.

### Bootstrap both sides (first-class)

Two first-class commands, not a wizard that only works once, not a media-server installer.

1. **Bootstrap a new seedbox** — connection, provider, packages, desired-state, paths, secret discovery, transfer-backend probe.
2. **Bootstrap a new local media directory** — schema dirs, watermarks, timer, lock, rclone remote, state/index.

See [Bootstrap](#bootstrap) for the full contract. `bootstrap --relocate` rewrites schema roots, systemd, and title-index paths when the library moves. Export desired-state plus title index; import bootstraps layout even before files exist (`new-machine`).

### Plan / run

`plan` is the default verb. `run` is plan plus apply. “Tell me what will happen” is the product, not an afterthought.

Plan is a first-class artifact with actions: `Copy`, `Skip`, `Review`, `Unmonitor`, `DeleteRemote`, `Encode`, `Reclaim`.

Jobs have **state machines**. Timers only start **make-progress-on-ready-jobs**. No overlapping hope-cron.

Config snapshot at plan start. Lock is machine-global (see Constraints). Lock conflict is a **distinct exit code** plus a skip-with-reason log line — never `exit 0` silent.

### Holds (typed inbox)

Holds are a typed queue with `Approve`, `Reject`, `Research` — not a folder a media server might ingest.

- `importBlocked` forever is a product feature: show the *arr message, ffprobe, and approve/reject/research. Never leave blocked NZBs posing as library.
- **Reject** is a first-class event: never pull this release; let *arr try another.
- **Approve** (e.g. Hearts): I looked at it, put it in the library, trust me — promote from the hold queue.
- Auto-approve every hold is forbidden. Holds require a human **or** an agent recommendation with a **confidence floor**.
- CLI/TUI inbox with age, size, and *arr reason, or it becomes a junk drawer again (Homework, The Who, Yes).
- `needs-split` (season pack) is a workflow with an agent, not a pile of files.
- `_ops` and `_incoming` are app-managed state dirs with a schema, **still not libraries**.

### Research / debug via CLI LLM agents

Agents are **subprocess CLI LLMs**, not a brain in-process. Research and debug are subcommands that shell out to `claude`, `codex`, or `grok` (pick per task). The app only cares about the **schema out**.

- Research: scene name → TitleId without downloading; theatrical-cut dossier (year, edition, runtime vs ffprobe). Web-using agent is allowed for research.
- Debug media: ffprobe + mediainfo + a **playback client profile** (Chrome, TV, Shield) → Direct Play vs encode. Local-file agent. This is encode/playback policy, not a media-server plugin.
- Title **dossier** (ffprobe, nfo, *arr payload, neighbors), not the whole library tree. Max-bytes budget per task.
- Capability tokens; **propose-only for writes**. Schema parser is the only writer. See [Agents](#agents).

### Quiet box / forget the panel

Success metric: **days without SSHing to the Swizzin panel**. All *arr settings as `seedbox apply` from a file you can read in git. Run-while-asleep is a **systemd oneshot**; the TUI is for Saturday.

### Reconcile after truth drifted

Manual copy, rename, or encode on disk must be reconcilable: tell *arr I have it; unmonitor; update sqlite identity.

### Encode queue

Visible, pausable job, not a surprise after sync. Home Ada GPU is a capability with a probe-derived NVENC cap. Seedbox CPU does not encode.

### Status without a daemon

`status --json` on localhost if needed, or `ssh mediaops status`. No extra daemon if possible.

---

## Bootstrap

Bootstrap is two first-class commands. Neither installs or configures Jellyfin/Plex.

### Bootstrap seedbox

Contract of `mediaops seedbox bootstrap` (name approximate; spec owns verbs):

**Connection.** Import `~/.ssh/config` `Host seedbox` for **bootstrap**. Install `mediaopsd`, **generate mTLS CA+certs**, start the gRPC listener. Optional Tailscale / WireGuard underlay. After that, health is gRPC, not SSH.

**Provider.** Trait: `SwizzinBox`, `AlreadyThere`, (later) `DockerCompose`. **v1 ships Swizzin plus AlreadyThere only.** This SeedIt4Me/Swizzin box is the v1 instance. Ultra.cc and other panels wait behind the trait; unimplemented providers may exist as tests, not as v1 scope. Abstracting Ultra.cc in v1 is a non-goal.

**Install / remove packages with version pins.** Version pins are first-class (Lidarr **2.14.5 / glibc** is the live lesson). Upgrade is a conscious transaction, never a panel click. A **compatibility matrix per provider OS**: `upgrade lidarr` explains glibc and can refuse. Freeze updates in config when the matrix says so. After install or upgrade, queue an edge check before success.

**EdgeInvariant** — reconcile fails if any of these drift:

- *arr bind `127.0.0.1` (bind address is part of the invariant; bind-to-star is a failure)
- `url_base` set
- nginx `Host $host` (not a rewrite to localhost)
- Forms auth

Panel fingerprint detection: if the panel rewrote nginx, freeze apply unless `--repair-edge`.

**GrabPolicy** is data, applied via *arr API, not clicks: delay, quality, indexer priority, client priority. Custom formats (`Prefer H.264`, HEVC last, 10-bit last, AV1 last) are **versioned policy packs** the app applies. Quality-profile custom-format scores drifting after a panel visit: treat CF packs as desired state and **re-PUT them on apply**. GrabPolicy changes are explicit commands with a diff; agents do not change quality profiles casually.

**PathSchema** is a versioned grammar that compiles to `PathBuf`s. Remote roots are an **allowlist**. Unknown paths are errors. Walk never follows symlinks off the allowlist. Never walk torrent save paths. `AGENTS.md` and human markdown (`mediaops docs render`) are **generated from the same structs as the walker**, so docs cannot lie.

**Indexer and download-client sets.** Upsert-by-name with a set diff. Adding a duplicate is a **conflict, not an append**. Apply **deletes extras**. NZBgeek twice is the named failure. Desired indexers are a set with priority.

**Secret discovery.** Discover API keys from remote `config.xml` and `sabnzbd.ini` at runtime. Never store the masked `********`. Zero secrets in git; keys are remote-discovered or in a keyring. Never display secrets; Test uses the discovered key; UI says **key present**. Presence-boolean, rotate command, never echo.

**Transfer probe.** After gRPC/mTLS is up, measure concurrent Range RPCs until throughput plateaus. Persist N. Live FTP-at-8-streams ~30 MiB/s was a **PASV-pool artifact**. Re-probe if bind address or underlay changes, not every run.

**Privacy / tracker invariants** for qBit (DHT/PeX/LSD) and SAB categories are asserted as part of apply, then checked by doctor.

### Bootstrap local media path

Contract of `mediaops local bootstrap <path>` (e.g. `I-just-bought-a-disk` → `/mnt/storage/videos`):

**Schema dirs:** `movies/`, `series/`, `music/` as the only library roots. `_incoming/` and `_ops/` as app-managed state, **not** libraries. Bootstrap must not add `_incoming` as a media library.

**Watermarks.** `ResourceBudget` lives in config: `max_copy_gib`, `min_free_gib`, `max_nvenc`, lock — no magic numbers in source. Refuse bootstrap (and preflight fail any copy) if the disk is below watermark. Live lesson: **256 GiB** watermark is a config value, not a comment.

**systemd user timer:** `OnUnitInactiveSec` is the **only v1 scheduler adapter**. Wrap systemd-user; do not invent cron math. Oneshot **plus** `OnUnitInactiveSec` **plus** flock — all three. `OnCalendar=hourly` overlapping runs is a failure mode.

**Lock.** Machine-global flock with pid, `started_at`, and command in the lockfile. `status` shows who holds it.

**gRPC Range client.** Home mediaopsd / transfer crate talks Range RPCs to seedbox mediaopsd with the bootstrap client cert. Persist probed concurrency. Not FTP, not SFTP, not rclone-as-pipe.

**State / index.** Create sqlite (`TitleId`, path, inode, hash), holds inbox, plan/job state, encode queue, `.partial` area. Generated docs from PathSchema land here so the next coding agent does not invent spaces in filenames.

**ffmpeg / NVENC probe.** Record GPU encode concurrency cap. This box’s live lesson: **cap 3**, never a hardcoded 8.

**Not in bootstrap:** Jellyfin/Plex libraries, users, playback-client server settings, or “add this folder to the media server.” Naming is schema-compatible with those servers; configuration of those servers is out of scope.

---

## Constraints

Laws, not preferences. Spec should copy these as MUST/MUST NOT.

### Sync laws

1. **One-way pull.** Home archive is the destination. Remote delete only for **surplus after local proof** (hash). Two-way sync that mirrors local deletes to the seedbox is forbidden. The job is the home archive, not a third cloud copy.
2. **Never walk torrent save paths.** Torrent files are not archive files. Allowlist of remote roots in schema; unknown paths are errors. Walk never follows symlinks off the allowlist. Copy from `torrents/incomplete` is a failure mode.
3. **Music-first, then videos under budget.** Job-priority law in the planner, not a hardcode in `pull()`. Generalize later to a **wants queue** the planner honors (“music this week, not more Futurama”). Many small FLACs vs one huge MKV also drives **per-item transfer shape** (see Transfer).
4. **Usenet delete after copy.** After Copy of a usenet complete: `DeleteRemote`. Usenet is deletable after copy.
5. **Torrents stay while seeding.** After Copy of a library hardlink: leave the torrent. Torrent delete belongs to **reclaim**, never to sync. Before any remote library unlink, **query qBit for that path**; if seeding, skip — a typed guard, not a comment.
6. **Holds as typed inbox.** Not a library root. Approve / Reject / Research only. See Holds.
7. **`.partial` is sacred.** Copy is always remote → staging `.partial` → verify → **atomic install** into a schema path. Crash recovery: `sync resume` lists `.partial` and continues. Empty-dir prune / GC **never** deletes partials. Resume is the job (`--partial` / rclone resume), not restart.
8. **Bulk bytes do not ride SSH.** Data plane is Range RPCs on mediaopsd gRPC/mTLS.
9. **Local FS is source of truth.** After copy, reconcile *arr (unmonitor / “I have it”). Do not trust *arr file-exists as local exists.
10. **Surplus is not skip.** Skip means do not copy. Surplus means remote **can go** after local proof.
11. **Decide-item upgrade default is never.** Matching current skip. See Watch.

**Copy path (always):** remote → staging `.partial` → verify → atomic install into schema path. Schema parser refuses illegal names at install.

**ReclaimPolicy** is constraints (private, seeding, imported) times an objective (free space percent), replacing leftover timers. Reclaim is either a real policy with a dry-run or it is **removed** — never a silent timer (`seedbox-reclaim.timer` leftover no-op is the named failure). Private-under-goal is untouchable.

### Transfer layer (picked)

**Control and data share one gRPC/mTLS identity.** Different RPCs, same listener.

| Plane | Mechanism |
| --- | --- |
| Control | mediaopsd gRPC (plan, apply, doctor, holds, …) |
| Data | `GetRange` / streaming RPCs for allowlisted local paths; home issues N in parallel |

**Why Range-shaped parallelism (research), implemented by us:**

- rclone showed ~4× rsync on large files via parallel streams (Geerling 2025). We take that lesson, not the rclone binary as the pipe.
- **FTP** on this box is a 10-slot PASV pool (~30 MiB/s). Rejected.
- **SFTP** is CPU-heavy and double-encrypts if an underlay is already encrypted. Rejected.
- **rsync** is one TCP stream; wrong default for 20–80 GiB over WAN.
- **rathole/frp apps** rejected — mediaopsd binds; certs from bootstrap.

Planner: many FLACs → many files in flight; huge MKV → many ranges on one file. Probe N; persist. Do not collapse all ranges onto one TCP connection.

Emergency rsync-ssh may exist unadvertised. Not FTP. Not a third-party tunnel app.

### Grabber stack (picked)

Current seedbox tools are a starting inventory, not a sacred set. Picks after looking at 2025–2026 alternatives:

| Role | Pick | Why |
| --- | --- | --- |
| TV | **Sonarr** | No replacement for the job. |
| Movies | **Radarr** | Same. |
| Music | **Lidarr**, version-pinned | Beets is a tagger, not a grabber. glibc pin stays a provider matrix row. |
| Indexers | **Prowlarr** | Jackett is the legacy proxy; NZBHydra2 is redundant with one Usenet indexer. |
| Usenet | **SABnzbd** | nzbgetcom is maintained, but SAB has the *arr integration they already run and this box is not RAM-poor. Switching is churn, not a throughput win. |
| Torrents | **qBittorrent** | Best WebAPI for categories, `pausedUP`/`stoppedUP`, seed-limit, path query before unlink. rtorrent/ruTorrent is seedbox-traditional and a worse API. |
| Subtitles | **Bazarr** optional | Files on disk; not a media-server plugin. |
| IRC snatch | **Autobrr** optional, not core | RSS via Prowlarr is enough. |
| TRaSH profiles | **GrabPolicy in our client** | Recyclarr/configarr are the idea (profiles as data). Do not require their binaries; PUT via mediaopsd. |

*arr HTTP never leaves localhost. mediaopsd is the remote surface.

### Encode and playback policy

In service of a **playable archive**, not a media-server plugin.

`EncodePolicy(codec, depth, container, hdr)` yields `Keep`, `NvencH264`, or `Refuse`.

**Playback matrix** in config: client profiles (Chrome, TV, Shield) × codec → keep / encode / refuse.

Load-bearing laws from the live box:

- **HEVC-MP4 Chrome dropped frames** → encode scan is `movies/**/*.mp4` HEVC10. **Series-skip is an explicit rule**, not an accident.
- **HDR / Dolby Vision and 2160p remux are Keep-forever.** Refuse-to-encode is a law. Transcoding Avatar DV to H.264 is forbidden.
- **H.264 8-bit** is what Chrome/TV need when encode *is* chosen. Policy packs **drive the encoder** or docs will keep lying (`encode_h264.py` not converting H.264 10-bit while `AGENTS.md` said to).
- **NVENC concurrency from GPU probe**, never a hardcoded 8. This box caps at **3**.
- Encode is a **home-GPU** capability. Seedbox is dumb disk plus pipe.
- Encode is a **reversible transaction**: write `.converting`, replace, move original to `backup-hevc-originals`; never delete until replace succeeds.

### Agents

`AgentTask = {prompt_template, inputs, output_schema, timeout, cwd sandbox, binary}`.

- LLM is a **subprocess**, not in-process.
- Default probes: read-only (`ffprobe`, `ls`, *arr GET).
- **Capability tokens:** `ReadFs`, `ProbeMedia`, `ArrGet`, `ArrPost`, `SshExecAllowlist`. No root SSH for agents. Vague prompts must not result in *arr POST; dry-run default.
- **Propose-only for writes.** Agent may propose a rename; **schema parser is the only writer**. Agents inventing spaces in filenames is a named failure.
- **Title dossier**, not the universe. Max-bytes budget per task.
- Pick an LLM per task (research vs debug-media). App validates schema out.
- GrabPolicy / quality-profile writes are explicit human commands with diffs, not casual agent side effects.

### Resource, lock, crash safety

- `ResourceBudget` in config: `max_copy_gib`, `min_free_gib`, `max_nvenc`, lock.
- Preflight fail if a copy would breach watermark.
- Machine-global flock; lockfile contains pid, `started_at`, command; distinct exit code on conflict.
- `*.partial` sacred; resume lists and continues; GC never deletes them.
- Jobs are state machines; timer only advances ready jobs.

### Secrets, apply, edge

- Zero secrets in git. Discover from remote config.xml / sabnzbd.ini or keyring.
- Never echo keys; presence-boolean; rotate command.
- Idempotent apply; diffs before write.
- EdgeRepair is one transaction.
- Panel fingerprint → freeze unless `--repair-edge`.
- Scheduled doctor read-only; write repairs need confirm flag or pin.

### PathSchema as single writer

Generated docs, install gate, agent proposals, and the naming contract all compile from one grammar. Composable parsers (movie, episode, track) with explicit reject bins (`needs-split`, `needs-year`). Not one giant filename regex.

### Provider and v1 instance

v1 provider is **this SeedIt4Me/Swizzin box plus this home disk**. Multi-panel later behind a `Provider` trait. `AlreadyThere` is the other v1 provider (box already has the stack; app only reconciles). Do not abstract Ultra.cc in v1.

### Runtime deps

Declare and probe: **mediaopsd**, ssh (bootstrap), ffmpeg/jellyfin-ffmpeg, ffprobe, systemd-user, CLI LLM binaries. Optional: tailscale or wireguard. Do not vendor them. rathole, rclone, and rsync are not required runtime deps for the overlay pull.

---

## v1 scope

In:

- This SeedIt4Me/Swizzin box + this home disk.
- CLI-first `mediaops-*` workspace plus **mediaopsd on the seedbox**.
- Desired-state file in the home repo.
- Doctor / repair edge via RPC.
- Sync planner preserving the sync laws above, with **gRPC Range RPCs** as the data plane.
- Schema-generated docs.
- Agent subprocess with dossier + capability tokens.
- `ui` via overlay-published localhost, not SSH `-L`.
- Bootstrap seedbox + bootstrap local media path as first-class.
- Full Servarr / rclone / SSH client libraries (thick), thin CLI.

Out of v1 (explicit):

- Ultra.cc / QuickBox / other panels as working providers (trait + tests only).
- Custom TCP file protocol / librsync as default. FTP, SFTP, or rathole **app** as the data plane.
- SaaS, accounts, extra daemon if CLI/SSH status suffices.
- Jellyfin/Plex configuration.
- Two-way sync.
- In-process LLM.
- Vendoring ffmpeg/rclone/ssh.

---

## Success

Signals the spec can test against, not slogans.

1. **Days without the panel.** Operator does not SSH to Swizzin to click nginx, indexers, or Lidarr Update. Desired-state apply from git-readable files is how the box stays correct.
2. **A new disk and a new box can be bootstrapped from the desired-state file.** `local bootstrap` on empty disk creates schema, watermarks, timer, lock, rclone remote, state. `seedbox bootstrap` on a fresh Swizzin (or `AlreadyThere`) brings connection, pins, EdgeInvariant, GrabPolicy, PathSchema, indexer/client sets, secrets, transfer probe. Layout can import before files exist.
3. **A title can be traced with `why`.** Grab → import → hold → pull → encode → library, with local FS as truth.
4. **Bandwidth is actually used.** Parallel Range RPCs on gRPC/mTLS saturate the pipe; concurrency probed and persisted. Success is beating the old FTP-PASV ~30 MiB/s ceiling, not matching it.
5. **Resume-not-restart.** Kill a copy at 90%; `sync resume` continues from `.partial`. GC has not deleted it.
6. **Quiet unattended.** systemd oneshot + `OnUnitInactiveSec` + flock; lock conflict is visible; scheduled doctor does not write.
7. **Holds do not rot.** Inbox with age/size/reason; reject/approve are events; blocked NZBs are not library.
8. **Playable archive.** HEVC-MP4 movies that break Chrome get NVENC under the probe cap; HDR/DV remuxes are kept; series-skip is explicit.
9. **Apply is boring.** Second apply is a no-op (Edge, Grab, Paths match). Diffs shown when something would change.
10. **One binary on PATH.** `cargo install` / path; no Python venv to remember.

---

## Non-goals

- **Configuring Jellyfin or Plex** — libraries, users, playback clients, plugins, or adding folders to a media server. Naming is schema-compatible; the app does not touch those servers’ config. Adding `_incoming` as a media library is forbidden even as a “helpful” default.
- **SaaS** — no hosted accounts, no multi-tenant cloud product. Cloneable ops via a desired-state file.
- **Two-way sync** — no mirroring local deletes to the seedbox; no “third copy” to another cloud as the product job.
- **Installing rathole, frp, or cloudflared.** The overlay is mediaopsd itself (gRPC/mTLS).
- **SSH + localhost HTTP as the API** — rejected. SSH is bootstrap.
- **FTP, rsync, or rclone-as-the-pipe** as the default pull — researched and rejected; we implement parallel Range on gRPC.
- **Abstracting Ultra.cc in v1** — Provider trait may exist; v1 implements Swizzin + AlreadyThere for this box.
- **Giving agents root SSH** — no `SshExec` to root; allowlisted exec at most; propose-only writes.
- **Media-server installer / plugin** — encode/playback matrix is archive policy, not a Jellyfin/Plex plugin.
- **Public WebUI or LAN bind of *arr** — no forwarding *arr to `0.0.0.0` on the home LAN; no assuming a public seedbox WebUI.
- **In-process LLM / dumping the whole library into context.**
- **Panel-click upgrades and unpinned Lidarr updates.**
- **OnCalendar overlapping cron** as the scheduler.
- **Hot-reload of config mid-copy.**
- **Identity = path string.**
- **TUI as the only UI.**
- **Auto-approve holds; auto-upgrade HD → UHD.**

---

## Load-bearing claims (for spec)

Spec should treat these as already decided. Architecture may refine HOW, not whether.

1. Reconciler of desired state across two machines; desired-state lives at home; box is disposable.
2. Local filesystem is library of record; *arr is an optional grabber you tell the truth to; identity is TitleId (TMDB/TVDB/MBID + kind).
3. CLI is the API; TUI optional; systemd oneshot is how it runs unattended.
4. Bootstrap seedbox and bootstrap local media path are first-class; Jellyfin/Plex configuration is out of scope.
5. v1 = this SeedIt4Me/Swizzin box + this home disk; Provider trait for later; AlreadyThere in v1.
6. EdgeInvariant = bind 127.0.0.1 + url_base + Host `$host` + Forms auth; panel is untrusted writer; repair is one transaction; scheduled doctor is read-only.
7. Version pins + OS compatibility matrix (Lidarr/glibc); upgrades are transactions.
8. Indexers/clients are sets (upsert-by-name, delete extras); secrets discovered from config.xml/ini, never `********`.
9. PathSchema is the single writer; generated docs; install gate; round-trip `parse(render(id)) == id`.
10. Sync laws: one-way pull; never torrent roots; music-first then videos under budget; usenet delete after copy; torrents stay while seeding (qBit guard); holds as typed inbox; `.partial` sacred; bulk bytes do not ride SSH.
11. Planes: control = mediaopsd **gRPC + mTLS** (certs at bootstrap). Data = parallel Range RPCs on the same listener. Optional Tailscale/WG underlay. Not rathole-the-app, not FTP, not SSH `-L`.
12. Copy = remote → `.partial` → verify → atomic schema install.
13. EncodePolicy + playback matrix; HEVC-MP4 Chrome encode for movies; series-skip explicit; HDR/DV keep; NVENC cap from probe; reversible `.converting` transaction; encode on home GPU only.
14. Agents: subprocess CLI LLMs, title dossier, capability tokens, propose-only writes, schema parser is the only writer.
15. Thick libraries: mediaopsd gRPC/mTLS + complete Servarr clients **on the box**, Range RPC client on home, SSH bootstrap (certs). Crate graph includes `mediaops-daemon`, `mediaops-net`, `mediaops-transfer`.
16. ResourceBudget in config; flock + OnUnitInactiveSec + oneshot; lock conflict is a distinct exit code.
17. Reclaim is a real policy with dry-run or it does not exist; private-under-goal untouchable; local hash proof before remote delete.
18. Tests: cassettes of *arr JSON and directory trees; live box not required for unit tests. Failure history is the test suite.
19. Workspace is repo of record; live `~/videos` tree is a deploy target.
20. Next session is spec. This document is the source of jobs, laws, non-goals, and success signals.
