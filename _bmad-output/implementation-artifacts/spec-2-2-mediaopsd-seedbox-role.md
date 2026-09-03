---
title: '2.2 mediaopsd seedbox role'
type: 'feature'
created: '2026-09-02'
status: 'done'
baseline_commit: 'd2d93318ce2d6b46c3c30a1a46b9e0239d19303f'
as_built: true
context:
  - '_bmad-output/planning-artifacts/epics.md'
---

## Intent

**Problem:** Health after bootstrap was not gRPC: there was no seedbox-role daemon binding Control + Transfer.

**Approach:** As-built record of Epic 2 story 2.2. Landed in `d2d93318` (stand-up), review close `b76cc23fe83347c5ac6f42f5a37f69dde4862beb`, merge `6e2be4c780f3d392c44e62a0ba4db3c2973e3fc1`. `grabber=None` is legal. No store or encode in the daemon.

## Boundaries & Constraints

**Always:**
- Role seedbox binds TCP gRPC+mTLS and serves Transfer (listing, Stat, streaming GetRange) backed by the one walker.
- Control includes at least `df` plus version + proto-package handshake. `grabber=None` means no live *arr calls.
- The seedbox role does not link `store` or `encode`. Reverse-connect stays a designed-unused mode of this same binary.

## Acceptance Criteria (as-built)

- Given config role = seedbox, when mediaopsd starts, then it binds TCP gRPC+mTLS and serves Transfer backed by the walker, and Control includes `df` plus version + proto-package handshake.
- Given the seedbox role, when the binary is linked, then it does not link `store` or `encode`.
- Given a Control response, when the CLI reads it, then it carries daemon semver + proto package name.

## Verification

As-built. `git_evidence.py --stories 2-2` cannot attribute these SHAs: subjects named Epic 2 / PR #4, not `2-2`. Future story commits put `N-M` in the subject.
