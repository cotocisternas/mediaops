---
title: Operator feedback and current interfaces
created: 2026-09-06
status: done
baseline_commit: 9b76f3db6827bd4be9d7105e4ce50c3ad056d163
---

# Operator feedback and current interfaces

The current implementation is grounded in `usage.md`, `tui.md`, and
`architecture.md`. Historical generated specifications are not active scope.

## Intent

Library reindex can spend hours reading media without showing that it is working.
The operator also needs understandable work, progress, and failure information
across the CLI and TUI. The v0.2.0 interface should consistently use the Home API;
old SQLite workflow selection and a second JSON interface are unnecessary.

Improve feedback using actual observed stages and byte counts. Give slow work
an immediate start indication, periodic feedback and an unambiguous outcome.
Keep human output readable, machine output parseable, and TUI writes guarded by
current state and exact identity.

## Boundaries

Preserve PathSchema, TitleId, immutable installation digests, file verification,
maintenance locks, exit codes, and gateway-only seedbox communication. A failed
maintenance operation must still explain any retained Cluster lock. No new
prompts, auto-approval, external metadata requests, seedbox access or GPU tests.
The existing local capability store may support hardware settings and locks;
it must not select a separate operator workflow. No version bump is required.

## Audit and implementation map

| Surface | Finding | Work |
| --- | --- | --- |
| Library reindex | Synchronous hash, publication and API verification are silent | Report scan, file hash, proof verification/publication, count and elapsed time |
| Other slow CLI commands | Results appear only after work finishes | Add human-only operation feedback, with periodic elapsed time during waits |
| Manual pull | Meter reports byte counts without percentage | Expose percentage and useful rate; end the meter on errors |
| CLI status / why | Job phases alone hide transfer progress and failure causes | Show bytes/percentage, reason and relevant scheduler/worker state |
| Object tables | Wide output can hide operational facts; empty tables are ambiguous to humans | Improve wide columns while preserving TSV for pipelines |
| Sync | Planning waits and outcome distinctions need clear feedback | Show planning stage and actionable report outcomes |
| TUI Overview / Wants | Raw IDs obscure names; scheduler pauses absent | Reuse readable names and expose current Cluster pause state |
| TUI Jobs | Copied bytes omit total and percentage | Show honest phase-specific progress and failures |
| TUI Nodes | Raw heartbeat timestamps obscure freshness | Render age/readiness accessibly |
| TUI navigation | Missing row/scroll position and explanations for unavailable actions | Add contextual feedback without weakening mutation guards |
| Public CLI compatibility | Custom `--state-db`, `--json` and `import-legacy` retain retired workflows | Remove old public routes and use raw `-o json` consistently |
| Home maintenance module | `api_legacy.rs` also contains necessary current API functionality | Rename to `home_library.rs`; retain current proof and maintenance logic |

Key paths: `bins/mediaops/src/main.rs` owns command routing and errors;
`library.rs` and `home_library.rs` own reindex and proof publication; `out.rs`
owns human formatting and pull metering; `api_cmd.rs` owns object/status/why
rendering; `sync_cmd.rs` and `sync_format.rs` own sync reporting. The new
`progress.rs` provides progress without depending on asynchronous I/O scheduling.
`bins/mediaops-tui/src/projection/` owns screen facts and `view*` owns presentation.
Existing exact-screen and interaction tests define the observable boundaries.

## Acceptance

- Given human reindex, when hashing or API verification takes time, stderr shows
  the current stage, file where known, byte progress where measurable, and elapsed
  time; successful stdout reports indexed files and the library root.
- Given structured output or redirected streams, when a command runs, stdout
  remains parseable and redirected output contains no terminal control sequences.
- Given a changed file or an I/O failure, when reindex fails, no successful
  completion is printed and verification/maintenance refusal remains intact.
- Given a missing library directory, when reindex runs, it reports an error
  rather than a successful empty index.
- Given running, verifying, failed or refused Jobs, when their CLI/TUI views are
  rendered, the phase, measurable progress and recorded reason are distinguishable.
- Given a stale TUI baseline, when an operator navigates, stale data is marked
  and write guards stay disabled; known empty data remains distinguishable.
- Given current commands, when requesting `-o json`, one raw result is emitted;
  deprecated CLI flags and import syntax no longer select old behavior.

## Verification

Run meaningful reader/progress and exact-screen tests, CLI argument and Home API
integration tests, and TUI render/interaction tests. Then run `make test` with
its required sibling-binary build, `make fmt-check`, and `make clippy`. Inspect
retired references to distinguish public compatibility from internal data storage.
No live seedbox or encode execution is part of verification.


## Review findings resolved

- Human progress uses complete lines for commands that also emit warnings;
  reindex and pull use bounded in-place terminal updates. Result output follows
  completion of the progress indicator.
- A stalled-API regression waits for a later periodic update, proving feedback
  continues after the initial start message.
- Provider IDs resolve readable names from current per-file Title proofs and Job
  paths, rather than depending on the old single-path observation.
- Current machine bundles require `cluster.json`. An absent `secret.json` never
  reconstructs credentials from a stale bootstrap config endpoint.
- TUI navigation preserves completed operation messages in help while showing
  row/scroll position; report status is independent of the underlying inventory.
  Verifying remains truthful during both installation and recovery.
- Daemon help no longer offers ignored Home socket/upstream flags; its identity
  and error output use the same raw JSON convention as the operator CLI.
- The destination-watermark fixture sets its reserve between the two filesystem
  capacities, avoiding failures when unrelated test cleanup changes free space.

## Verification results

`make test`, `make fmt-check`, and `make clippy` passed. Clippy reports existing
workspace warnings. Reindex integration coverage exercises raw/human output,
periodic progress during a stalled API, drift refusal and retained maintenance
lock. TUI tests cover all seven screens and preserve mutation guards. Local
terminal checks covered a 100×24 CLI with NO_COLOR and a 60×16 TUI, including
navigation, detail, help, result ordering and clean exit. No live seedbox or GPU
operation was used.

## Suggested Review Order

- Start with command routing, one output format, and operation feedback.
  [main.rs:580](../bins/mediaops/src/main.rs#L580)
- Follow reindex from maintenance lock through proof publication and its final result.
  [library.rs:142](../bins/mediaops/src/library.rs#L142)
  [home_library.rs:323](../bins/mediaops/src/home_library.rs#L323)
- Inspect periodic feedback that continues during blocking filesystem work.
  [progress.rs:21](../bins/mediaops/src/progress.rs#L21)
- Review operator status, title names, and TUI pause/readiness information.
  [api_cmd.rs:276](../bins/mediaops/src/api_cmd.rs#L276)
  [overview.rs:8](../bins/mediaops-tui/src/projection/overview.rs#L8)
- Check externally observable behavior and the current command documentation.
  [reindex_progress.rs](../bins/mediaops/tests/reindex_progress.rs)
  [operator_feedback.rs](../bins/mediaops-tui/tests/operator_feedback.rs)
  [Usage](usage.md)
