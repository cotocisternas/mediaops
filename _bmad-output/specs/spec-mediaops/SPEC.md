---
id: SPEC-mediaops
companions:
  - module-map.md
  - grabber-inventory.md
  - bootstrap-surfaces.md
  - failure-history-tests.md
  - increments.md
  - ../../planning-artifacts/architecture/architecture-mediaops-2026-08-29/ARCHITECTURE-SPINE.md
sources:
  - _bmad-output/brainstorming/brainstorm-rust-seedbox-media-app-2026-08-28/brainstorm-intent.md
  - _bmad-output/brainstorming/brainstorm-rust-seedbox-media-app-2026-08-28/product-idea.md
  - _bmad-output/brainstorming/brainstorm-rust-seedbox-media-app-2026-08-28/modules.md
---

> **Canonical contract.** This SPEC and the files in `companions:` are the complete, preservation-validated contract for what to build, test, and validate. Source documents listed in frontmatter are for traceability — consult them only if you need narrative rationale or prose color this contract intentionally omits.

# mediaops

## Why

Pain plus vision, one operator, two machines. Tribal Python under `~/videos` (`apply-stack.py`, media-sync, `encode_h264.py`, leftover reclaim timers, `SEEDBOX.md` that can lie) encodes the laws in comments; failure history is already the test suite. The seedbox is a disposable rented buffer. The home filesystem is the library of record. *arr is an optional grabber you tell the truth to — not the catalog. mediaops exists so tonight-playable media, a quiet box (days without the panel), resume-not-restart, a why-trace, and a real holds inbox live in types, idempotent transactions, and generated docs.

## Capabilities

- **CAP-1 Tonight-playable**
  - **intent:** Operator can mark a title as wanted (and monitored if a grabber is on) so the unattended timer pulls and encodes it to a schema-valid playable file on the home disk, without occupying a console and without filling that disk past budget.
  - **success:** `watch TITLE` exits 0 when the title is in the want set and, if grabber is on, *arr is monitoring — not when the file is already playable. A later timer/`run` leaves a playable schema file or an open hold. `why` / `status` show hold, watermark, lock, pull, and encode. Remaining-home-disk and max-copy budget are honored. Upgrade default never.

- **CAP-2 Quiet box**
  - **intent:** Operator can keep grabber, edge, and path desired-state correct from a git-readable file without SSHing the Swizzin panel.
  - **success:** Second apply is a no-op when Edge, Grab, and Paths already match; unified diffs of ini, xml, and nginx appear before any write.

- **CAP-3 Why-trace**
  - **intent:** Operator can peek, from time to time, why a title is or is not on the home disk, along grab → import → hold → pull → encode → library, with local FS as truth.
  - **success:** A why-trace of a title shows that chain including stuck states (hold, watermark, lock, encode queue). If *arr still thinks the file is missing while it exists locally, the system tells *arr to unmonitor. Disk-full is answered by seedbox df plus a reclaim preview ranked by ratio, private, and age, with local BLAKE3 proof before any remote delete.

- **CAP-4 Resume-not-restart**
  - **intent:** Operator can continue a dead transfer from staging rather than restarting.
  - **success:** Kill a copy at ~90%; resume lists the `.partial` and continues; empty-dir prune and GC have not deleted it.

- **CAP-5 Bootstrap seedbox**
  - **intent:** Operator can bring a new SeedIt4Me/Swizzin box, or an AlreadyThere box, to desired-state from the home repo — daemon, overlay identity, packages and pins, edge, grabber policy, and transfer probe. Command surface: [bootstrap-surfaces.md](bootstrap-surfaces.md).
  - **success:** After bootstrap, mediaopsd answers gRPC under mTLS; EdgeInvariant holds; indexer and client sets match desired-state; API keys are discovered not pasted; Range-RPC concurrency N is probed and persisted; Jellyfin/Plex is untouched.

- **CAP-6 Bootstrap local library**
  - **intent:** Operator can stand up a new home media directory from desired-state so the library of record is ready to plan, lock, time, and encode — not a media-server installer. Command surface: [bootstrap-surfaces.md](bootstrap-surfaces.md).
  - **success:** Schema dirs `movies` / `series` / `music` exist; `_ops` and `_incoming` exist as app-managed non-libraries; watermarks, lock, systemd-user oneshot + `OnUnitInactiveSec` + flock, title-index sqlite, and NVENC cap are in place; bootstrap refuses if the disk is below watermark; no media-server libraries, users, or clients are configured.

