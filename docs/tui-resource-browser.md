---
title: 'TUI resource browser'
type: 'feature'
created: '2026-09-06'
status: 'done'
baseline_commit: '86e7937845c7f1a52560c7cd2599d57a9d4b3295'
review_loop_iteration: 0
context: ['{project-root}/AGENTS.md', '{project-root}/docs/tui.md']
---

## Intent

The operator expects a clearer, k9s-inspired TUI. The current flat ledger hides
resource navigation, gives little indication of focus, and cannot filter lists.
Deliver one coherent resource browser with visible navigation, framed content,
contextual keyboard hints, live filtering, and resource commands.

## Boundaries & Constraints

Retain the Home API unix socket, Actor::Cli, exact object identity, UID/version
checks, stale-state behavior, scoped mutations, sync key release guard, and
60×16 minimum. No new dependencies, backend operations, CLI prompts, or approval
flows. Enter never writes. Colon commands navigate only, plus help and quit;
they never invoke sync or mutations. Use current docs, not historical artifacts.
This scope supersedes DESIGN.md's flat ASCII-only ledger and closed navigation
key set. Keep terminal theme background and named semantic colors, monochrome
reverse/bold, Unicode cell-width handling, and sanitized untrusted text.

## Code Map

- `bins/mediaops-tui/src/view.rs`, `view_chrome.rs`, `view_text.rs`: current
  shell, table, detail, status/footer and help rendering; column widths adapt.
- `model.rs`, `keys.rs`, `update.rs`: navigation, mutation guards, event decoding.
- `interaction.rs`: `project_ui` reconciles selection by key/UID and computes
  identity visibility; `can_submit` validates prepared writes again.
- `projection/mod.rs`: `project` returns rows and selected detail together;
  filtering must map the selected filtered row back to its original source index.
- `runtime.rs`: event path currently counts unfiltered rows; synchronize it
  with the displayed projection. Geometry currently repeats `rows - 5` and
  `width - 15` across rendering, scrolling, and safety checks.
- `tests/operator_feedback.rs`, `render.rs`, `render_chrome.rs`,
  `interaction_safety.rs`, `sync_key_safety.rs`: screen contracts and safety.
- `DESIGN.md`, `docs/tui.md`, `docs/tui-qa.md`: current operator/design docs.

## Tasks & Acceptance

- [x] Introduce shared shell/pane geometry in `bins/mediaops-tui/src/` and use
  it for drawing, paging, report/help wrapping, detail scroll, and identity
  visibility. Account for borders, gutters, and the pinned Hold caption.
- [x] Rework `view.rs`/`view_chrome.rs`: compact brand/connection/disk header,
  visible seven resource tabs with active emphasis, compact contextual shortcuts,
  framed resource table with count and selected-row marker, labeled detail pane
  whose focus is clear, and fixed status/footer. Use thin terminal borders and
  restrained cyan emphasis; keep failures and readiness semantic. At >=120
  columns retain live detail preview; Enter focuses it, Esc returns to list.
  At narrower sizes Enter replaces the list with detail. Keep readable TITLE,
  complete PHASE and PROGRESS at 60 columns. Do not add invented metrics.
- [x] Add `/` live case-insensitive literal filtering across displayed cells
  and exact identity in `model.rs`, `keys.rs`, `update.rs`, and projection.
  Enter keeps the filter; Esc cancels editing; Esc in list clears an applied
  filter. Show query, matching count, and a distinct no-match state. A filter
  yielding zero rows cannot make unavailable or stale data look known-empty.
- [x] Add `:` resource navigation with singular/plural aliases (overview,
  want(s), job(s), hold(s), title(s), node(s), box), help and quit. Render
  discoverable aliases while typing; unknown commands give useful feedback.
  Add `g`/`G` for first/last and `d` for detail alongside existing keys.
- [x] Isolate text entry before global key interpretation. Letters including
  q/W/D/A/X/p/S, digits, punctuation, and navigation letters are text in input
  mode; Ctrl-C still exits. Ignore paste/repeat/release for edits and writes;
  preserve genuine sync-release processing. Backspace edits, Ctrl-U clears.
  Disable writes while editing and invalidate captured action targets when
  input/filter/navigation changes. In-flight preparation must not submit after
  navigation or opening input. Preserve filtered selection by key/UID across
  updates, and clear action eligibility if the object disappears or is replaced.
- [x] Update meaningful rendering and navigation tests, including exact minimum
  screen, colored and monochrome focus, Unicode input, filtered detail/target
  agreement, no match, unavailable/stale state, and mutation input isolation.
