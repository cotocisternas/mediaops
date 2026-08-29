# Adversarial Divergence Review — ARCHITECTURE-SPINE (mediaops)

- **Lens:** adversarial — construct two units one level down that each obey every AD to the letter yet build incompatibly
- **Artifact:** `_bmad-output/planning-artifacts/architecture/architecture-mediaops-2026-08-29/ARCHITECTURE-SPINE.md` (draft, 2026-08-29)
- **Units attacked:** workspace crates `core`, `proto`, `store`, `net`, `ssh`, `arr`, `transfer`, `sync`, `encode` + binaries `mediaopsd`, `mediaops` (cli)
- **Out of scope:** SPEC constraints bind verbatim; only the HOW the spine fixes (or fails to fix) is attacked
- **Verdict:** **NOT READY TO FREEZE.** The dependency graph and side-effect ports are genuinely tight — most classic divergences (second HTTP stack, second path renderer, second cert minting path, second resume format, second timer) are closed. But the spine places *edges* better than it places *entities*: the job state machine, the hold, the desired-state snapshot type+hash, the remote listing shape, the staging layout, the WAN channel pool, and the wire error detail all sit between two crates with no fixed owner or shape. I found **3 critical**, **8 high**, **3 medium**, **1 low** divergence pairs. Every one is closable with a new or sharpened AD; none requires re-architecting.

Method: for each shared entity, I built two crate-level implementations that each satisfy every AD rule literally, then checked whether they compose. A pair that compiles-but-lies (silent wrong behavior) tiers higher than a pair that fails loudly at integration.

---

## CRITICAL

### ADV-1 — The WAN channel pool has no process, and N is unreadable by the process that must own it

**Units:** `transfer` (in the CLI process) vs `mediaopsd` (home role); accessory: `store`.

**Incompatible constructions.** AD-12 says probed concurrency N is "the number of independent TCP+TLS gRPC channels … owned by `net`'s channel pool" — but `net` links into *both* `transfer` and the daemon, and AD-4 routes all seedbox traffic CLI → UDS → home `mediaopsd` → mTLS/TCP → seedbox, with the seedbox address known *only* to home `mediaopsd`. So the only process that can hold N WAN TCP channels is home `mediaopsd`.