- **CAP-7 Plan and run**
  - **intent:** Operator can see what the reconciler will do before it does it, then apply that plan as transactions.
  - **success:** Plan is a first-class artifact with at least Copy, Skip, Review, Unmonitor, DeleteRemote, Encode, Reclaim, plus edge/grab apply as reconcile steps; run = plan + apply; config is snapshotted at plan start; lock conflict is a distinct exit code plus skip-with-reason, never silent 0.

- **CAP-8 Holds inbox**
  - **intent:** Operator can treat import-blocked and similar as an inbox with Approve, Reject, and Research — not a folder a media server might ingest.
  - **success:** Inbox shows age, size, and *arr reason; Reject means never this release and lets *arr try another; Approve promotes to a schema path; auto-approve is impossible; there is no agent-approve path (so no confidence floor) in v1; blocked NZBs are not library. Research may later call CAP-11; first demo has no LLM.

- **CAP-9 Reclaim**
  - **intent:** Operator can free remote buffer after local proof, under a real policy, without deleting seeding or private-under-goal torrents.
  - **success:** Preview is ranked; dry-run exists or reclaim does not exist; qBit is queried before any remote library unlink and seeding skips; private-under-goal is untouched; usenet-complete is deletable after Copy; torrent delete is reclaim-only.

- **CAP-10 Home encode**
  - **intent:** Operator can transcode home-disk files so Chrome/TV can play, without touching seedbox CPU or destroying originals, and without encoding Keep-forever titles.
  - **success:** HEVC-MP4 movies that break Chrome encode under the probed NVENC cap; series-skip of HEVC-MP4 is an explicit named rule; HDR/DV and 2160p remux refuse; encode writes `.converting`, replaces, then moves the original to backup; original is never deleted before replace succeeds; queue is visible and pausable.

- **CAP-11 Agent research and debug**
  - **intent:** Operator can research a scene name to TitleId and debug playability via a sandboxed LLM subprocess without opening twelve tabs or granting write by default.
  - **success:** Task is `{prompt_template, inputs, output_schema, timeout, cwd sandbox, binary}`; output is schema-validated; writes are propose-only unless Apply is granted; PathSchema is the only writer; dossier is per-title with a max-bytes budget; no root SSH.

- **CAP-12 Doctor and edge repair**
  - **intent:** Operator can detect when the panel rewrote the edge, and repair it as one confirmed transaction.
  - **success:** Scheduled doctor is read-only; write repair requires a local confirm flag or pin; EdgeInvariant drift fails reconcile; repair is one transaction (diff, then nginx + stack apply); after install/upgrade an edge check is queued before success; panel fingerprint freezes apply unless repair-edge is explicit.

## Constraints