- [x] Rewrite `DESIGN.md` around the final browser, update `docs/tui.md` and
  `docs/tui-qa.md` for navigation and checks. Run targeted tests and formatting.

Acceptance criteria:

- Given a 60×16, 80×24, or 140×40 terminal, when opening any resource, then its
  active tab, content, focus, connection state, help, and quit remain discoverable.
- Given several similarly named objects, when filtering and opening a result,
  then the displayed detail and guarded action target identify that exact row.
- Given input mode, when typing action keys or pressing Enter, then no mutation
  or sync request is queued; commands can only navigate, show help, or quit.
- Given disconnect, resize, or replacement of the selected object, when actions
  are attempted, then the existing connection and identity gates still apply.
- Given long detail/help/report content, when using End then Up after resizing,
  then scrolling remains usable and line counts match the actual pane geometry.

## Design Notes

Prefer three compact header rows (brand/status/disk, contextual navigation hints,
resource tabs), a flexible framed body, and status/footer. Reserve full labels
for action hints; shorten secondary navigation hints before hiding help or quit.
Use a visible `>` marker plus reverse selection to survive monochrome. Detail
labels can be muted and values emphasized; identity stays first. Avoid decorative
logos that consume working space. A filter is local to the active resource;
switching resources resets it. Input is a single-line local navigation editor,
not a shell or an approval prompt. Reuse existing helpers where possible.

## Verification

- `cargo test -p mediaops-tui --locked`: TUI and safety regression tests pass.
- `make fmt-check`: formatting passes.
- `make test`: root runs the full default gate after implementation.
- Root exercises a dedicated local fixture in a real PTY, including resize,
  filters, commands, detail, help, disconnect, quit and terminal restoration.
  No live seedbox, SSH, GPU, or production state is used for these checks.

## Spec Change Log

- Review: reconcile filtered selection between consecutive key events without
  granting an action target before drawing. Cover commands and paging with
  independent expected values and preserve scrolled viewports on cancellation.
- Review: reserve the complete Hold caption at the 120-column split boundary;
  keep filter scope, row position, and input focus visible. Preserve completed
  reports and notifications when they arrive during input.
- Review: normalize Unicode case consistently for final sigma and reuse built
  rows for selected detail instead of rebuilding the complete projection.

## Verification Results

- `make test`: 877 tests passed, including 112 TUI tests and the architecture
  checks; required sibling binaries were built first.
- `make fmt-check`, `git diff --check`, and `make clippy` passed. Existing
  workspace Clippy warnings remain outside the TUI; no TUI warnings were emitted.
- Real PTY: resource commands, rapid filtering and exact detail, Unicode,
  input/action isolation, no-match states, list/detail/help/report navigation,
  60/80/100/120/140-column layouts, undersize editor quit, monochrome,
  disconnected action blocking and automatic reconnection passed.
- Real PTY: preview completion during input, persistent scope and pane focus,
  full Hold caption at 120×16, and terminal restoration on quit, Ctrl-C,
  SIGTERM, error and panic passed. Read-only navigation left fixture decisions
  and work unchanged. The dedicated local fixture and terminal were stopped.
- Independent review findings were patched and covered by regression checks.
  No live seedbox, SSH, NVENC or production state was used for verification.

## Suggested Review Order

**Browser layout**

- Framed resources and preview/focus behavior share one shell.
  [view.rs:15](../bins/mediaops-tui/src/view.rs#L15)

- Borders, split widths, and page sizes use the same cell geometry.
  [geometry.rs:50](../bins/mediaops-tui/src/geometry.rs#L50)

**Input and identity**

- Production event dispatch reconciles selection without authorizing an undrawn target.
  [interaction.rs:94](../bins/mediaops-tui/src/interaction.rs#L94)

- Editors consume typed shortcuts before global commands.
  [keys.rs:38](../bins/mediaops-tui/src/keys.rs#L38)

- Filtered rows retain exact keys and reuse selected detail.
  [mod.rs:73](../bins/mediaops-tui/src/projection/mod.rs#L73)

**Asynchronous feedback**

- Completed reports wait behind editors; unseen outcomes remain available.
  [update.rs:315](../bins/mediaops-tui/src/update.rs#L315)

**Verification**

- Fast input, action targets, paging, and asynchronous UI transitions have regression coverage.
  [resource_browser.rs:744](../bins/mediaops-tui/tests/resource_browser.rs#L744)

- The smallest supported screen retains titles, phase, percent, and controls.
  [operator_feedback.rs:65](../bins/mediaops-tui/tests/operator_feedback.rs#L65)
