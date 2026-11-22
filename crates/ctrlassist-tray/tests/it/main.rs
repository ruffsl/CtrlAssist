//! The tray's settings service against a live daemon, with no desktop:
//! what the tray serves is read as the window reads it, through the
//! SDK's tray client on another link to the same daemon.

#![expect(
    clippy::unwrap_used,
    reason = "integration tests fail loudly by design; clippy's allow-*-in-tests keys don't reach helper fns under tests/"
)]

mod settings;
