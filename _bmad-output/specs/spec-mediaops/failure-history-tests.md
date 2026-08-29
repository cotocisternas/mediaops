# Failure-history tests

Unit tests never require the live box. Failure history is the test suite, not folklore. Live-box integration is optional behind an explicit feature/env; never default CI.

## Kinds

| Kind | What |
| --- | --- |
| Cassettes | Recorded *arr / SAB / qBit JSON fixtures. Client tests replay HTTP. |
| Tree fixtures | Directory layouts for parser, lint, planner (Copy/Skip/Hold/surplus), `.partial` resume, symlink-off-allowlist. |
| Schema CI | `parse(render(id)) == id`; scene-tag strip cases from real sins (REPACJ, REPACK, PROPER). |
| Transfer | Range RPC concurrency; probe persistence; `.partial` range map; per-range BLAKE3; whole-file BLAKE3 at install; no network. |
| Net | Overlay enroll dry-run; never publish *arr ports. |
| SSH | Bootstrap-only; bulk copy over SSH is a test failure. No rsync-ssh symbol in the tree. |
| Provider | DockerCompose / Ultra stubs: unimplemented + tests, not silent no-ops. |
| Integration (optional, not unit) | Live box behind an explicit feature/env; never default CI. |

## Named failures (must have tests)

| Failure | Law it encodes |
| --- | --- |
| Panel Host rewrite → 302 to localhost after a panel install | EdgeInvariant; panel is untrusted writer; freeze apply unless repair-edge |
| ControlMaster starvation of a 200 GiB copy | Bulk bytes do not ride SSH; SSH mux is bootstrap-only |
| HEVC-MP4 Chrome dropped frames | Encode scan `movies/**/*.mp4` HEVC10; series-skip is an explicit named rule |
| Holds rotting as a junk drawer | Typed inbox with age/size/reason; Approve/Reject/Research; blocked NZBs are not library |
| Lidarr glibc trap | Version pins + OS compatibility matrix; upgrade can refuse |
| Masked `********` keys pasted into Test | Discover keys from config.xml/ini; never store or echo secrets; UI is presence boolean |
| Duplicate NZBgeek append | Indexers are sets; duplicate add is a conflict, not an append |
| Docs vs code (`SEEDBOX.md` lying about scan paths; encode policy not matching docs) | PathSchema generates docs; EncodePolicy packs drive the encoder |
| Leftover no-op reclaim timer | Reclaim is a real policy with dry-run or it is removed |
| REPACJ / REPACK / PROPER in library names | Scene-tag strip is schema; install is the quality gate; `library lint` finds survivors |
| qBit seeding delete | Typed guard: query qBit before unlink; torrent delete is reclaim, never sync-after-copy |
| `_incoming` as a media-server library | `_ops` / `_incoming` are app-managed, never libraries; warn if an existing server already does this |
| Overlapping `OnCalendar` timers | oneshot + `OnUnitInactiveSec` + flock, all three |
| Watermark breach | ResourceBudget in config; preflight fail; bootstrap refuses below watermark |
| Lock conflict silent 0 | Distinct exit code + skip-with-reason |
| Spaces in names invented by agents | PathSchema is the only writer; agents propose |
| Bind-to-star / missing `url_base` / Prowlarr app URL `/{id}/` | EdgeInvariant; Prowlarr URLs must include `url_base` (`/prowlarr/{id}/`) |
| Walk of torrent save paths or `torrents/incomplete` | Allowlist only; unknown paths error; never follow symlinks off allowlist |
| Two-way sync mirroring local deletes | One-way pull; surplus after local proof only |
| `doctor --repair` unattended from a public laptop | Scheduled doctor is read-only; write needs confirm or pin |
| Auto-upgrade 1080p → 4k remux because disk is bored | Upgrade class default never |
| Transcoding HDR/DV or 2160p remux | Keep-forever; refuse-to-encode |
| Encode deleting original before replace | Reversible `.converting` transaction |
| Provider stub silently succeeding (DockerCompose / Ultra) | Unimplemented + tests, not silent no-ops |
| Collapsing all Range RPCs onto one TCP | Many files vs many ranges; do not idle the WAN |
| Size/mtime treated as proof | Verify is BLAKE3; reclaim uses only the install digest |
| Cert PEMs committed or sitting inside a git work tree | Gitignored tls dir; doctor refuses; desired-state has fingerprints only |
| CLI process contains a seedbox address | CLI talks only to home mediaopsd unix socket |
| Agent auto-approve / confidence floor | No agent-approve path in v1 |
| Emergency rsync-ssh “just in case” | Forbidden; hidden pipe will get used |
| Autobrr/Bazarr stub that silently no-ops | Out, not even stubs |
