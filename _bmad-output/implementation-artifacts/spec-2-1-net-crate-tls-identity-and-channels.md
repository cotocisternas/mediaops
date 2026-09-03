---
title: '2.1 net crate — TLS identity and channels'
type: 'feature'
created: '2026-09-02'
status: 'done'
baseline_commit: 'd2d93318ce2d6b46c3c30a1a46b9e0239d19303f'
as_built: true
context:
  - '_bmad-output/planning-artifacts/epics.md'
---

## Intent

**Problem:** There was no bootstrap-minted mTLS or channel-pool primitive, so Range RPCs could collapse onto one TCP.

**Approach:** As-built record of Epic 2 story 2.1. Landed in `d2d93318` (stand-up), review close `b76cc23fe83347c5ac6f42f5a37f69dde4862beb`, merge `6e2be4c780f3d392c44e62a0ba4db3c2973e3fc1`. No live-box work.

## Boundaries & Constraints

**Always:**
- `net` mints ECDSA P-256 CA, server, and client certs via rcgen.
- rustls server config requires-and-verifies client certs against that CA. `native-tls` is forbidden.
- UDS and TCP share the same rustls config. `endpoint_fingerprint` hashes seedbox address + underlay mode.
- Channel pool is N independent TCP+TLS channels, one in-flight stream per channel. Unit tests are offline.

## Acceptance Criteria (as-built)

- Given bootstrap minting, when `net` runs rcgen, then it produces ECDSA P-256 CA, server, and client certs, and rustls requires-and-verifies client certs against that CA.
- Given UDS and TCP, when serve/connect run in tests, then both transports work through the same rustls config and `endpoint_fingerprint` is a hash of seedbox address + underlay mode.
- Given the channel-pool primitive, when N slots are configured, then it is N independent TCP+TLS channels, one in-flight stream per channel, without a live box.

## Verification

As-built. `git_evidence.py --stories 2-1` cannot attribute these SHAs: subjects named Epic 2 / PR #4, not `2-1`. Future story commits put `N-M` in the subject.
