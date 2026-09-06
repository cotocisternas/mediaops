# Architecture session prompt

> **Historical prompt. Do not use it to start current architecture work.** Its
> locked decisions describe the pre-rewrite system. Start from the
> [current architecture](../../../docs/architecture.md) instead.

Paste this as the first message in a **fresh** window, in the `mediaops` repo.

```
/bmad-architecture

Create a new architecture spine. Fast path — spec is decided; tag remaining HOW forks as [ASSUMPTION], do not coach the five-field kernel or re-open locks.

Input is the spec package (read in this order; later memlog lines win):

1. _bmad-output/specs/spec-mediaops/SPEC.md
2. Every file listed in SPEC.md frontmatter `companions:`
3. _bmad-output/specs/spec-mediaops/.memlog.md

Do not treat the brainstorm folder or party-mode memlog as peer sources. They are already absorbed.

Product: mediaops — Rust CLI + seedbox daemon that reconciles desired state across a rented seedbox and a home archive disk. Replaces the live Python under ~/videos. This workspace is the repo of record.

Locked — do not re-open:

- Formal API is mediaopsd gRPC with mTLS. Self-signed CA + server + client certs generated at bootstrap. v1 is mTLS only; bearer token deferred.
- Cert PEMs are gitignored files next to desired-state. Desired-state stores fingerprints + paths, never PEMs. Doctor refuses if PEMs sit inside a git work tree.
- Overlay is in-process (rathole the idea, not the app). This SeedIt4Me box: mediaopsd binds. Reverse-connect and Tailscale/WireGuard are designed, unused by default, not required to demo the first Range RPC.
- SSH is bootstrap only (install unit, mint/place certs). Not SSH + localhost HTTP as the API. Not ssh -L. No emergency rsync-ssh path.
- Data plane: parallel Range RPCs on the same gRPC/mTLS. BLAKE3 per-range (in the .partial map) + whole-file BLAKE3 at schema install. Reclaim proof is the install digest only. Not FTP, rsync, SFTP, or rclone-as-the-pipe.
- Home CLI talks only to local mediaopsd on a unix socket and never contains a seedbox address.
- watch TITLE enqueues (wanted; and monitored if grabber is on). Playable is the timer/run job, including encode. Peek via why/status. No attached console.
- First demo on this box is the pipe (plan → Range pull → .partial resume → schema install) AND encode (HEVC-MP4 movie under NVENC). grabber=None is a valid demo path. GPU is this-box demo, not CI. See companions/increments.md.
- Bootstrap both a new seedbox and a new local media directory. Jellyfin/Plex configuration is out of scope.
- Keep Sonarr, Radarr, Prowlarr, SABnzbd, qBittorrent, Lidarr (version-pinned). Jackett, Autobrr, Bazarr out (not even stubs). Recyclarr absorbed as GrabPolicy data, not a required binary.
- v1 is this SeedIt4Me/Swizzin box + this home disk. Provider trait may exist; v1 ships SwizzinBox + AlreadyThere only.
- Local filesystem is library of record; *arr is an optional grabber. One-way pull. Never torrent save paths.

module-map.md is intended module responsibilities, not a frozen crate graph. Architecture may shuffle impl crates. It must not put grabber HTTP in the CLI or FTP/rsync in transfer.

Spine job:

- Altitude: whole product, scoped so independently-built crates cannot diverge.
- Purpose: build substrate for implementation of the first demo (pipe + encode), not a board deck.
- Fold the spine back into the spec package when done (offer bmad-spec to adopt ARCHITECTURE-SPINE.md as a companion; keep AD IDs stable).
- Verify current crate versions (tonic/prost, rustls, blake3, clap, sqlite, NVENC/ffmpeg bindings if any) on the web before pinning.
- Failure-history tests in failure-history-tests.md are the test-strategy seed; unit tests never require the live box.

Do not implement code. Do not run story breakdown unless I ask after the spine exists.
```
