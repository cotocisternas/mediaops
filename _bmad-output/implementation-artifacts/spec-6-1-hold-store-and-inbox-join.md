---
title: '6.1 Hold store and inbox join'
type: 'feature'
created: '2026-09-02'
status: 'done'
baseline_commit: '793a74b69bf14abe188e2ac0b4b0b6a34a66981a'
review_loop_iteration: 0
context:
  - '_bmad-output/implementation-artifacts/epic-6-context.md
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** Import-blocked releases have no typed inbox — no `holds_decisions`, no `HoldKey`/`release_id`, no `hold list` — so blocked NZBs rot as a junk drawer or look like library.

**Approach:** Persist `holds_decisions` keyed by `HoldKey {title_id, release_id}`, carry `release_id` on additive proto hold messages, map Servarr queue items only in `arr`, compute inbox as live ⊖ decided in `sync`, ship lock-free `hold list --json`. Approve/Reject wait for 6.2.

## Boundaries & Constraints

**Always:**
- `release_id` is defined in `core`, carried verbatim on the wire, mapped from Servarr queue JSON only inside `arr`. Inbox join (live ⊖ decided) runs only in `sync`.
- Store migrates `user_version` 5→6 creating `holds_decisions` (PK `(title_id, release_id)`). Refuse `>6`. `HoldsRepo` in `core`; `store` adapts; neither daemon links `store`.
- Live snapshot is seedbox `Control` (`GrabOps`/`arr` in mediaopsd). CLI uses home UDS only. `grabber=None` → empty live set, exit 0.
- `hold list` is lock-free; `--json` envelope. No `install`/`replace`; no writes under movies/series/music/`_incoming`. Offline tests; grabber failures are cassettes. Cite "holds rotting as a junk drawer".
- Additive `mediaops.v1`. TitleId on the wire is `kind:source:id`. `proto` owns conversions.

**Ask First:** Cassette queue JSON lacks Design Notes fields — do not invent a second mapping.

**Never:** approve/reject/research, auto- or agent-approve, LLM, emitting `Action::Review`, creating Hold jobs, `store` in mediaopsd, CLI/`sync` speaking *arr HTTP, `_incoming` as a hold folder, a second join outside `sync`.

## I/O & Edge-Case Matrix

- grabber=None / empty live → `{ok, holds: []}`, exit 0
- 2 importBlocked, 0 decided → both listed with age, size, reason
- 2 live, 1 decided key → inbox is the undecided remainder
- missing TitleId or release_id → omit from live (no error)
- `protocol` torrent vs usenet → infohash vs blake3(NZB name)
- exclusive flock held → list still exit 0 (lock-free); no schema/`_incoming` writes
- `user_version` > 6 or bad wire TitleId → StoreError / ControlError runtime

</frozen-after-approval>

## Code Map

Reuse: `JobKind::Hold` (no Hold rows here), `Blake3Hex::of_bytes`, `TitleId::{movie,series,album}`, `ArrClient::queue` + cassettes, `ControlPort`/`GrabOps`/`ControlPortClient`, Seedbox/HomeGateway `Control`, `LocalhostGrabOps`, CLI UDS (`connect_home`), store v5, `FakeControl`, loopback `test_support`. Do not emit `Action::Review`. mediaopsd already injects GrabOps.

Add: `core` `hold.rs` (`ReleaseId`, `HoldKey`, `HoldLiveItem`, `HoldDecision`, `HoldsRepo`) + `hold_list()` on both ports. Store v6 `holds_decisions(title_id, release_id, decision, PK)`. Proto `rpc HoldList` + `HoldLiveItem`; conversions only in `proto`. `arr::hold_items_from_queue` + importBlocked cassette; GrabOps pages sonarr/radarr/lidarr. Seedbox: None → empty, else `grab_ops.hold_list`; gateway proxies. `sync::inbox` key set-difference, live order. CLI: Control + `list_decided` + `inbox`.

## Tasks & Acceptance

