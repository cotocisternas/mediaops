# First demo vs later

Party-mode last-wins (2026-08-28). Kernel capabilities may outlive the first demo; this file says what must actually run on this box first.

## First demo (must run on this box)

Bootstrap enough that seedbox mediaopsd binds gRPC/mTLS and home mediaopsd is on a unix socket. Then:

1. Plan.
2. Parallel Range pull on allowlisted paths.
3. `.partial` resume with per-range BLAKE3.
4. Whole-file BLAKE3, schema install.
5. Encode: at least one HEVC-MP4 movie under the probed NVENC cap so Chrome can play. HDR/DV remuxes stay Keep-forever.

`grabber=None` is a valid demo path (a folder on the box, a disk at home). Unit tests still never require the live box or a GPU. The GPU is this-box demo, not CI.

`watch` is not the demo harness. It enqueues a per-title want (and monitoring if grabber is on). Timer / `run` delivers the playable file or an open hold. Peek via `why` / `status`.

## Designed, unused by default

- Reverse-connect in the same binary.
- Tailscale / WireGuard underlay.

Not required to demo the first Range RPC on this reachable SeedIt4Me host.

## Deferred (keep the capability, do not build yet)

| Item | Notes |
| --- | --- |
| TUI | Saturday skin. systemd cannot use it. |
| `ui <app>` | Session-scoped localhost overlay. Not how apply works. |
| CAP-11 LLM agents | Holds inbox still ships. Research as an LLM verb waits. No CLI LLM runtime dep. |
| Bearer token 2FA | v1 is mTLS only. |
| Generalized wants queue | Music-first planner law + per-title `watch` want only. |

## Forbidden (do not implement)

- Emergency rsync-ssh, advertised or not. SSH bulk copy is a test failure.
- Autobrr / Bazarr, including stubs.
- Agent auto-approve / confidence floor. No agent-approve path in v1.
