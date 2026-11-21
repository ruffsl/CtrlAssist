//! The crate's integration tests, as one binary.

#![expect(clippy::unwrap_used, reason = "tests fail loudly")]

mod allocations;
mod properties;
mod scenarios;
