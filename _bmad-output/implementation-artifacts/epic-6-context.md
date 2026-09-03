# Epic 6 Context: Holds are an inbox

<!-- Compiled from planning artifacts. Edit freely. Regenerate with compile-epic-context if planning docs change. -->

## Goal

Import-blocked is a product feature, not a junk drawer. This epic makes blocked releases a typed operator inbox: list age, size, and *arr reason; Approve promotes through PathSchema; Reject means never-this-release and lets *arr try another. Auto-approve is impossible, there is no agent-approve path in v1, and blocked NZBs never become library. An unattended `run` may then leave an open hold instead of a folder a media server might ingest.

## Stories

- Story 6.1: Hold store and inbox join — shipped PR #7
- Story 6.2: Approve and Reject — shipped PR #8

## Requirements & Constraints

- Inbox shows import-blocked (and similar) with age, size, and *arr reason. The surface is *arr message plus ffprobe plus Approve / Reject / Research — not a media-server folder.
- Approve promotes through the install gate onto a schema path. PathSchema still refuses spaces and leftover scene tags (including REPACJ, REPACK, PROPER).
- Reject is never-this-release; *arr may try another.
- Auto-approve is forbidden. No agent-approve path and no confidence floor in v1. A Research verb may exist as a stub or be omitted until CAP-11; it must not call an LLM. The inbox still ships.
- `_ops` and `_incoming` are app-managed, never libraries; `_incoming` is not a hold folder. Do not add them to a media server even as a default. If a server already scans them, warn — do not reconfigure Jellyfin/Plex.
- `needs-split` is a workflow (agent), not a pile to ingest.
- Every hold verb takes `--json`. Stdout is a human result or a single `{ok, data, error}` envelope; stderr is tracing (JSON lines when not a tty). Timers never require a TUI.
- The CLI talks only to local mediaopsd over a unix socket and never contains a seedbox address. Grabber HTTP stays inside seedbox mediaopsd on localhost. The planner must not speak Sonarr HTTP. `arr` must not cherry-pick “just queue + history.”
- *arr is an optional grabber (`grabber=None` is valid). Local FS is the library of record.
- An apply loop that opens a hold still completes: holds, refusals, and skips are data in the envelope and tracing, not a policy-refusal exit for `run`/`apply`.
- Named failures that need tests: holds rotting as a junk drawer; agent auto-approve / confidence floor. Grabber failures replay as HTTP cassettes. Default tests never need the live box or a GPU.

## Technical Decisions

**Where it lives.** Holds are planned in `sync`, persisted by `store`, and sourced live through seedbox `Control` (`arr` linked only into mediaopsd). Transfer must not decide Copy vs Skip vs Hold. Planning is home-side: Control supplies grabber-state snapshots (including hold listing) and remote mutations; `sync` consumes `ControlPort`; binaries inject the proto client. Wire evolution is additive-only in `mediaops.v1`; `proto` is the only home of wire↔domain conversions and of hold messages.

**Identity and join.** The hold key is `HoldKey {title_id, release_id}`. `release_id` is the durable release identifier (usenet: NZB-name hash; torrent: infohash), defined in `core`, carried verbatim on the wire, and mapped from Servarr queue items only inside `arr`. `holds_decisions` uses that key. The inbox is live ⊖ decided, computed in `sync` only — nowhere else. Do not key decisions by scene-normalized title strings.

**Persistence and jobs.** One home `state.db`, touched only by `store` (rusqlite behind `spawn_blocking`). Repository traits live in `core`; binaries inject the adapter; neither daemon role links `store`. Migrations are embedded, numbered, forward-only via `PRAGMA user_version`; table/column names are snake_case. This epic creates `holds_decisions` (it was deferred from the first store migration). A hold is also a `jobs` row: `core::jobs` owns `JobKind`, per-kind state enums, and pure `advance()` as the sole state write; illegal transitions are repository errors. The planner links action jobs to wants via `parent_job_id`. New decision/job rows are next-run input, never mid-apply input.

**Lock classes.** `hold list`, `hold approve`, and `hold reject` are lock-free: they may only perform single-transaction row writes through `store`. Exclusive flock stays on plan/apply/run, install, encode, reclaim apply, repair, bootstrap. The flock-holding CLI is the only executor of install/encode; home mediaopsd never writes staging or library paths. Approve records a decision; promotion through `install` happens on the exclusive apply path. `Review` already exists on the Plan `Action` enum; this epic is the inbox that feeds it, not a second action type.

**Install gate.** Library paths have exactly two writers: `install` and `replace`. Staging is `_incoming/<TitleId>/…` from `core::pathschema::staging_path` — that tree is pull staging (sacred `*.partial*`), not the hold inbox.

**Grabber port.** All *arr / SAB / qBit HTTP goes through `HttpTransport`; the reqwest impl is daemon-only; tests replay cassettes. ffprobe (inbox context) goes through the shared exec port, not a lib binding.

**Exits.** `core` owns ExitCode: 0 ok, 1 runtime, 2 usage, 3 lock conflict, 4 drift/verify, 5 policy refusal. Libraries never `exit`.

## UX & Interaction Patterns

CLI-first; no separate UX contract. TUI is deferred and will attach to the tracing stream, not a second API. Operator surface: `hold list|approve|reject` (optional `research` stub). The inbox is those verbs, not a folder.

## Cross-Story Dependencies

- 6.1 before 6.2: the decisions table, HoldKey on the wire, and live ⊖ decided join must exist before Approve/Reject can persist and drop out of the inbox.
- Depends on earlier work already in types and runtime: TitleId / PathSchema / install gate, jobs + `store`, Control/arr snapshots, plan/apply executor, lock classes. `holds_decisions` was explicitly not created with `title_index` / `jobs` / `probes`.
- Epic 7 completes the why-trace grab → hold → reclaim chain and Unmonitor/reclaim. This epic ships the inbox; it does not require the full why slice or reclaim.
- CAP-11 LLM research stays deferred; v1 only reserved the capability-token enum in `core`.