- *Construction A (`transfer`):* opens N concurrent `GetRange` streams over the UDS, believing (per AD-12's letter, "one in-flight GetRange stream per channel") that it controls parallelism.
- *Construction B (home `mediaopsd`):* implements AD-4's letter — "proxies control and Range streams" — with one default tonic upstream client. All N UDS streams multiplex onto **one** HTTP/2/TCP connection to the seedbox.

Both are fully compliant. The composed system silently recreates the single-TCP ceiling — the *exact named failure AD-12 exists to prevent* — and no test in AD-20's suite would catch it offline.

Second half of the same hole: N lives in `probes` in home `state.db`, `store` links only into the CLI (AD-2), and AD-8 says "neither daemon role links `store`." The gateway that must size the pool **cannot read N**. Compliant builds: daemon duplicates N in its own config (drifts from `probes` after re-probe) vs CLI passes nothing (proto has no field for it) and the gateway guesses.

**Tightening (sharpen AD-4 + AD-12).** The home gateway owns the N-channel WAN pool. Add to `proto` a gateway-only UDS RPC (`ConfigurePool{n}` / `PoolStatus`) so the CLI, which reads `probes`, sets N at run start. State the invariant as a rule: *each proxied Range stream is pinned 1:1 to a dedicated upstream channel; the N+1th concurrent stream is refused with `ResourceExhausted`, never queued onto a shared channel.* Add a failure-history test: N concurrent streams must produce N distinct upstream connections (fake-transport transcript per AD-20).

### ADV-2 — `range_len` cannot obey the size convention; every builder deviates differently

**Units:** `core`/`cli` (desired-state parse) vs `transfer` (range scheduler + sidecar); accessory: `sync resume`.

**Incompatible constructions.** The Consistency Conventions fix config sizes as "GiB as integers." AD-12 fixes `range_len` default **64 MiB** as a desired-state value. 64 MiB is not representable as an integer GiB — the spine contradicts itself, so every builder *must* deviate, and each deviates differently:

- *Construction A (`core` config):* `range_len = 64` parsed under the GiB convention → 64 GiB ranges → every file is one range → single stream per file → the AD-12 ceiling again, silently.
- *Construction B (`transfer`):* reads the AD-12 prose, treats the field as MiB; writes the sidecar's `range_len` field (AD-11 JSON) in **bytes**. A resume implementation reading the sidecar under the config unit re-slices ranges at the wrong offsets → recorded per-range hashes never match → resume restarts from zero, breaking CAP-4, or worse, "verifies" the wrong slices.

**Tightening (sharpen the Config convention + AD-11).** Replace "sizes in GiB as integers" with: *all desired-state sizes carry the unit in the field name (`max_copy_gib`, `min_free_gib`, `range_len_mib`); `core` converts every size to a `Bytes(u64)` newtype at parse; no bare integer size crosses a crate boundary.* Sharpen AD-11: *sidecar `file_len`, `range_len`, `offset`, `len` are always bytes; resume uses the sidecar's `range_len`, never the current config's* (the plan-carries-config principle of AD-9 extended to the sidecar).

### ADV-3 — The job state machine is placed nowhere; `sync` and `encode` each mint their own, and `store` can type-check neither

**Units:** `sync` vs `encode`; forcing wall: `store` (AD-2).

**Incompatible constructions.** AD-10 mandates "a typed per-kind state machine; state transitions are the only writes" and binds `core, store, sync, encode` — but never places the machine. The Structural Seed's `core` line lists "TitleId, PathSchema, Plan, policies, budgets, ExitCode, Provider trait" — **no job types**. Meanwhile AD-2 makes the placement question unanswerable by accident: `store`'s only outgoing edge is `store → cli`, so `store` cannot name any type defined in `sync` or `encode`; and `sync`/`encode` cannot depend on `store` (no such edges), so AD-8's "every other crate consumes typed repository traits" is only satisfiable if *both* the repo traits *and* the state types live in `core` — which no AD states.

- *Construction A (`sync`):* defines `enum PullState { Queued, Pulling, Verifying, Installed }` privately, persists transitions through a stringly repo method `set_state(job_id, "verifying")`.
- *Construction B (`encode`):* defines its own `EncodeState`, serializes as JSON `{"phase":"ENCODING"}` into the same `jobs.state` column.

Both compile, both "have typed state machines," and: `store` enforces no transition legality (the AD-10 invariant is decorative), the CLI's `why`/`status` must parse two ad-hoc encodings, and the pull→encode handoff (a Copy finishing making an Encode job ready) has **no legal home** — `sync` and `encode` share no edge, and AD-1 forbids the wiring logic living in the binary.

**Tightening (sharpen AD-10 + AD-8).** *`core::jobs` owns `JobKind`, the per-kind state enums, and `advance(state, event) -> Result<state>` as pure functions; `store`'s jobs repository trait lives in `core`, persists only these types, and exposes `advance` as the sole state write (illegal transitions are a repo error, not a caller convention).* Fix the chain shape: the planner (AD-9) creates one job row per Plan action, linked to the originating want by `parent_job_id`; an Encode job's readiness predicate ("parent Copy job = Installed") is evaluated by `core::jobs`, not by `sync` or `encode`.

---

## HIGH

### ADV-4 — The hold entity exists three times with no shared key and no join owner

**Units:** `arr` (live import-blocked queue, inside the daemon) vs `store` (`holds_decisions`); accessory: AD-10 (a hold is *also* a `jobs` row).

**Incompatible constructions.** CAP-8's inbox = (live \*arr queue) ⊖ (decided set) ⊕ (hold job state). The spine fixes none of: the hold's identity key, which representation is authoritative, or which crate computes the join.

- *Construction A (`arr`/daemon):* returns queue items keyed by Servarr **queue item ID** — which Servarr regenerates when it re-queues a release, so "Reject means never *this release*" silently forgets rejections.
- *Construction B (`store`):* keys `holds_decisions` by `(TitleId, release_title)` after scene-tag normalization — which `arr` never applied, so approve/reject decisions fail to join against the live view; the same NZB shows as undecided forever.

Both obey AD-8 and AD-15 to the letter; the inbox is wrong in both directions.

**Tightening (new AD).** *The hold identity is `HoldKey { title_id, release_id }` where `release_id` is the durable release identifier (usenet: nzb name hash; torrent: infohash), defined in `core`, carried verbatim in `proto`'s hold messages, and the primary key of `holds_decisions`. The daemon's Control hold-listing must return `HoldKey` (the `arr` crate maps Servarr queue items to it — the only place Servarr IDs are visible). The inbox join (live ⊖ decided) is computed in `sync`, nowhere else.*

### ADV-5 — Nobody owns the `DesiredState` type or the snapshot-hash recipe

**Units:** `mediaops` (cli, "snapshot config") vs `sync` (drift check / apply-from-plan); accessory: `core`.

**Incompatible constructions.** AD-9: a Plan embeds "the full desired-state snapshot plus its hash." The Structural Seed's `core` line does not list a desired-state document type (only "policies, budgets"), and no AD names the hash algorithm or the byte-domain it runs over. (AD-14's aside even scopes "the BLAKE3-only lock … to content digests" — is a config snapshot a content digest? Two defensible answers.)

