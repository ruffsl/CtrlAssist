//! `ctrlassist` SDK integration tests: one test binary, one module per
//! suite.

#![expect(
    clippy::unwrap_used,
    reason = "integration tests fail loudly by design; clippy's allow-*-in-tests keys don't reach helper fns under tests/"
)]

mod client;
mod fake;
mod pads;
