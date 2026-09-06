# Documentation status

The Home API rewrite replaces the old CLI plan/run loop with an always-on home
control plane. Use the current documentation below for operation and development.
Historical specifications explain past decisions; they do not define the current
architecture or establish that a current release has passed verification.

## Current sources of truth

| Need | Read |
| ---- | ---- |
| Process roles, data ownership, protocols, and safety boundaries | [Architecture](architecture.md) |
| Installation and replacement of retired home units | [Setup](setup.md) |
| Commands, output formats, and recovery | [Usage](usage.md) |
| Runtime settings, paths, identities, and file layout | [Config](config.md) |
| Build and verification commands | [Development](development.md), the [Makefile](../Makefile), and [CI](../.github/workflows/ci.yml) |
| Rules for agents working in this repository | [AGENTS.md](../AGENTS.md) |

Check executable behavior against the source and `--help` when updating these
pages. Runtime configuration is the Home API Cluster object, not a planning
document or a stale copy of `config.toml`.

## What the rewrite supersedes

| Previous design | Current design |
| --------------- | -------------- |
| CLI `plan` / `run` and a periodic run timer | Wants and per-file Pull Jobs, reconciled by the always-on `mediaops-home` roles |
| `mediaopsd --role home` | Separate `mediaops-api` and `mediaops-gateway` processes |
| `state.db` as the home catalog | `api.db`, opened only by `mediaops-api`; `state.db` remains for supported legacy capabilities |
| `config.toml` as the copy loop's live configuration | Cluster settings imported into the API and snapshotted when each Job is created |
| One JSON envelope for every command | Raw Home API `-o json` output and the separate legacy `--json` envelope |
| One title-wide copy result | Installation and current digests for individual episode, track, or movie placements |
| A library flock coordinating the normal copy loop | Job bind and status for unattended copying; explicit CLI maintenance still has its own locking rules |

Use [Setup](setup.md#3-bootstrap-the-home-library) for the actual upgrade steps.
Do not run commands copied from old demos to migrate an installation.

## BMAD artifacts

The pre-rewrite generated material is retained under
[`_bmad-output/`](../_bmad-output/README.md) for historical reference. Its old
architecture, project context, implementation instructions, demo commands, and
sprint status are superseded. Old `done` or readiness labels describe that old
delivery, not the Home API rewrite or a current backlog.

Not every idea in that material is invalid. Requirements such as home being the
library of record, optional *arr integration, Range-only copying, and explicit
hold approval still apply where the current docs and code retain them. Historical
reviews and test reports remain useful evidence of what was checked at the time,
not proof about today's implementation.

The installed BMAD workflows and skills are tooling, not obsolete product
specifications. Keep them separate from generated project artifacts. Before using
a BMAD workflow for new work, ground it in the current docs and create an explicit
new scope; do not resume a pre-rewrite story or sprint merely because a workflow
discovers its status file.

BMAD's configured output paths still point into `_bmad-output/`. Archival notices
do not disable a loop or rewrite its local policy. Select a fresh scope and output
location explicitly before using automated planning or implementation discovery.