- *Construction A (cli):* snapshots the raw TOML file bytes, hash = blake3(file bytes), embeds the TOML string in the Plan JSON.
- *Construction B (`sync`):* parses, re-serializes the snapshot to canonical JSON inside the Plan, hash = blake3(canonical JSON). At apply, B recomputes its canonical hash and compares against A's file-bytes hash → **every** apply reports drift (exit 4), or the builder "fixes" it by skipping the check entirely.

Also structural: since the Plan is a `core` type and Plan embeds the snapshot, `DesiredState` is *forced* into `core` by the graph — but a to-the-letter builder who keeps it in the cli (where "snapshot config" happens per AD-1) will embed an untyped string and push parsing into `sync`, duplicating the parse.

**Tightening (sharpen AD-9).** *`core` owns `DesiredState` (serde TOML, `deny_unknown_fields`, `schema_version`) and the snapshot rule: the Plan embeds the exact raw TOML bytes of the snapshotted file plus `blake3(bytes)`; apply re-parses from the embedded bytes only; every hash comparison anywhere is bytes-hash vs bytes-hash. No canonical-serialization hashing exists.*

### ADV-6 — The remote listing's shape is unfixed: the planner needs fields the wire may not carry, and path form can diverge between plan and pull

**Units:** `sync` (planner input) vs `proto`/daemon `Transfer` service (listing producer); accessory: `transfer` (`GetRange` path argument).

**Incompatible constructions.** AD-13 fixes *one walker* (good) but not what a listing entry **is**. Remote buffer paths are scene-named, not PathSchema-parseable, so "library paths are opaque values" doesn't define them; and the planner's laws need per-entry facts the spine never requires the wire to carry: size (budget), mtime/age (reclaim ranking), hardlink count ("library hardlink of a torrent: leave the torrent"), kind (music-first ordering).

- *Construction A (`proto`):* `ListResponse { path: string (absolute), len }` — minimal, compliant. `sync` now cannot distinguish a torrent hardlink from a usenet-complete file → plans `DeleteRemote` for a seeding torrent's library link (the qBit guard saves the unlink but the plan is systematically wrong), and cannot rank reclaim by age at all.
- *Construction B (`sync`):* plans with paths **relative to the allowlisted root** (natural for an allowlist walker); `transfer` sends that string to `GetRange`, whose daemon-side handler expects absolutes → every pull errors, or worse, a lenient handler resolves relative paths against the wrong root.

**Tightening (sharpen AD-13 + AD-3).** *`core` defines `RemoteRef { root_id, rel_path }` and `RemoteEntry { ref, len, mtime, nlink }`; the walker is the sole producer of both; `proto` mirrors them field-for-field; the `Transfer` service's listing returns `RemoteEntry`, and `Stat`/`GetRange` accept `RemoteRef` — never a bare path string. The planner and the pull consume the same message.*

### ADV-7 — Staging directory layout has three toucher crates and no owner

**Units:** `transfer` (writes `.partial` + sidecar) vs `cli`/`core` library bootstrap (creates "staging dirs") vs `sync` (resume listing, GC/empty-dir prune sparing).

