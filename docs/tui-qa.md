# Home TUI QA

Headless `TestBackend` tests run in `make test`. They are not a substitute for
a real PTY.

`tests/operator_feedback.rs` fixes the complete 60×16 Jobs screen and checks
verification versus byte completion, scheduling pauses, absent workers,
heartbeat age, inventory failure guidance and scrollable connection errors.

`tests/resource_browser.rs` covers filtering, resource aliases, editor isolation,
filtered identity and mutation targets, live replacement, pane focus in color
and monochrome, Unicode input, and shared scroll/identity geometry.

## Local fixture

```bash
cargo run -p mediaops-apiserver --example tui_fixture --locked --offline -- \
  /tmp/opencode/mediaops-tui-qa-UNIQUE rich
```

Prints the `api.sock` path. Modes: `rich`, `empty`, `not-ready`. The fixture
heartbeats all three Nodes locally and does not start pull/scheduler/inventory
workers, SSH, or a seedbox.

Use a dedicated scratch directory. Store recordings outside it: restart refuses
unrecognized files in the fixture directory. Restarting the same directory keeps
its existing objects and decisions. To exercise freshness expiry, stop `rich`
and restart that same directory in `not-ready` mode: no new heartbeats are sent,
so after 30 seconds the connected TUI marks the listing unavailable.

```bash
mediaops-tui --api-socket /tmp/opencode/mediaops-tui-qa-UNIQUE/api.sock
```

## Restoration probe

```bash
cargo run -p mediaops-tui --example terminal_probe --locked --offline -- normal
cargo run -p mediaops-tui --example terminal_probe --locked --offline -- error
cargo run -p mediaops-tui --example terminal_probe --locked --offline -- panic
```

Must return the terminal to cooked mode after draw (and after a panic).

## Manual PTY checklist

Drive a real terminal (tmux is fine for keys; do not treat `tmux capture-pane`
as truecolor evidence):

- [ ] seven visible tabs, Tab / 1–7, j/k, g/G, Enter/d/Esc, `?`, `q`
- [ ] `:jobs` and singular aliases navigate; `:help` opens help; `:q` quits; an unknown command explains the available commands
- [ ] `/matrix` filters readable titles and exact IDs; Enter keeps it; Esc in the list clears it; Esc during editing restores the prior filter
- [ ] filtered detail and selected action identify the same exact object, including two Holds sharing one title
- [ ] typing `qWDXASp123?gG` in either editor remains text; Enter cannot write; Backspace, Ctrl-U, Unicode, and cancellation work
- [ ] no matches is distinct from known-empty or unavailable; the active filter remains visible
- [ ] at 120+ columns the detail preview follows selection; Enter focuses detail, and Esc restores list navigation
- [ ] exactly 120×16: the complete Hold caption fits; PageDown/Up advances by the visible rows or fact lines
- [ ] type a filter and press Enter twice quickly: detail remains on the filtered object even before a redraw
- [ ] cancel a resource command after scrolling: the list viewport stays in place
- [ ] start a preview and immediately open `/`: keep filtering the visible list; receive the completed report after leaving the editor
- [ ] completion messages arriving during command input remain readable after it closes
- [ ] Jobs show readable titles, complete phase names and percentages at 60 columns; detail separates copied bytes from verification/installation
- [ ] Overview exposes scheduling/encode pauses; missing or expired workers have an inspection command
- [ ] row/line position follows navigation; long errors remain readable via `?`, End, Up, Home without changing the selected object
- [ ] pending requests show operation and elapsed time; disconnects show retry countdown
- [ ] W/D on a Want detail; A/X on one Hold when two share a TitleId
- [ ] `p` produces a completed preview after the next fixture inventory; no Sync or copy objects are created
- [ ] `S` submits a fresh one-shot request; its report names a durable Sync, and repeated requests reuse existing Jobs
- [ ] Preview/report source paths, destinations and reasons wrap and scroll at 60 columns; End then Up remains usable
- [ ] missing socket shows reconnecting, not a local DB
- [ ] kill the fixture: `NOT CURRENT`, mutations off, then restart: Current
- [ ] wait past inventory freshness: Holds/Box become unavailable
- [ ] resize below 60×16 from list, detail and input: notice, mutations off, advertised quit works; restore size
- [ ] `--color never` and `NO_COLOR`: reverse/bold still mark focus and stale
- [ ] Unicode title clips on cell width, not byte length
- [ ] redirected stdin/stdout or `TERM=dumb`: exit 2, no escapes
- [ ] Ctrl-C / SIGTERM / panic restore cursor, paste, and alternate screen

Never claim this checklist from snapshots alone.
