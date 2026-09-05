# Contributing

Build and test on the machine you already use. Do not put a live seedbox, a real pull, or NVENC in CI.

```bash
make fetch
make test
make fmt
make clippy
```

`make test OFFLINE=1` after a fetch. `make musl` needs `musl-gcc` and is what CI runs for the static daemon.

## Commits

[Conventional Commits](https://www.conventionalcommits.org/): `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`. Imperative, lowercase after the type.

Story work puts the story key in the subject as `N-M` (hyphen, e.g. `2-1`) so `git_evidence.py --stories` can attribute. Do not rewrite published commits to backfill old subjects.

## Pull requests

- Keep `--json` envelopes and plan JSON stable unless the change needs a new field.
- Human stdout is the operator UI. Add or update exact-screen tests when you change a formatter.
- Do not add a TUI, prompts, or auto-approve.

By contributing you agree the work is licensed under the MIT License (see [LICENSE](LICENSE)).
