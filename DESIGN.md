# mediaops-tui Design System

Reading this as: operator TUI for a home media control plane, with an
editorial operations-ledger language, leaning toward Linear's restrained
chrome translated into terminal-native ASCII.

Dials: `DESIGN_VARIANCE: 3` / `MOTION_INTENSITY: 1` / `VISUAL_DENSITY: 8`.

## 0. Research Log

- Embedded refs: shortlisted linear.app (editorial density, aligned columns,
  reserved accent), notion (quiet chrome), vercel (ops status) → picked
  `taste-skill` (operational) + `linear.app` because the spec names an
  editorial operations ledger and Linear's luminance hierarchy maps to
  terminal reverse/bold without inventing a second accent. Brand indigo is
  **not** used; the spec's cyan focus replaces it.
- Lazyweb: skipped — spec marks browser/image lanes inapplicable for a
  terminal surface.
- Imagen drafts: skipped — spec marks image lanes inapplicable; wireframes
  below are the reference-fidelity contract.
- Lighthouse / React / browser QA: skipped — spec marks those lanes
  inapplicable. Verification is TestBackend + real PTY.
- layout-skill: loaded for list-detail / scroll-body-shell ownership.

## 1. Atmosphere & Identity

A quiet operations ledger on the operator's default terminal background.
Content is the surface: ASCII rules, aligned numbers, one cyan focus row.
No nested boxes, no animation, no invented throughput. The signature is
the masthead plus a dominant table — status is a word in the header, never
a spinner. Linear's "darkness as space" becomes "default background as
space": we do not paint a canvas; we let the terminal be the canvas and
mark only focus, success, stale, and failure.

## 2. Color

Terminal named colors only. Default background is never overwritten.
`NO_COLOR` and `--color never` keep meaning via text, reverse, and bold.

### Palette

| Role | Token | Color | Usage |
|------|-------|-------|-------|
| Surface | `--surface-default` | Default | Entire canvas; never fill |
| Text/primary | `--text-primary` | Default fg | Body, table cells |
| Text/muted | `--text-muted` | Default fg + dim if color | Captions, ages, help |
| Focus | `--accent-focus` | Cyan | Selected row, active screen digit |
| Success | `--status-success` | Green | Ready, Current, Approved, Installed |
| Stale | `--status-stale` | Yellow | NOT CURRENT, reconnecting, observation age |
| Failure | `--status-error` | Red | Failed jobs, write errors, undersize |
| Emphasis | `--text-emphasis` | Default + bold | Masthead title, column headers |

### Rules

- Accent is cyan and only for the focused row / active screen key.
- Green / yellow / red are status only, never decoration.
- Monochrome: selected row is reverse; Current/Ready stay as words;
  NOT CURRENT / Failed stay as words plus bold.
- Never introduce a color not in this table.

## 3. Typography

Holds and Titles use human-readable names and years in their TITLE cells.
Exact object IDs stay first in detail and continue to determine selection and
mutations; readable names are display-only. Resolve labels from Home API
placement/path metadata, with explicit ID fallback when no name is known.

Terminal-native monospace. Unicode content is width-aware (`unicode-width`).
Untrusted strings are sanitized of control sequences before draw.

### Scale

| Level | Token | Treatment | Usage |
|-------|-------|-----------|-------|
| Masthead | `--type-masthead` | Bold, one line | `mediaops` + screen + sync word |
| Header | `--type-header` | Bold | Column labels |
| Body | `--type-body` | Default | Table cells, detail facts |
| Caption | `--type-caption` | Default (dim if color) | Footer keys, observation age |
| Empty | `--type-empty` | Default | Exact English empty-state strings |

### Font Stack

- Primary: the terminal's monospace. No alternate family.

### Rules

- ASCII structure (rules, pipes, spaces). Unicode only in content.
- Table cells clip with `…` to the column width. Detail values wrap;
  identity fields (`name`, `title_id`, `uid`, `release_id`) are never
  clipped in detail.
- Column headers are fixed English: `KIND`, `TITLE`, `FACT`, `PHASE`,
  `BYTES`, `ATTEMPTS`, `NODE`, `FAILURE`, `SIZE`, `AGE`, `READY`, `ROOT`,
  `PATH`.
- Column widths are computed from the pane width. Never use a fixed 18-cell
  column. Drop trailing columns when TITLE would fall below 12 cells.
- Numeric columns (`BYTES`, `ATTEMPTS`, `SIZE`, `AGE`) are right-aligned.

## 4. Spacing & Layout

### Base Unit

One character cell. Horizontal padding is one space. Vertical rhythm is
blank lines, never box-drawing padding.

| Token | Value | Usage |
|-------|-------|-------|
| `--space-cell` | 1 col | Gutter between columns |
| `--space-rule` | 1 row | Hairline under masthead / above footer |
| `--space-stack` | 0 rows | No extra blank rows inside the table |

