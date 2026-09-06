# Home TUI QA

Headless `TestBackend` tests run in `make test`. They are not a substitute for
a real PTY.

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

- [ ] seven screens, Tab / 1–7, j/k, Enter/Esc, `?`, `q`
- [ ] W/D on a Want detail; A/X on one Hold when two share a TitleId
- [ ] missing socket shows reconnecting, not a local DB
- [ ] kill the fixture: `NOT CURRENT`, mutations off, then restart: Current
- [ ] wait past inventory freshness: Holds/Box become unavailable
- [ ] resize below 60×16: notice, mutations off; restore size
- [ ] `--color never` and `NO_COLOR`: reverse/bold still mark focus and stale
- [ ] Unicode title clips on cell width, not byte length
- [ ] redirected stdin/stdout or `TERM=dumb`: exit 2, no escapes
- [ ] Ctrl-C / SIGTERM / panic restore cursor, paste, and alternate screen

Never claim this checklist from snapshots alone.
