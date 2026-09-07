# mediaops-tui Design System

A keyboard-driven resource browser for the home media control plane. Inspired
by k9s: keep resource navigation visible, make focus explicit, and let the
operator filter a table before inspecting an object. Use the terminal's own
background, compact working space, and semantic status colors.

## Shell

```text
mediaops  Current  |  disk 693 GiB free 2s
Tab resource  j/k rows  g/G first/last  d detail
1 Overview 2 Wants 3 Jobs 4 Holds 5 Titles 6 Nodes 7 Box
┌ Jobs (2) [list] ────────────────────────────────────────┐
│  TITLE                       PHASE     PROGRESS BYTES   │
│> The Matrix (1999)            pulling        30%   1 GiB │
│  Fight Club (1999)            failed          0%     0 B │
│                                                        │
└────────────────────────────────────────────────────────┘
row 1 of 2  |  Enter detail
Enter detail  /  :  p preview  S sync  ? help  q quit
```

The sketch illustrates hierarchy; exact 60×16 output is fixed in
`bins/mediaops-tui/tests/operator_feedback.rs`.

| Region | Height | Purpose |
| --- | --- | --- |
| Header | 1 row | Application, connection state, observed free disk space and age |
| Context keys | 1 row | Navigation, detail actions, or input suggestions |
| Resources | 1 row | All seven numbered tabs; current resource in reverse/bold |
| Body | Remaining rows | Framed list, detail, help, or sync report |
| Status/input | 1 row | Position, operation status, error, filter, or command editor |
| Footer | 1 row | Available actions, navigation, help, and quit |

A thin Unicode border labels each content pane. The active pane has cyan/bold
borders; inactive panes are muted. The list title includes the resource name,
count, and `[list]` when focused. Detail is `[preview]` until Enter or `d`
focuses it; then its label reads `[focus]`.

| Terminal | Body layout |
| --- | --- |
| At least 120 columns and 16 rows | Table on the left, live detail preview on the right; Enter focuses detail |
| 60–119 columns and at least 16 rows | One pane; Enter replaces the list with detail; Esc returns |
| Below 60 columns or 16 rows | Resize notice and quit; actions disabled |

`bins/mediaops-tui/src/geometry.rs` owns shell and pane dimensions. Renderers,
page navigation, help/report wrapping, detail scrolling, and identity visibility
use that same geometry, including borders and the pinned Hold caption. At 60×16,
the table has eight data rows beneath its header. Help and reports own the whole
body; opening one replaces both panes on wide terminals.

## Tables and details

- Mark the selected row with `> ` and reverse video. Color adds cyan/bold when
  the list has focus. Keep selection visible when scrolling or resizing.
- Resolve readable titles and years from existing Home API metadata. Exact
  object keys, UIDs and resource versions determine selection and actions.
- Give TITLE at least 24 cells when several columns fit. Drop trailing columns
  before squeezing titles. Jobs retain TITLE, PHASE and PROGRESS at 60 columns.
- Right-align numeric BYTES, ATTEMPTS, SIZE, AGE and PROGRESS cells.
- Clip table text with `…` using terminal cell width. Detail values wrap at word
  boundaries where possible; long IDs and paths wrap across lines.
- Keep exact identity first in detail. Disable mutations when the selected
  identity cannot be fully seen or detail has scrolled away from the top.
- Detail labels are muted; values use normal foreground. Long messages and
  reasons remain scrollable. Never infer throughput or ETA from byte counts.
- Pin `Approve records a decision; it does not install.` below Hold facts,
  including during scrolling.

## Keyboard behavior

| Key | Behavior |
| --- | --- |
| `1`–`7`, Tab / Shift-Tab | Change resource, clear its filter, return to list |
| Arrows, `j` / `k` | Move rows; scroll when detail/help/report has focus |
| PageUp / PageDown | Move by the visible page |
| Home / End, `g` / `G` | First or last row/page |
| Enter, `d` | Focus selected detail; never write |
| Esc | Dismiss input, help, or report; return from detail; clear filter in list |
| `/` | Edit a live, case-insensitive literal filter |
| `:` | Enter a resource command |
| `?` | Scrollable help, including full status and service/log inspection commands |
| `q`, Ctrl-C, SIGTERM | Exit and restore the terminal |