### Grid

Editorial operations ledger:

```
masthead   (1 row, fixed)
rule       (1 row, ASCII `-`)
table      (flex, scroll owner)
[detail]   (>=120 cols: right pane, independent scroll; 60-119: Enter)
rule       (1 row)
status     (1 row, fixed)  message and/or pending
footer     (1 row, fixed)
```

Breakpoints (character cells, not CSS):

| Name | Size | Layout |
|------|------|--------|
| Wide | >=120 cols, >=16 rows | list-detail split; table left, detail right |
| Narrow | 60-119 cols, >=16 rows | single pane; Enter opens detail, Esc back |
| Min | 60x16 | same as narrow; mutations off if identity clips |
| Undersize | <60 cols or <16 rows | resize notice only; quit offered; mutations off |

### Scroll ownership

- Shell: `scroll-body-shell` — masthead + footer fixed; table body scrolls.
- Wide: `list-detail` — table pane and detail pane each own independent
  vertical scroll. Arrows/j/k move the table; detail has its own offset.
- One scroll owner per pane. No nested boxes.

### Rules

- Numeric columns right-aligned (`BYTES`, `ATTEMPTS`, `SIZE`, `AGE`).
- Title / name columns left-aligned.
- Independent detail scroll never moves the table selection, including
  when the layout is wide split.
- Known-empty English strings render only while sync is `Current`.
  Connecting / Synchronizing / Stale paint the sync word, never
  `nothing happening` / `nothing on hold` / `nothing on the box`.

## 5. Components

### Masthead

- **Structure**: `mediaops  <screen>  <sync>  [disk]`
- **Variants**: Overview / Wants / Jobs / Holds / Titles / Nodes / Box
- **Spacing**: `--space-cell` between fields
- **States**: Connecting, Synchronizing, Current, Stale (`NOT CURRENT`)
- **Accessibility**: sync word is text, not color-only
- **Motion**: none
- **Layout**: fixed header row of the scroll-body-shell
- **Scroll owner**: none (fixed)

### Rule

- **Structure**: a line of `-` across the width
- **States**: default only
- **Motion**: none

### LedgerTable

- **Structure**: header row + N body rows; highlight via reverse + cyan
- **Variants**: wants, jobs, holds, titles, nodes, remotefiles, overview
  (`KIND` / `TITLE` / `FACT`: open wants, non-installed jobs, failures,
  worker readiness)
- **Spacing**: `--space-cell` gutters; numeric columns right-aligned
- **States**: default, selected (focus), empty (exact English string),
  unavailable (not empty), clipped identity (mutations off)
- **Accessibility**: selection is reverse even without color
- **Motion**: none
- **Layout**: table body is the scroll owner
- **Scroll owner**: table body

### DetailPane

- **Structure**: stacked facts, one per line, label + value
- **Variants**: want, job, hold, title, node, remotefile, overview
- **Spacing**: `--space-cell` after labels; labels padded to a column
- **States**: default, scrolled, identity-clipped
- **Accessibility**: facts are text; Hold caption is a reserved last row
  and stays visible while scrolled. Long `reason` / `failure` wrap.
- **Motion**: none
- **Layout**: wide = right pane; narrow = full body after Enter
- **Scroll owner**: the detail pane itself

### Footer

- **Structure**: scoped keys for this screen + global `?` `q`
- **Variants**: list; Wants detail `W apply  D delete`; Titles detail
  `W apply`; Holds detail `A approve  X reject`; never Overview / Jobs /
  Nodes / Box; help omits mutation hints
- **Spacing**: `--space-cell`
- **States**: mutations enabled / disabled (keys omitted when disabled)
- **Accessibility**: keys are letters, not color
- **Motion**: none
- **Layout**: fixed footer of the scroll-body-shell
- **Scroll owner**: none (fixed)

### HelpOverlay

- **Structure**: read-only key list replacing the table body; Esc/`?` dismiss
- **States**: default
- **Motion**: none
- **Layout**: body region; masthead + footer remain
- **Scroll owner**: help body if it overflows

### ResizeNotice

- **Structure**: `terminal too small  60x16 required` + `q quit`
- **States**: undersize only
- **Motion**: none
- **Layout**: cover (centered in the full frame)
- **Scroll owner**: none

### StatusWord

- **Structure**: `Current` | `NOT CURRENT` | `reconnecting` | `Synchronizing`
- **States**: as named; yellow/bold when stale
- **Motion**: none

Empty states reuse exact English strings only when sync is `Current` and
the baseline is known empty: `nothing happening`, `nothing on hold`,
`nothing on the box`. Unavailable and not-current never those strings.

### StatusLine

- **Structure**: `pending` and/or `ui.message`; blank when neither
- **Layout**: reserved row above the footer; survives leaving detail
- **Motion**: none

## 6. Motion & Interaction

### Timing

