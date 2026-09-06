//! Live-box gate (AD-20). Compiles only with `--features live-box`.
//! Default `cargo test` never enables it. Even with the feature, nothing
//! here dials SeedIt4Me or the GPU unless `MEDIAOPS_LIVE=1` **and** the
//! operator has confirmed — and this file still refuses to do that.

#![cfg(feature = "live-box")]

fn live_env() -> bool {
    std::env::var("MEDIAOPS_LIVE").ok().as_deref() == Some("1")
}

#[test]
#[ignore = "requires MEDIAOPS_LIVE=1 and operator confirm; never dials SeedIt4Me or the GPU"]
fn live_demo_pending_operator_confirm() {
    if !live_env() {
        return;
    }
    // Do not SSH, pull, or encode. Current operator steps are in docs/setup.md;
    // the test boundary is documented in docs/development.md#tests.
}