Filtering matches displayed cells and exact identity. It is local to the active
resource. Enter keeps the filter; Esc while editing restores the previous filter.
The query stays visible in the pane frame, including narrow detail. Lists retain
matching counts and row position. No results show
`no matching resources`; a failed or stale read does not become a known-empty list.

Colon commands accept `overview`, `want`/`wants`, `job`/`jobs`, `hold`/`holds`,
`title`/`titles`, `node`/`nodes`, `box`, `help`, and `q`/`quit`. Enter navigates;
the context row suggests matching names. Unknown commands show a useful error.
This is a local navigation editor: commands never invoke a shell or write.

During input, printable keys are text, including digits, `q`, `?`, `W`, `D`,
`A`, `X`, `p`, and `S`. Backspace edits, Ctrl-U clears, and Ctrl-C exits.
Paste and repeat events do not edit or submit. Sync release events still reach
the held-key guard. Opening input invalidates any captured action target.

## Actions and status

Show scoped mutation hints in the context row only when enabled: `W apply` and
`D delete` for Want detail, `W apply` for Title detail, and `A approve` /
`X reject` for Hold detail. Sync preview `p` and one-shot sync `S` remain global
shortcuts when enabled. Shorten secondary navigation labels before hiding help,
quit, or the meaning of an action.

Actions require Current state, a sufficient terminal size, no active editor or
help, and no pending request. Object actions additionally require focused detail
and visible identity. Keep all fresh-read UID/version checks and do not resubmit
conflicts or uncertain outcomes. Each Hold action identifies one exact release.
There is no confirmation prompt, automatic approval, or command alias for writes.

The status row reports row/line position or the pending operation and elapsed
time. Long statuses point to `?` for the complete text. Navigation retains a
completed message in help as `Last status`. Reconnection shows its retry countdown
and disables actions until a fresh baseline and subscription succeed.
Completions received during input remain available when the editor closes;
keystrokes cannot archive an outcome before it has been displayed.

A sync report includes request ID, captured generation, copy/reuse/present/
blocked/ineligible counts, and wrapped source/destination/reason rows. Esc returns.
Reports arriving during input wait until the editor closes.
A copied percentage of 100% still requires verification and installation; phase
text makes that distinction visible.

## Color and accessibility

| Role | Treatment |
| --- | --- |
| Background / body | Terminal defaults; never fill the canvas |
| Active tab, pane, selected row | Cyan and bold; reverse tab/row |
| Ready, Current, Installed, Satisfied | Green plus the status word |
| Not ready, NOT CURRENT, reconnecting, pauses | Yellow plus the status word |
| Failed, refused, missing, undersize | Red plus the status word |
| Secondary hints, inactive borders, labels, disk age | Default foreground with dim in color mode |

`NO_COLOR` and `--color never` preserve meaning through text, reverse and bold.
Terminal themes own color contrast. Use width-aware Unicode clipping and sanitize
untrusted content before drawing. Do not add animation, blinking, a mouse
requirement, external metadata requests, or fabricated operational metrics.

The CLI remains the text alternative (`mediaops status`, `why`, `hold`, `sync`).
This TUI makes no screen-reader guarantee. Request keyboard event-type reporting
when supported; a held `S` must not enqueue repeated sync requests. Without
reliable release reporting, allow one `S` per launch and direct further requests
to the CLI or a new TUI session.

Known-empty baselines retain `nothing happening`, `nothing on hold`, and
`nothing on the box`. Unavailable data says `unavailable`. Non-current empty
screens show the connection state. Color never substitutes for those words.

See [TUI usage](docs/tui.md) and [QA](docs/tui-qa.md) for operator behavior and
verification. TestBackend checks cover screens and styles; real PTY checks cover
key input, resizing, disconnect/reconnect, and terminal restoration.
