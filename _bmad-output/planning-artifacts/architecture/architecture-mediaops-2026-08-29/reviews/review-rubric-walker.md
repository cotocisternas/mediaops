# Rubric-walker review — ARCHITECTURE-SPINE.md (mediaops)

- **Reviewer:** rubric walker (good-spine checklist gate)
- **Date:** 2026-08-29
- **Subject:** `_bmad-output/planning-artifacts/architecture/architecture-mediaops-2026-08-29/ARCHITECTURE-SPINE.md`
- **Driving spec:** `_bmad-output/specs/spec-mediaops/SPEC.md` + companions (module-map, grabber-inventory, bootstrap-surfaces, failure-history-tests, increments)
- **Scope honored:** spec Constraints bind verbatim and are not restated in the spine; missing restatements were **not** flagged. Greenfield; no parent spine. Reconcile pass assumed done; this review walks the checklist only.

## Verdict

**REVISE.** The spine is a strong substrate — verified-current stack, complete CAP-1..12 coverage, clean deferrals, and most cross-crate divergence points genuinely fixed. But its central law (the AD-2 legal-edge diagram) contradicts four other ADs by omitting the `core → cli` and `core → daemon` edges those ADs require, the CI enforcement of that law checks only a subset of it, and the operational envelope (upgrade of mediaops itself, log retention, monitoring, state.db backup) is left silent rather than decided, deferred, or opened.

---

## Checklist item 1 — Does it fix the real divergence points for independently built crates, and miss none?

**Mostly yes.** The spine fixes the divergence points that matter most for parallel crate builds: crate naming and workspace shape (Conventions, Structural Seed), the wire contract and its sole conversion home (AD-3), executor topology (AD-4/AD-5), data-tier ownership (AD-6), config layout (AD-7), sqlite ownership (AD-8), the plan/apply protocol and the exhaustive `Action` enum (AD-9), job-row semantics (AD-10), the `.partial` staging format down to sidecar JSON shape (AD-11), channel-pool parallelism (AD-12), path rendering and walking (AD-13), TLS mechanics including the fingerprint-algorithm scoping (AD-14), transport/exec ports (AD-15/AD-16), exit codes (AD-17), stdout/stderr contract (AD-18), executor (AD-19), test regime (AD-20), and provider loud-failure (AD-21). Identifier, timestamp, error, diff, and migration conventions close the usual cosmetic drift channels.

**Divergences two AD-compliant crates could still hit:**