- Formal API is mediaopsd gRPC with mTLS. Self-signed CA, server cert, and client cert are generated at bootstrap and never committed. v1 is mTLS only; a bearer token as second factor is deferred. Home CLI talks only to local mediaopsd (unix socket) and never contains a seedbox address. Seedbox mediaopsd is the only process that opens HTTP to the grabber stack on `127.0.0.1`. Cert PEMs live as gitignored files next to desired-state; desired-state stores fingerprints and paths, never PEMs. Doctor refuses if cert PEMs are inside a git work tree.
- Overlay is in-process: for this SeedIt4Me box, mediaopsd binds. Reverse-connect (same binary, if the bind side is NATed) and Tailscale/WireGuard underlay are designed and unused by default — not required to demo the first Range RPC. Not rathole, frp, or cloudflared apps. Not Cloudflare Tunnel for bulk. Not `ssh -L` / `ssh -R` as the product.
- SSH is bootstrap only: copy binary, systemd user unit, mint/place certs, Swizzin root for nginx and packages. After enroll, doctor/apply/copy do not use SSH. Never bulk copy over SSH. There is no emergency rsync-ssh path.
- Data plane is parallel Range RPCs on that same gRPC/mTLS listener, allowlisted library/usenet-complete paths only. Not FTP, rsync, SFTP, or rclone-as-the-pipe. Probe N until throughput plateaus; persist N; re-probe on bind/underlay change. Many files in flight versus many ranges on one file; do not collapse onto one TCP. Verify is BLAKE3 per-range (recorded in the `.partial` map) plus whole-file BLAKE3 at schema install. Reclaim local-proof uses only the install digest. Size/mtime is not proof.
- One-way pull. Remote delete only for surplus after local hash proof. Never two-way; never a third cloud; Push is not a product surface. Skip ≠ surplus: skip means do not copy; surplus means remote may go after local proof.
- Never torrent save paths, never `torrents/incomplete`. Remote roots are a PathSchema allowlist; unknown paths error; walks never follow symlinks off the allowlist.
- Copy is always remote → staging `.partial` → per-range BLAKE3 verify → whole-file BLAKE3 → atomic install into a schema path. `.partial` is sacred; resume lists completed ranges and continues; GC never deletes partials. Title-index hash is the install BLAKE3.
- Usenet: DeleteRemote after Copy. Torrents: leave while seeding. Torrent delete belongs to reclaim, never to sync. Before any remote library unlink, query qBit; if seeding, skip.
- Local filesystem is library of record. *arr is an optional grabber (`grabber=None` is valid). Never treat *arr file-exists as local-exists.
- Identity is TitleId `kind:source:id`, never a raw path string. Sources: `key` (the identity the library itself carries — normalised title + year for movies and shows, artist + album for music, derived from the dotted `Title.(Year)` folders *arr writes and the operator keeps) and the *arr authorities TMDB / TVDB / MBID (holds, wants, unmonitor; the daemon bridges them to keys). Identity is per **file**: an episode is `(show, season, episode)`, a track is `(album, disc, track)`; a movie is one file. Local sqlite maps each installed path → TitleId + BLAKE3. Music remasters: artist + album, not folder year (Relayer 1974 vs Relayer.(2013)).
- PathSchema (grammar v2) is the single writer for names, install gate, lint, agent proposals, and generated docs. No id token lives in a path: `movies/T.(Y)/T.(Y).ext`, `series/T.(Y)/Season.NN/T.(Y).SnnEnn[-Enn][.Episode.Title].ext`, `music/Artist/Album.(Y)/[Disc.NN/]Album.(Y).NN.Title.ext`. `parse(render(key_id, p)) == (key_id, p)`; parse is lenient about spaces and `Title - Subtitle (Year)`, render is strict dots. Scene-tag strip includes REPACJ, REPACK, PROPER. Spaces refused on render. Explicit reject bins include needs-split and needs-year; media the grammar cannot place is a visible `Review` action, never a silent drop.
- Holds are an inbox, not a library. `_ops` and `_incoming` are app-managed, never libraries.
- EdgeInvariant: bind `127.0.0.1` + `url_base` + Host `$host` + Forms auth. Panel is an untrusted writer; hash nginx app confs. Prowlarr app URLs must include `url_base`. qBit DHT/PeX/LSD off is a doctor invariant. SAB categories `tv` / `movies` / `music` asserted on SAB and each *arr client. If a media server is already present, warn (do not reconfigure) when its libraries include `_incoming` or `_ops`. Root on the seedbox only for nginx and package install; mediaopsd is user-level.
- Encode on home GPU only. Seedbox is dumb disk + pipe. EncodePolicy(codec, depth, container, hdr) → Keep | NvencH264 | Refuse. When encode is chosen, the target is H.264 8-bit. Playback matrix is config (v1 client profiles: Chrome, TV, Shield × codec), not a media-server API. HDR/DV and 2160p remux are Keep-forever. NVENC concurrency from GPU probe; this box caps at 3; budget `max_nvenc` is the ceiling. Upgrade class is never | if-better-profile | if-user-marked-UHD; default never.
- ResourceBudget lives in config: `max_copy_gib`, `min_free_gib`, `max_nvenc`, lock — no magic numbers. Preflight fail if a copy would breach watermark. Live 256 GiB watermark is a config value.
- Scheduler is systemd-user oneshot + `OnUnitInactiveSec` + flock — all three. Lockfile has pid, started_at, command; `status` shows who holds it. Lock conflict is a distinct exit code, never silent 0. Jobs have state machines; timers only make-progress-on-ready-jobs. No `OnCalendar` overlapping hope-cron. Config is not hot-reloaded mid-copy.
- Indexers and download clients are sets keyed by name (+ priority). Duplicate add is a conflict, not an append. Apply is set-diff: PUT missing, delete extras. Custom-format packs are desired-state and re-PUT on apply. Version pins are first-class (Lidarr 2.14.5 / glibc matrix); upgrade is a conscious transaction that can refuse. GrabPolicy changes are explicit commands with a diff. Inventory: [grabber-inventory.md](grabber-inventory.md).
- v1 grabber stack: Sonarr, Radarr, Prowlarr, SABnzbd, qBittorrent, Lidarr version-pinned. Jackett out. Recyclarr binary not required (GrabPolicy data). Autobrr and Bazarr are out — not even stubs. *arr HTTP never leaves localhost.
- Zero secrets in git. Discover API keys from remote `config.xml` / `sabnzbd.ini` at runtime. Never store or echo masked `********`. UI is a presence boolean; Test uses the discovered key.
- Agents (CAP-11) are subprocess CLI LLMs (`claude` | `codex` | `grok`), not in-process, when shipped. Capability tokens: ReadFs, ProbeMedia, ArrGet, ArrPost, SshExecAllowlist. Default read-only. Never ssh root. Vague prompts must not *arr POST. First demo does not ship CAP-11 or any LLM runtime dep. No agent-approve path, so no confidence floor in v1.
- CLI is the API: every capability is a subcommand; `--json` on every command; TUI is an optional Saturday skin and is deferred from the first demo; timers must not require a TUI. Import `~/.ssh/config` Host `seedbox` — do not invent another alias format. Human WebUI (`ui <app>`) is overlay-published localhost, session-scoped, not how apply works, and is deferred from the first demo. `status --json` is localhost or `ssh mediaops status` — no extra public daemon. Command surface: [module-map.md](module-map.md). First demo vs later: [increments.md](increments.md).
- v1 is this SeedIt4Me/Swizzin box + this home disk. Provider trait may exist (`SwizzinBox`, `AlreadyThere`, later `DockerCompose` / other panels); v1 ships Swizzin + AlreadyThere only. Ultra.cc / QuickBox are unimplemented trait variants + tests, not ports. SeedIt4Me port 2097 is an instance, not architecture. Connection is `{host, port, user, auth}` plus gRPC bind and mTLS identity. First demo includes the pipe *and* encode: [increments.md](increments.md).
- Thick libraries, thin CLI. Complete grabber HTTP clients live only inside mediaopsd. Home CLI never speaks Sonarr HTTP. Architecture may shuffle impl crates; it must not put grabber HTTP in the CLI or FTP in transfer. Module responsibilities: [module-map.md](module-map.md). Binding crate-graph HOW (stable AD-N IDs): [ARCHITECTURE-SPINE.md](../../planning-artifacts/architecture/architecture-mediaops-2026-08-29/ARCHITECTURE-SPINE.md).
- Planner honors music-first then videos under ResourceBudget. `watch` enqueue is a per-title want, not a weekly playlist object. Generalized wants queue is deferred.
- This crate workspace is the repo of record. Live `~/videos` is a deploy target.
- Unit tests never require the live box. Failure history is the test suite: [failure-history-tests.md](failure-history-tests.md).
- Runtime deps are probed, not vendored: mediaopsd, ssh (bootstrap), ffmpeg/jellyfin-ffmpeg, ffprobe, systemd-user. CLI LLM binaries are not a first-demo dep. Optional: tailscale or wireguard. rathole, rclone, and rsync are not required and must not exist as a pull path.

