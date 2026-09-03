---
title: '2.3 Seedbox bootstrap over ssh'
type: 'feature'
created: '2026-09-02'
status: 'done'
baseline_commit: 'd2d93318ce2d6b46c3c30a1a46b9e0239d19303f'
as_built: true
context:
  - '_bmad-output/planning-artifacts/epics.md'
---

## Intent

**Problem:** A new box could not answer gRPC under mTLS without daily SSH: no `mediaops seedbox bootstrap` to install musl-static mediaopsd and mint certs.

**Approach:** As-built record of Epic 2 story 2.3. Landed in `d2d93318` (stand-up), review close `b76cc23fe83347c5ac6f42f5a37f69dde4862beb`, merge `6e2be4c780f3d392c44e62a0ba4db3c2973e3fc1`. Live SeedIt4Me execution was not part of this story's offline landing.

## Boundaries & Constraints

**Always:**
- Import `~/.ssh/config` Host `seedbox`. Build `x86_64-unknown-linux-musl` mediaopsd. Copy binary + systemd user unit. Mint certs into `~/.config/mediaops/tls/`. Refuse to mint into a git work tree.
- Desired-state stores SHA-256-of-DER lowercase-hex fingerprints and paths, never PEMs.
- SwizzinBox and AlreadyThere complete (AlreadyThere is no-op install). Unimplemented providers fail loudly.
- Range concurrency probe persists N keyed by `endpoint_fingerprint` in `probes`. Re-probe only on fingerprint mismatch.
- Unit tests use exec-port transcripts. Bulk copy over SSH is a test failure.

## Acceptance Criteria (as-built)

- Given `~/.ssh/config` Host `seedbox`, when `mediaops seedbox bootstrap --json` runs, then it imports that host, builds musl mediaopsd, copies binary + unit, mints certs into the active config dir, and refuses to mint into a git work tree.
- Given Provider SwizzinBox or AlreadyThere, then bootstrap completes; unimplemented providers return errors with tests that they fail loudly.
- Given gRPC is up, when bootstrap probes Range concurrency, then it raises N until throughput plateaus and persists N keyed by `endpoint_fingerprint`.
- Given unit tests, when they cover bootstrap, then they use exec-port transcripts.

## Verification

As-built. `git_evidence.py --stories 2-3` cannot attribute these SHAs: subjects named Epic 2 / PR #4, not `2-3`. Future story commits put `N-M` in the subject.