**Execution:**
- [x] `crates/core/src/{hold,control_port,lib}.rs` -- types, HoldsRepo, hold_list ports; test keys -- identity
- [x] `crates/store/src/{lib,holds}.rs` -- migrate 5→6; HoldsRepo; refuse >6 -- AD-8
- [x] `proto/mediaops.proto` + `crates/proto/src/lib.rs` -- HoldList + conversions + client -- AD-3
- [x] `crates/arr/src/{lib,apply}.rs` + `fixtures/arr/` -- importBlocked map, cassette, GrabOps.hold_list; no path fields -- AD-15
- [x] `crates/net/src/{seedbox,gateway}.rs` -- HoldList serve/proxy; grabber=None empty -- AD-4
- [x] `crates/sync/src/{lib,hold}.rs` -- `inbox` join; FakeControl; matrix tests -- AD-8
- [x] `bins/mediaops/src/{main,hold}.rs` + `tests/cli.rs` -- `hold list --json` lock-free; loopback empty; no schema writes -- FR17

**Acceptance Criteria:**
- Given store at v5, when opened, then `holds_decisions` exists keyed by `(title_id, release_id)` and v>6 refuses.
- Given `release_id`, when mapped in `arr` and sent on proto HoldLiveItem, then core and wire carry the same token; `sync`/`cli` do not parse Servarr JSON.
- Given live queue and decided keys differ, when `hold list --json` runs, then `data.holds` is live ⊖ decided from `sync` (age, size, reason) and blocked NZBs are not library paths.
- Given grabber=None or an exclusive flock held, when `hold list` runs, then exit 0.

## Design Notes

`ReleaseId`: torrent = lowercase hex of `downloadId`; usenet = `Blake3Hex` of queue `title`. Join on `HoldKey`, never a scene title.

Include when `trackedDownloadState` is `importBlocked` (ci). TitleId: Radarr `movie.tmdbId` → `movie:tmdb:{id}`; Sonarr `series.tvdbId` → `series:tvdb:{id}`; Lidarr `album.foreignAlbumId` → `album:mbid:{id}`. Skip if any piece is missing.

`HoldsRepo::put` is for tests and 6.2; list only reads. `decision` is `approved`|`rejected` so 6.2 needs no extra migration. Do not advance Hold jobs. JSON holds: `title_id`, `release_id`, `age_secs=max(0, now-added_unix)`, `size`, `reason`.

## Verification

**Commands:**
- `cargo test --workspace --offline --locked` -- pass (hold matrix, cassette, store v6, CLI lock-free list)
- `cargo test -p mediaops-arch-tests --offline --locked` -- no store in mediaopsd; no arr in CLI

## Suggested Review Order

**Identity and join**

- HoldKey is title_id + release_id; never a scene title.
  [`hold.rs:67`](../../crates/core/src/hold.rs#L67)

- Torrent infohash vs usenet BLAKE3 of the NZB name.
  [`hold.rs:26`](../../crates/core/src/hold.rs#L26)

- Inbox is live ⊖ decided, live order, only in sync.
  [`hold.rs:8`](../../crates/sync/src/hold.rs#L8)

**Persistence**

- Schema 5→6 creates holds_decisions PK (title_id, release_id).
  [`lib.rs:349`](../../crates/store/src/lib.rs#L349)

**Wire and grabber**

- Additive Control.HoldList; TitleId stays a kind:source:id string.
  [`mediaops.proto:62`](../../proto/mediaops.proto#L62)

- ControlPort.hold_list is the home-side snapshot port.
  [`control_port.rs:35`](../../crates/core/src/control_port.rs#L35)

- grabber=None is empty; Servarr goes through GrabOps.
  [`seedbox.rs:281`](../../crates/net/src/seedbox.rs#L281)

- Home gateway proxies HoldList byte-for-byte.
  [`gateway.rs:184`](../../crates/net/src/gateway.rs#L184)

- Queue fetch requests nested movie/series/album objects.
  [`apply.rs:158`](../../crates/arr/src/apply.rs#L158)

- importBlocked mapping stays in arr; unmappable rows omitted.
  [`apply.rs:859`](../../crates/arr/src/apply.rs#L859)

**CLI**

- Lock-free hold list: UDS snapshot + list_decided + inbox.
  [`hold.rs:29`](../../bins/mediaops/src/hold.rs#L29)

**Peripherals**

- Lock-free empty list and no schema writes.
  [`cli.rs:711`](../../bins/mediaops/tests/cli.rs#L711)