## Non-goals

- Configuring Jellyfin or Plex (libraries, users, playback clients, plugins, or adding folders to a media server). Adding `_incoming` or `_ops` as a media-server library is forbidden even as a helpful default.
- Working Ultra.cc / QuickBox / other-panel providers in v1 (trait + tests only).
- Custom file-transfer protocol; FTP, rsync, SFTP, or rclone-as-the-pipe; emergency rsync-ssh; rathole/frp/cloudflared apps; Cloudflare Tunnel for bulk.
- Two-way sync; third-cloud archive; Push as a product surface.
- SSH + localhost HTTP as the API; `ssh -L` as the product.
- Public WebUI; LAN bind of *arr; extra public status daemon.
- SaaS / hosted accounts.
- In-process LLM; dumping the whole library into context; agents with root SSH.
- TUI as the only UI.
- Jackett; requiring the Recyclarr binary; Autobrr; Bazarr.
- Auto-approve holds; auto-upgrade HD → UHD.
- Encode on the seedbox.
- Panel-click upgrades and unpinned Lidarr updates.
- `OnCalendar` overlapping cron as the scheduler.
- Hot-reload of config mid-copy.
- Identity = path string.
- Media-server installer or plugin.
- Vendoring ffmpeg or ssh.

## Success signal

A new Swizzin box and a new home disk can be bootstrapped from the home desired-state file so that mediaopsd answers gRPC under bootstrap-minted mTLS (seedbox bind, home unix socket); `watch TITLE` enqueues without occupying a console; a later timer/`run` leaves a playable schema file (encode included) or an open hold; a copy killed at 90% resumes from `.partial` with per-range BLAKE3; a second apply is a no-op when Edge, Grab, and Paths match; parallel Range RPCs beat the live FTP-PASV ~30 MiB/s ceiling; `why` shows the chain including stuck states; scheduled doctor does not write; lock conflict is a distinct exit; HEVC-MP4 movies encode under the probe cap while HDR/DV remuxes are kept.

## Assumptions

- Instance numbers (NVENC cap 3, SeedIt4Me port 2097, 256 GiB watermark, Lidarr 2.14.5) are config/matrix rows, not architecture constants.
- The "no extra daemon" non-goal means no extra *public* status daemon — home mediaopsd on a unix socket is required.
- Unit tests for encode policy do not require a GPU; the first *box* demo does.