**Incompatible constructions.** AD-11 makes staging *files* sacred and bootstrap-surfaces makes `library bootstrap` create "staging dirs" — but nothing fixes where staging lives (`_incoming`? `_ops`? per-title subdirs?) or who renders the path.

- *Construction A (`transfer`):* stages at `_ops/staging/<plan-id>/<final-name>.partial` (plan-scoped temp feels idempotent-friendly). `sync resume` — built to "list the `.partial`" — scans `_incoming/` (the obvious app-managed inbox) and finds nothing; a killed copy is invisible to resume, breaking CAP-4 while every AD is obeyed.
- *Construction B (GC in `sync`):* spares "staging dirs" it defines as *dirs currently containing a `*.partial`* — a dir where transfer has written only the sidecar so far (or where the `.partial` was renamed first during install) gets pruned mid-operation.

**Tightening (new AD).** *Staging root is `_incoming/` (already app-managed, never a library). Layout is `_incoming/<TitleId serialized>/<final-name>.partial{,.b3}`, rendered by exactly one function `core::pathschema::staging_path(TitleId, final_name)`; `transfer` (write), `sync` (resume scan), GC/empty-dir prune (spare rule: any dir under `_incoming/` containing `*.partial*` is sacred; sidecar-only counts), and `library bootstrap` (mkdir) all call it. GC of stale staging is a named explicit command, never implicit.*

### ADV-8 — "Machine-readable detail field" is two different bytes on two sides of the wire, and the gateway may destroy it anyway

**Units:** `mediaopsd` (Status producer) vs `mediaops` cli (Status → ExitCode consumer); accessory: home gateway passthrough.

**Incompatible constructions.** The Errors convention says gRPC errors are `tonic::Status` "with the ExitCode-aligned reason in a machine-readable detail field." No AD says *which* field or *what encoding*, and `core` can't define it (no tonic dep), while AD-3's "sole home of conversions" only covers `.proto` messages someone actually writes.

- *Construction A (daemon):* packs a serialized `ErrorDetail` proto into `Status::details` (grpc-status-details-bin).
- *Construction B (cli):* reads a custom metadata header `mediaops-exit-code: 5`. Both are "machine-readable detail fields." The CLI sees only the Status code, maps everything to exit 1, and AD-17's taxonomy (policy refusal ≠ drift ≠ runtime) evaporates at the process boundary.

Independent second pair: the home gateway (AD-4 "proxies control and Range streams") built naively re-wraps upstream errors as `Status::internal("transport error")` — compliant, and it strips the detail even when both ends agreed.

