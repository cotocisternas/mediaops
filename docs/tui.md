# Home TUI

`mediaops-tui` is an additive Home API viewer with scoped Want/Hold actions.
It is not a `mediaops` subcommand.
The CLI, its help, formatters, exact-screen tests, `-o json`, `--json`, and exit
codes are unchanged.

```bash
mediaops-tui [--api-socket PATH] [--color auto|always|never]
```

`--help` and `--version` never need a socket or a TTY. Interactive use requires
a terminal on stdin and stdout. `TERM=dumb` or redirection exits 2 with no
escape sequences.

The TUI talks to the Home API over its unix socket as `Actor::Cli`. API absence
shows `reconnecting`. It never falls back to `state.db`.

## Screens

| Key | Screen |
| --- | --- |
| 1 | Overview — open work, non-installed Jobs, failures, disk |
| 2 | Wants |
| 3 | Jobs — phase, bytes, attempts, binding, failure |
| 4 | open Holds |
| 5 | Titles / why facts |
| 6 | Nodes / readiness |
| 7 | Box / current RemoteFiles |

Holds and Titles show readable names and years from existing placement metadata
and library/Job/listing paths. Exact TitleIds and release-object names remain in
detail, and actions still target those IDs, never the display label. If no name
metadata is available, a readable key label or the original ID is shown instead
of guessing a name or making an external metadata request.

Tab / Shift-Tab cycle screens. Arrows and `j`/`k`, PageUp/Down, Home/End move
rows. In detail, those keys scroll facts instead. Enter opens detail; Esc goes
back. `?` is read-only help. `q`, Ctrl-C, and
SIGTERM exit 0 after restoring the terminal.

## Mutations

Only in selected detail, and only while the stream is `Current`, the terminal is
at least 60×16, and the identity is not clipped:

| Key | Action |
| --- | --- |
| W | apply or reapply one Want (`title_id` only), from Wants or Titles |
| D | delete one Want, from Wants |
| A | approve one exact release, from Holds |
| X | reject one exact release, from Holds |

Enter never writes. Approve records a decision; it does not install. Want apply
is not `mediaops watch` (that also writes Title). Hold A/X target the selected
release, not merely its TitleId.

Writes check the displayed UID and resourceVersion against a fresh read and
submit once. Conflicts refresh rather than retry; an uncertain response is shown
as `outcome unknown`. Navigation and quit remain available during requests.
An ended or expired subscription is `NOT CURRENT` and disables all writes until
a new baseline and subscription succeed. Failed reads never mean empty state.

There is no Reconcile, Job bind/cancel, Cluster/Secret/Title/Node write, Hold
Apply, select-all, confirmation prompt, or auto-approve.

## Empty states

Known-empty baselines reuse the CLI English strings: `nothing happening`,
`nothing on hold`, `nothing on the box`. A missing or stale inventory listing is
`unavailable`, never those strings.

## Accessibility

Meaning survives `NO_COLOR` and `--color never` via text, reverse, and bold.
The TUI cannot offer a screen reader. Use the CLI (`mediaops status`, `why`,
`hold`) as the unchanged alternative.

See [tui-qa.md](tui-qa.md) for fixture and PTY checks. Design tokens live in
[`DESIGN.md`](../DESIGN.md).
