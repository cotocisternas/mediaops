# Bootstrap surfaces

Two first-class commands. Not a media-server installer. Not a wizard that only works once: every wizard step, if any, is an apply of desired-state, so running it twice is a no-op when Edge, Grab, and Paths already match.

Verbs (modules last-wins): `mediaops seedbox bootstrap` and `mediaops library bootstrap`.

## Matrix

| Concern | `seedbox bootstrap` | `library bootstrap` |
| --- | --- | --- |
| Goal | New box: connection, provider, packages, desired-state, mint mTLS certs, bind gRPC | New home disk: schema, watermarks, timer, lock, client cert, state |
| core | Validate/write desired-state; remote PathSchema roots allowlist; version pins | Create schema dirs (`movies`/`series`/`music` plus app-managed `_ops` / `_incoming` — **not** libraries); watermarks; sqlite index; generate docs; refuse if disk below watermark |
| ssh | Bootstrap: install mediaopsd, mint/place mTLS certs, `SwizzinBox` packages/nginx | — |
| net | gRPC listen (this box binds). Reverse-connect and Tailscale/WG designed, unused by default. Mint mTLS certs to gitignored files next to desired-state | gRPC client via home mediaopsd unix socket; client cert from gitignored tls dir |
| daemon | mediaopsd running; gRPC + Range RPCs | Home mediaopsd unix socket |
| arr | Inside mediaopsd: discover keys, upsert indexers/clients, GrabPolicy, EdgeInvariant API | Optional: empty grabber is fine |
| transfer | Probe Range RPC concurrency over gRPC; persist N. Re-probe if bind address or underlay changes, not every run | staging dirs |
| encode | — | ffmpeg/NVENC probe; persist `max_nvenc` |
| sync | — | Ready to plan (no copy during bootstrap unless asked) |
| agent | — | — |
| cli | Lock, `--json`, diffs, confirm | systemd-user timer + flock; `library relocate` rewrites schema roots, systemd, title-index paths |
| Explicitly out | Jellyfin/Plex, Ultra.cc, public WebUI | Adding `_incoming` as a media-server library; wizard-once; rclone remote |

After seedbox bootstrap, health is gRPC, not SSH.

mTLS material: gitignored files next to desired-state. Desired-state stores fingerprints + paths, never PEMs. Doctor refuses if cert PEMs are inside a git work tree. `new-machine` copies the tls dir with desired-state + title index. Re-mint is the disaster path.

Provider: trait `SwizzinBox`, `AlreadyThere`, (later) `DockerCompose`. v1 ships Swizzin plus AlreadyThere only. This SeedIt4Me/Swizzin box is the v1 instance. Unimplemented providers may exist as tests, not as v1 scope.

Connection import: `~/.ssh/config` Host `seedbox` for bootstrap. SeedIt4Me port 2097 is an instance, not the API.

Transfer probe: after gRPC/mTLS is up, measure concurrent Range RPCs until throughput plateaus. Persist N. Live FTP-at-8-streams ~30 MiB/s was a PASV-pool artifact to beat, not match.

Privacy / tracker invariants for qBit (DHT/PeX/LSD) and SAB categories are asserted as part of apply, then checked by doctor.

## Relocate and new-machine

- `library relocate` rewrites schema roots, systemd, and title-index paths when the library moves.
- `new-machine`: export desired-state + title index + gitignored tls dir; import bootstraps layout even before files exist.

## Not in either bootstrap

Jellyfin/Plex libraries, users, playback-client server settings, or “add this folder to the media server.” Naming is schema-compatible with those servers; configuration of those servers is out of scope.