**Tightening (sharpen AD-3 + AD-17).** *`proto` defines message `mediaops.v1.ErrorDetail { exit_code, reason, message }` and owns the only two functions that build a `Status` from a domain error and parse one back; both binaries must use them (extend AD-2's CI walk: `tonic::Status` construction outside `proto` fails the build, if feasible; else state it as a rule). New rule in AD-4: the gateway forwards upstream `Status` code, message, and details byte-for-byte; it never re-wraps.*

### ADV-9 — Encode's replace cannot pass the install gate as specified, and the title-index digest lies after every encode

**Units:** `encode` vs `core` (install gate) and `sync`/`store` (verify + reclaim proof).

**Incompatible constructions.** The State-mutation convention says "library paths via install gate only," but AD-13's gate signature is `(TitleId, verified staging handle) → installed path` — a remote-pull shape. Encode's reversible transaction (`.converting` → replace → original to backup) writes library paths with no staging handle.

- *Construction A (`encode`):* reads the gate as pull-scoped and writes the replace directly — a second library-path writer, the exact thing AD-13 prevents.
- *Construction B (`sync`/cli `verify`):* hashes the live file against `title_index`'s digest, which the SPEC fixes as *the install BLAKE3*. After any encode-replace, the live file is the H.264 output, the install digest describes the original now sitting in backup → verify reports drift (exit 4) on every encoded title forever, or a builder "fixes" `verify` by having encode overwrite the digest — at which point reclaim's local proof ("uses only the install digest") compares the remote original against the encode output hash and can never prove, so reclaim goes dead.

**Tightening (sharpen AD-13 + AD-8).** *The install gate gets a second entry point: `replace(TitleId, verified converting handle, backup destination) → installed path` — still the only library-path writer; `encode` must use it. `title_index` carries two digests: `install_b3` (immutable; the reclaim/local-proof digest, satisfiable by the live file or the backup original) and `current_b3` (updated only by the gate on install or replace; what `verify` checks). State which digest each consumer reads.*

### ADV-10 — The qBit seeding guard can be built as check-then-delete across the wire (TOCTOU)

**Units:** `sync` (reclaim execution orchestration) vs `mediaopsd` seedbox Control (`qBit guard`, `DeleteRemote`).

**Incompatible constructions.** The constraint "before any remote library unlink, query qBit; if seeding, skip" binds both, and AD-4 lists "qBit guard" and "DeleteRemote" as two remote mutations — inviting two RPCs.

- *Construction A (`sync`):* calls `QbitGuard(path)` → "not seeding" → calls `DeleteRemote(path)`. Fully compliant: it queried before the unlink. Between the two RPCs the torrent resumes seeding (tracker re-announce, operator click) → the delete lands on a seeding torrent — the named live failure.
- *Construction B (daemon):* embeds the guard inside the `DeleteRemote` handler atomically and expects no separate guard call; a `sync` built as A now guards twice with different staleness, and a `sync` that assumes B while talking to a daemon built as A guards **zero** times.

**Tightening (sharpen AD-4).** *The qBit guard is part of the `DeleteRemote` implementation inside the seedbox daemon — query and unlink in one handler, no wire round-trip between them; `DeleteRemote` returns a typed `SkippedSeeding` outcome. A standalone guard RPC exists only for preview/`why` rendering and is never a precondition for delete.*

### ADV-11 — Which subcommands take the flock is unfixed; both legal partitions break a CAP

**Units:** `mediaops` cli (lock policy per subcommand) vs `store` (concurrent writers) — with a 2-hour `run` as the background.

**Incompatible constructions.** AD-4 enumerates the flock-holder verbs: "plan/apply/pull/verify/install/encode." `watch`, `hold reject`, `encode pause`, `status`, `reclaim preview` are not in the list.

- *Construction A (lock-everything cli):* every subcommand takes the machine-global flock. During any long copy, `watch TITLE` exits 3 — violating CAP-1's success line ("`watch TITLE` exits 0 … without occupying a console"), and CAP-10's "pausable queue" is unreachable while encoding.
- *Construction B (lock-listed-verbs-only cli):* `watch` and `hold reject` write `jobs`/`holds_decisions` rows concurrently with a running apply — two writers to `state.db` with no stated isolation contract, while AD-4's headline ("the flock holder is the only executor") reads as if it prevented exactly this.

**Tightening (sharpen AD-4).** *Define two lock classes explicitly: exclusive (plan, apply, run, sync resume, encode run, reclaim apply, repair, bootstrap) and lock-free (watch, why, status, hold list/reject, reclaim preview, encode pause, docs). Lock-free commands may only perform single-transaction row inserts/updates on `jobs`/`holds_decisions` through `store`; the executor treats those tables as plan-time snapshots (new rows are next-run input, never mid-apply input). `encode pause` is fixed as a store flag the executor polls between jobs — not a signal to the lock-holder's pid.*

---

## MEDIUM

### ADV-12 — Remote-mutation apply orchestration has no crate that can legally hold it

**Units:** `sync` (apply orchestration per AD-4/Structural Seed) vs `proto` (the only legal RPC types).

**Incompatible constructions.** The Plan's remote actions (`Unmonitor`, `DeleteRemote`, `GrabApply`, `EdgeApply`) execute over Control RPCs, but AD-2 has no `proto → sync` edge — `sync` cannot name a generated client. *Construction A:* `sync` defines its own `ControlPort` trait in `sync` (or `core`?) and the cli implements it over proto clients — port home unstated, two builders put it in two crates. *Construction B:* the cli matches on `Action` and dispatches RPCs itself — apply orchestration in a binary, against AD-1's spirit and AD-4's "the code lives in the sync/transfer/encode libraries," but defensible from the diagram.

**Tightening (sharpen AD-2/AD-3).** *`core` defines the `ControlPort` trait (Unmonitor, DeleteRemote, GrabApply, EdgeCheck, Df, QbitGuard-preview, KeyDiscovery); `proto` ships the canonical implementation over its generated clients (proto already depends on core); binaries only inject it. Add the `proto → sync` question to rest: sync consumes the trait, never the clients.*

### ADV-13 — The re-probe trigger keys on a value only the daemon knows, stored where only the CLI can write

**Units:** `store` (persists N in `probes`) vs home `mediaopsd` (sole holder of the seedbox address/underlay per AD-4).

**Incompatible constructions.** AD-12: N is "re-probed only when the bind address or underlay changes." The CLI/`store` side never sees the address (AD-4 forbids it), so it cannot detect the change; the daemon sees it but "neither daemon role links `store`" (AD-8). *Construction A:* store keys `probes` by nothing → N survives a box migration, silently wrong. *Construction B:* daemon config grows its own `probe_generation` counter the operator must bump — a second machine-state location, against AD-6.

**Tightening (sharpen AD-12).** *The home gateway exposes an `endpoint_fingerprint` (hash of seedbox address + underlay mode) on a UDS status RPC; `store` persists N keyed by that fingerprint; the CLI compares at run start and triggers re-probe on mismatch.*

### ADV-14 — ExitCode aggregation over a multi-action run is undefined

**Units:** `mediaops` cli (the one error→ExitCode mapping point, AD-17) vs `sync`/`encode` (per-action outcomes).

**Incompatible constructions.** A `run` advances many jobs; one encode `Refuse` (a policy outcome, exit 5's domain) or one opened hold occurs alongside nine successes. *Construction A:* any policy refusal anywhere → exit 5; timer-driven runs now "fail" nightly on every HDR title, and monitoring cries wolf. *Construction B:* exit 0 with per-action results in the JSON envelope; CAP-1's "playable file **or an open hold**" reads as success. Both obey AD-17's letter. The same ambiguity hits `verify` (is one drifted title exit 4?).

**Tightening (sharpen AD-17).** *ExitCode reflects the command's own contract, not per-action outcomes: `run`/`apply` exit 0 if the apply loop completed (refusals, holds, and skips are data in the envelope and tracing events), 1 only if the loop itself broke, 3 on lock, 4 only for verbs whose purpose is verification (`verify`, `doctor` drift), 5 only when the *command's primary action* was refused (e.g. `encode run FILE` on an HDR title).*

---

## LOW

### ADV-15 — The Plan artifact fits no data tier and has no home or lifecycle

**Units:** `sync`/cli (plan writer) vs GC / AD-6 taxonomy.

**Incompatible constructions.** AD-6's tier 3 enumerates "lockfile, `.partial` + sidecar, `tls/` PEMs" — the Plan JSON (a first-class artifact per AD-9, applyable later as a standalone `apply`) is in no tier. *Construction A:* plans written to `_ops/plans/` on the library disk; *Construction B:* plans in `~/.local/state/mediaops/`; neither is GC'd, or one is pruned by the empty-dir logic. Loud failure at worst, clutter at least.

**Tightening (sharpen AD-6/AD-9).** *Plans are runtime artifacts: add "plan JSONs under `~/.local/state/mediaops/plans/`" to AD-6 tier 3; a plan is invalidated (refused by apply) when its embedded snapshot hash no longer matches the active desired-state file; `run` deletes its plan on completion; stale plans are pruned by an explicit command.*

---

## Closed doors (attacked, held)

For completeness, divergence pairs I constructed that the spine already kills: a second HTTP stack (AD-2 CI walk), a second path renderer (AD-13), CLI learning the seedbox address (AD-4), two cert-minting paths or fingerprint format drift (AD-14), a second resume format (AD-11's versioned sidecar), a second daemon binary (AD-5), provider stubs silently succeeding (AD-21), a second timer on the seedbox (deployment diagram + AD-4 leave it nowhere to live), TitleId serialization drift (conventions fix `kind:source:id`), and native-tls (AD-14).

## Priority of closure

1. ADV-1, ADV-2, ADV-3 — each silently defeats a headline capability (parallelism, resume, job recovery) with fully compliant crates.
2. ADV-4 through ADV-11 — shared-entity shape/owner fixes; all are one-paragraph AD sharpenings.
3. ADV-12–ADV-15 — wiring and taxonomy; cheap to close in the same editing pass.