1. **Store access pattern (the strongest miss).** AD-8 says "every other crate consumes typed repository traits" and AD-10 makes `sync` and `encode` writers of `jobs` rows — but the AD-2 diagram gives them no `store` edge, and the spine never says where the repository traits live or that the binaries inject the store adapter. Builder A defines `JobsRepo` in `core` and expects composition-root injection; builder B links `store` from `sync` directly (forbidden by the diagram's prose but **not** by its CI test — see item 2). Both believe they are AD-compliant; the crates do not compose. One sentence ("repository traits live in `core`; `store` is the adapter; binaries wire them") closes it.
2. **`sync` purity split.** The layer map calls `sync` a "pure planner," while AD-4 and the Structural Seed put apply orchestration — side-effectful, consuming `PullFile` — inside `sync`. Two builders can split planner vs. apply along different lines, or one can reject the other's side-effectful code as violating "pure."
3. **Plan artifact lifecycle.** AD-9 makes the Plan a first-class JSON artifact, but no AD-6 tier owns it (runtime artifacts list only lockfile, `.partial` + sidecar, `tls/`), and nothing fixes where `plan` writes it, how long it lives, or whether a stale artifact may be applied later (the embedded config hash mitigates but is not stated as a staleness gate). The `plan`-then-later-`apply` path is real per CAP-7, so this is not purely hypothetical.

## Checklist item 2 — Is every AD's Rule enforceable, and does it prevent its stated divergence?

Walked all 21. Most Rules are concretely enforceable (enum exhaustiveness + workspace `never` rule, cassette replay, `parse(render(id)) == id` schema CI, deny_unknown_fields, one-place error→ExitCode mapping) and do prevent their stated divergence. Exceptions:

1. **AD-2's enforcement does not enforce AD-2's law (critical, combined with the diagram omission below).** The Rule states "only the edges in this diagram are legal," but the CI test checks exactly five conditions (`arr` outside daemon tree, `reqwest` outside `arr`, `arr`/`reqwest` in CLI tree, `reqwest`/`ssh` in transfer tree, banned transport crates anywhere). Unchecked but diagram-illegal and consequence-bearing: `store`/`rusqlite` inside a daemon tree (AD-8 explicitly says neither daemon role links store — no CI check), `encode` inside the daemon tree (spec forbids seedbox encode), `sync`/`encode` linking `store` (item 1.1 above), and `ffmpeg-next` (banned by AD-16, absent from AD-2's ban list even though `ssh2`/`russh` made it). The stated divergence — "crate dependency direction is law, enforced in CI" — is only partially prevented.
2. **The diagram itself is unsatisfiable as written.** Read literally, it forbids direct `core → cli` and `core → daemon` dependencies — yet AD-9 (cli matches the `Action` enum), AD-13 (daemon `Transfer` service uses the one `core` walker), AD-17 (each binary maps `core::ExitCode`), and AD-21 (Provider trait + `AlreadyThere` live in `core`, driven from the cli) all require the binaries to name `core` types, which in Cargo requires a direct dependency. A builder must either violate the diagram or invent a re-export of `core` through `proto`/`sync` that no rule sanctions — and two builders will invent it differently. Every non-binary crate has its `core` edge drawn; the omission of the two binary edges reads as an oversight, but the spine's own words currently make compliance impossible.
3. **`tests/architecture.rs` may silently not exist.** The Structural Seed places the AD-2 enforcement test at the workspace root, whose `Cargo.toml` is shown as `[workspace]` only. A virtual manifest compiles no root `tests/` target — cargo would skip the file without error, and the "law enforced in CI" would enforce nothing. It needs to be a member crate (e.g. `crates/architecture-tests`) or the root must be a package.
4. Soft but acceptable at this altitude: AD-1's "any logic a test would want lives in a library crate" is review-enforceable only; AD-12's re-probe trigger ("bind address or underlay changes") names no detector. Neither is a divergence engine.

## Checklist item 3 — Could anything under Deferred let two units diverge before its revisit condition arrives?

**No — this section is clean.** Walked all nine entries; each either freezes the present-tense law while deferring the future (reverse-connect stays a designed-unused mode of the one binary per AD-5; music-first + per-title want is law while the wants queue waits; `range_len` is a fixed desired-state value while autotune waits), or defers something no v1 unit builds against (TUI attach mechanics — the stream it attaches to is already fixed by AD-18; agent internals — CAP-11 is spec-deferred and the capability-token enum is reserved in `core`; provider variants are unimplemented-loud per AD-21; underlay wiring — AD-12 explicitly assumes nothing about it; bearer token rides the additive-only wire rule of AD-3; multi-box is out of v1 scope by spec). No deferral leaves two v1 units free to diverge before its revisit condition.

## Checklist item 4 — Is every named technology verified-current (claimed 2026-08-29)?

**Yes — fully verified today (2026-08-29) against live sources.** Every one of the 17 crates matches crates.io `max_stable_version` exactly: tonic/tonic-build 0.14.6, prost 0.14.4, rustls 0.23.43, tokio-rustls 0.26.4, rcgen 0.14.10, blake3 1.8.7, clap 4.6.6, rusqlite 0.40.2, tokio 1.53.1, reqwest 0.13.4, serde 1.0.229, toml 1.1.4, tracing 0.1.44, tracing-subscriber 0.3.23, thiserror 2.0.20, anyhow 1.0.104, similar 3.2.0. Rust stable is confirmed 1.98.0 (channel manifest, built 2026-08-18), and edition 2024 is valid on it. The verification claim in the Stack section is true. No finding.

## Checklist item 5 — Does it cover the driving spec's capabilities (CAP-1..CAP-12)?

**Yes.** `binds:` lists all twelve; the Capability → Architecture map has a row per capability, each naming both the owning crates and the governing ADs, and the "grabber state only via daemon Control" preamble correctly routes every *arr-touching capability (CAP-2, 3, 8, 9, 12) through AD-4's gateway topology. CAP-11's deferral matches the spec and increments.md exactly (capability kept, no LLM in v1, token enum reserved). Spot-checks hold: CAP-5's probe-persist chain (transfer probe → store `probes`) matches AD-12; CAP-12's doctor split matches the convention row and the spec's read-only/repair split. One cosmetic nit: the CAP-4 row lists only `transfer` / AD-11+AD-12, though pull jobs are also `jobs` rows (AD-10, `store`) — the resume flow crosses that table row's boundary. Not a coverage gap, just an under-inclusive row.

## Checklist item 6 — Is every dimension this altitude owns decided, deferred, or an open question?

Decided well: paradigm, crate topology, data ownership, wire contract, security identity, scheduling, testing, deployment topology (the "Deployment and environments" diagram fixes the two-machine envelope), and infra/provider strategy (SwizzinBox + AlreadyThere shipped, others loud-unimplemented per AD-21). The spine has no open-questions section, which is fine only if nothing is open — but the operational envelope has silent dimensions that are none of decided/deferred/open:

1. **Upgrade of mediaops itself / build-and-release (silent — the biggest gap).** Nothing fixes how the seedbox `mediaopsd` binary is built for its target (glibc vs. musl static — pointed, given "Lidarr glibc trap" is a named failure the product exists to avoid), how the binary is redeployed after the first bootstrap (is re-`bootstrap` the upgrade path? a `seedbox upgrade` variant?), or what version skew between CLI, home daemon, and seedbox daemon means operationally beyond AD-3's additive-only wire rule (no version handshake, no minimum-version refusal). Two increments will improvise different answers.
2. **Monitoring/alerting (silent).** Scheduled doctor is read-only and timers are fire-and-forget oneshots — but nothing says who notices when doctor fails or the timer stops firing. Even "operator checks `status`; alerting is deliberately out of scope for one operator" would be a decision; silence is not.
3. **Backup/DR for `state.db` (silent where it matters).** `new-machine` export and "re-mint is the disaster path" cover config and TLS, but `state.db` holds the install digests that are the *only* legal proof for remote deletes (spec: "reclaim local-proof uses only the install digest"). Loss of state.db strands reclaim and resume; no backup cadence, no statement that export-on-demand is the accepted risk.
4. **Log retention (silent, minor).** stderr tracing under systemd-user implies journald, but neither that implication nor a retention posture is stated.

---

## Findings (tiered)

| # | Tier | Finding |
| --- | --- | --- |
| F1 | **Critical** | AD-2's legal-edge diagram omits `core → cli` and `core → daemon`, contradicting AD-9, AD-13, AD-17, and AD-21, which all require the binaries to name `core` types directly. As written the spine's central law is unsatisfiable; two builders will resolve it two ways (illegal direct dep vs. improvised re-export). Fix is two diagram edges. |
| F2 | **High** | AD-2's CI test enforces only five enumerated violations, not the stated "only these edges are legal": store/rusqlite in a daemon tree (AD-8), encode in the daemon tree (spec: no seedbox encode), sync/encode → store, and `ffmpeg-next` (AD-16) are all diagram-/rule-illegal but unchecked. The Rule does not prevent its stated divergence. |
| F3 | **High** | Store access pattern is a live divergence for independently built crates: AD-8/AD-10 make `sync`/`encode` consumers-writers via "typed repository traits," but no legal edge reaches `store` from them and the traits' home (core?) and injection point (binaries?) are never stated. |
| F4 | **High** | Operational dimension "upgrade of mediaops itself" is silent: seedbox binary build target (glibc/musl), post-bootstrap redeploy path, and CLI↔daemon version-skew policy are neither decided, deferred, nor open. |
| F5 | **Medium** | Operations envelope partially silent: monitoring/alerting for failed doctor runs or a dead timer; backup/DR for `state.db` (whose install digests are the sole reclaim delete-proof); log retention posture. Each needs a decision, a deferral with a revisit condition, or an open question. |
| F6 | **Medium** | Structural Seed puts the AD-2 enforcement test at `tests/architecture.rs` under an apparently virtual `[workspace]` manifest — cargo compiles no root tests target, so the "enforced in CI" test could silently never run. |
| F7 | **Medium** | `sync` is "pure planner" in the layer map but owns side-effectful apply orchestration per AD-4 and the Structural Seed; two builders can draw the planner/apply line differently. |
| F8 | **Medium** | Plan artifact has no home: not in AD-6's runtime-artifacts tier, no location, no retention, no staleness rule for apply-later beyond the implicit config hash. |
| F9 | **Low** | CAP-4 map row omits `store`/AD-10 though resume rides `jobs` rows; under-inclusive, not wrong. |
| F10 | **Low** | AD-12's re-probe condition ("bind address or underlay changes") names no detection mechanism; AD-1's "logic a test would want" is review-only enforceable. Acceptable at this altitude; noted for the build level. |

## What passed cleanly

- **Stack (item 4):** all 18 named technologies verified current on 2026-08-29 against crates.io and the Rust channel manifest — the table's claim is accurate to the patch version.
- **Deferred (item 3):** all nine deferrals freeze present law and carry a real revisit condition; no pre-revisit divergence channel found.
- **CAP coverage (item 5):** all twelve capabilities mapped to crates and governing ADs, consistent with spec deferrals.
- **Divergence-fixing breadth (item 1):** naming, wire, config, tiers, staging format, TLS, exec, exit codes, output contract, and test regime are all pinned tightly enough for parallel builds.
