# Epic 7 Context: Reclaim after local proof

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

The seedbox is disposable buffer; the home filesystem is the library of record. This epic completes the why-trace so grab → import → hold → pull → encode → library (including reclaim) is visible end-to-end with local FS as truth, tells *arr to unmonitor when the file is already local, and ships a real reclaim policy: ranked dry-run preview, apply only after install-digest proof, never deleting seeding or private-under-goal torrents.

## Stories

- Story 7.1: Unmonitor and why-trace completion
- Story 7.2: Reclaim preview and apply

## Requirements & Constraints

- `why TITLE` / `status` show the full grab → import → hold → pull → encode → library chain, including stuck states (hold, watermark, lock, encode queue). Local FS is truth; *arr “file exists” is not local-exists. No media-server URL is a product requirement.
- When a title is already local (install digest in title-index) and *arr still monitors it as missing, reconcile Unmonitors via seedbox Control. The CLI never opens *arr HTTP.
- Disk-full is answered by seedbox `df` plus a reclaim preview ranked by ratio, private, and age. Local BLAKE3 proof is required before any remote delete.
- Reclaim is a real policy with a dry-run, or it does not exist (no leftover no-op timer). Preview is ranked; apply is explicit.
- Before any remote library unlink, qBit is queried and seeding skips. Private-under-goal is untouched. Usenet-complete is deletable after Copy. Torrent delete belongs to reclaim only — never sync-after-copy. A library hardlink of a torrent is left after Copy finishes.
- Skip ≠ surplus: skip means do not copy; surplus means remote may go after local proof.
- Local proof is only `install_b3`. No digest means no delete. Size/mtime is not proof. Encode replace updates `current_b3` only; reclaim still uses the immutable install digest (live file or backup original).
- One-way pull. Remote delete only for surplus after hash proof. Never two-way sync, never a third cloud, never torrent save paths or `torrents/incomplete`. Remote walks use the PathSchema allowlist; unknown paths error; never follow symlinks off it.
- Every verb takes `--json`. Stdout is a human result or a single `{ok, data, error}` envelope; stderr is tracing. The CLI talks only to local mediaopsd over a unix socket and never contains a seedbox address. Grabber HTTP stays inside seedbox mediaopsd on localhost. `grabber=None` remains valid.
- Named failures that need tests: leftover no-op reclaim timer; qBit seeding delete; size/mtime treated as proof; two-way sync mirroring local deletes; walk of torrent save paths; CLI process contains a seedbox address. Grabber failures replay as HTTP cassettes. Default tests never need the live box or a GPU.

## Technical Decisions

**Where it lives.** ReclaimPolicy lives in `core` (constraints: private, seeding, imported × objective: free-space percent). Planner and reclaim execution live in `sync`. Seedbox `Control` supplies snapshots (listing, df, grabber state, qBit guard preview) and remote mutations (Unmonitor, DeleteRemote). `sync` consumes `ControlPort`; binaries inject the proto client. `arr` is linked only into mediaopsd. Transfer never decides Copy vs Skip vs Hold and never deletes torrents.

**DeleteRemote is atomic with the qBit guard.** Query and unlink happen in one seedbox handler with no wire round-trip between them. Seeding returns typed `SkippedSeeding`. A standalone guard RPC exists only for preview/`why` rendering and is never a precondition for delete.

**Identity and proof.** `title_index` carries `install_b3` (immutable reclaim proof) and `current_b3` (what verify checks). Loss of `state.db` without export refuses reclaim until `library reindex` re-hashes — no digest, no delete. Remote listings are typed `RemoteEntry {ref, len, mtime, nlink}` from the one allowlist walker so reclaim can rank by age and detect library hardlinks.

**Plan actions.** Unmonitor, DeleteRemote, and Reclaim exist on the exhaustive `Action` enum and must apply here (types landed earlier). Planning is home-side; there is no remote Plan RPC. `run` = plan then apply of that artifact.

**Locks.** Exclusive: plan, apply, run, reclaim apply. Lock-free: why, status, reclaim preview (single-transaction store writes only). The flock-holding CLI is the executor; home mediaopsd is a gateway.

**Exits.** `core` owns ExitCode: 0 ok, 1 runtime, 2 usage, 3 lock conflict, 4 drift/verify, 5 policy refusal. Libraries never `exit`. Refusals inside an apply loop are data in the envelope, not exit 5.

## UX & Interaction Patterns

CLI-first; no separate UX contract. TUI is deferred and will attach to the tracing stream. Operator surface: `why TITLE`, `status`, `reclaim preview|apply`, plus `df`. Disk-full is that peek, not a panel. Preview is the dry-run; apply is the mutation.

## Cross-Story Dependencies

- 7.1 before 7.2: Unmonitor and the completed why-trace (including seedbox `df`) land first. Ranked reclaim preview may ship in 7.2 if 7.1 only surfaces `df`.
- Depends on Epic 4: `why`/`status` already show pull, watermark, lock, and encode-queue; Action variants existed as types. This epic applies Unmonitor / DeleteRemote / Reclaim and fills grab / hold / reclaim slices.
- Depends on Epic 6: the holds inbox is the hold slice of the why-trace; this epic does not reopen Approve/Reject.
- Epic 8 owns `new-machine` export and `library reindex`; this epic must still refuse reclaim when `install_b3` is missing.
