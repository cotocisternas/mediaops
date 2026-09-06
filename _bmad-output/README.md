# Historical BMAD output

**Status: superseded by the Home API rewrite. Not an active implementation plan.**

The existing planning, implementation, review, and retrospective artifacts in
this directory document the pre-rewrite product. They are retained in place to
preserve links and historical evidence, not rewritten to pretend they specified
the new architecture.

Start with the [current documentation index](../docs/README.md) and
[documentation status](../docs/documentation-status.md). For agent instructions,
use the root [AGENTS.md](../AGENTS.md), not an archived project-context file.

## Archive map

| Material | Historical use | Current replacement |
| -------- | -------------- | ------------------- |
| [Brainstorming](brainstorming/) and [discussion records](party-mode/) | Original product intent and tradeoffs | Current scope in [Architecture](../docs/architecture.md) |
| [Specification package](specs/spec-mediaops/SPEC.md) and its companions | Pre-rewrite contract and module map | [Architecture](../docs/architecture.md), [Config](../docs/config.md), and [Usage](../docs/usage.md) |
| [Planning artifacts](planning-artifacts/) | Old architecture, work breakdown, readiness assessment | [Documentation status](../docs/documentation-status.md) and a freshly scoped plan for new work |
| [Implementation records](implementation-artifacts/) | Old specifications, handoffs, reviews, and retrospectives | Current source, tests, and [Development](../docs/development.md) |
| [Sprint status](implementation-artifacts/sprint-status.yaml) and [story queue](specs/spec-mediaops/stories.yaml) | Progress at the time of the old delivery | Not a current queue; establish new work explicitly |
| [Deferred findings](implementation-artifacts/deferred-work.md) | Leads from earlier reviews | Reproduce and triage against current code before scheduling |

## How to use this history

- Read it to understand past requirements, decisions, and results.
- Revalidate any still-relevant requirement against current docs and code before
  carrying it into new work.
- Do not execute old demo or deployment commands, resume old story files, or use
  historical sprint/readiness labels as the current backlog.
- Do not interpret old test counts, coverage, or review verdicts as verification
  of the Home API rewrite.

For new BMAD-assisted work, establish a fresh scope from current documentation
and keep its status distinct from these historical records. The installed BMAD
tooling itself is not retired by this notice.

The installer-managed BMAD configuration still points into this tree, and a local
loop configuration may still select its status file. These notices do not disable
that software. Do not launch automated work against the historical queues; select
an explicitly current scope and output location first. Old pending/review labels
are preserved, not requests to finish the previous architecture.