| Type | Duration | Easing | Usage |
|------|----------|--------|-------|
| None | 0 | n/a | Spec forbids animation and blink |

Redraw at most 10Hz on change. Local 1s tick for freshness. No hover,
no spinner, no progress bar animation.

Keys (closed set):

- Screens: `1`-`7`, Tab / Shift-Tab
- Rows: arrows, `j`/`k`, PageUp/Down, Home/End
- Detail: Enter in, Esc back
- Help: `?`
- Quit: `q` / Ctrl-C / SIGTERM
- Mutations (selected detail only): `W` apply/reapply Want, `D` delete Want,
  `A` approve Hold, `X` reject Hold
- Ignore repeat / release / pasted mutation input

## 7. Depth & Surface

Strategy: **borders-only**, ASCII. One hairline under the masthead, one
above the footer. No nested boxes. No shadows. No fill. Focus is reverse
(+ cyan when color). Linear's luminance stacking is expressed as bold vs
default vs dim, not painted surfaces.

## 8. Accessibility Constraints & Accepted Debt

### Constraints

- Meaning survives `NO_COLOR` / `--color never` via text, reverse, and bold.
- Full keyboard reachability; no mouse requirement.
- Untrusted content sanitized of CSI / OSC / C0 controls.
- Unicode width-aware clipping; CJK cells count as 2.
- Minimum 60x16; below that, resize notice, mutations off.
- CLI remains the unchanged alternative (`mediaops status` / `why` / `hold`).
- Hold caption always: `Approve records a decision; it does not install.`

### Accepted Debt

| Item | Location | Why accepted | Owner / Exit |
|------|----------|--------------|--------------|
| No screen reader / braille | whole TUI | Terminal accessibility is inherently limited; CLI is the alternative. Spec-accepted. | stays |
| No 4.5:1 contrast guarantee | named ANSI colors | Operator terminal theme owns contrast; we use default bg. Spec-accepted. | stays |
| Mutations off when identity clips | narrow/min | Prevents acting on a truncated TitleId/Hold. Spec-required. | stays |

## Wireframes

Exact-width sketches. `·` is a space. Selected row marked with reverse
implied by `>` in column 0 (implementation uses reverse, not a glyph).

### 140x40 wide — Overview (list-detail)

```
mediaops  Overview  Current                  disk  693.1 GiB free  2s
--------------------------------------------------------------------------------
KIND  TITLE                          FACT
>want  movie:key:thematrix.1999       open
 job   movie:key:thematrix.1999       pulling
 node  pull                           ready
 series:key:mrrobot.2015           Pending         -  -
--------------------------------------------------------------------------------
title_id     movie:key:thematrix.1999
phase        pulling
bytes        1.2 GiB / 6.8 GiB
attempts     1
node         pull
failure
1-7 screens  j/k rows  Enter detail  ? help  q quit
```

(Detail occupies the right ~50 columns at >=120; shown stacked here for
plain-text wrapping. Implementation splits horizontally.)

### 80x24 narrow — Holds list

```
mediaops  Holds  Current                     disk  693.1 GiB free  2s
--------------------------------------------------------------------------------
TITLE                              SIZE     AGE
>movie:tmdb:4539                   7.1 GiB  75m
 movie:tmdb:9999                   2.0 GiB  12m
--------------------------------------------------------------------------------

j/k rows  Enter detail  1-7 screens  ? help  q quit
```

### 80x24 narrow — Hold detail (after Enter)

```
mediaops  Holds  Current                     disk  693.1 GiB free  2s
--------------------------------------------------------------------------------
name         movie:tmdb:4539
title_id     movie:tmdb:4539
size         7.1 GiB
reason       Found matching movie via grab history, but release was
             matched to movie by ID. Manual Import required.
release      Hearts.of.Darkness-…WATCHABLE
generation   4
Approve records a decision; it does not install.
--------------------------------------------------------------------------------
conflict; refreshed
A approve  X reject  Esc back  ? help  q quit
```

### 60x16 min — Jobs, mutations off if identity clips

```
mediaops  Jobs  Current            disk  n/a
------------------------------------------------------------
TITLE                    PHASE
>movie:key:thematrix.1999 pulling
------------------------------------------------------------
j/k  Enter  1-7  ?  q
```

### Undersize

```
terminal too small  60x16 required
q quit
```

### Help (read-only)

```
mediaops  Overview  Current
--------------------------------------------------------------------------------
1 Overview  2 Wants  3 Jobs  4 Holds  5 Titles  6 Nodes  7 Box
Tab / Shift-Tab  next/prev screen
j k arrows  PageUp PageDown  Home End  rows
Enter  detail   Esc  back   ?  help   q  quit
W apply Want   D delete Want   A approve Hold   X reject Hold
mutations only in selected detail; Enter never writes
--------------------------------------------------------------------------------
Esc dismiss  ? help  q quit
```
