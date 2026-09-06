# Contributing

Build and test on the machine you already use. Do not put a live seedbox, a real pull, or NVENC in CI.

```bash
make fetch
make test
make proto
make fmt
make clippy
```

`make test OFFLINE=1` after a fetch. `make proto` needs [Buf](https://buf.build/docs/installation). `make musl` needs `musl-gcc` and is what CI runs for the static daemon.

Read [docs/development.md](docs/development.md) and [AGENTS.md](AGENTS.md) before changing crate boundaries, PathSchema, or either JSON contract.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`. Imperative, lowercase after the type.

## Pull requests

- Keep two output contracts stable unless the change needs a new field: Home API `-o json` is the raw object; legacy `--json` is one `{ok,data,error}` envelope.
- Human stdout is the operator UI. Add or update exact-screen tests when you change a formatter.
- Do not add a TUI, prompts, or auto-approve.

By contributing you agree the work is licensed under the MIT License (see [LICENSE](LICENSE)).
