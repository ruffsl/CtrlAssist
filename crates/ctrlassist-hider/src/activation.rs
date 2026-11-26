//! Taking the listening socket systemd hands over.

#![expect(
    unsafe_code,
    reason = "adopting an inherited descriptor has no safe form"
)]

use std::os::fd::{FromRawFd, OwnedFd};

use rustix::io::{FdFlags, fcntl_setfd};

/// The first descriptor systemd passes, by its protocol.
const FIRST: i32 = 3;

/// The one listening socket systemd passed to this process, if it did.
/// Made close-on-exec, as everything else the service opens is: it runs
/// no program, and nothing it runs should inherit the socket if it ever
/// does.
pub(crate) fn listener() -> Option<OwnedFd> {
    let for_us = std::env::var("LISTEN_PID").ok()?.parse::<u32>().ok()? == std::process::id();
    let one = std::env::var("LISTEN_FDS").ok()?.parse::<u32>().ok()? == 1;
    // SAFETY: systemd's socket activation protocol guarantees that, when
    // LISTEN_PID names this process and LISTEN_FDS is 1, descriptor 3 is
    // open, is the socket, and belongs to this process alone. This is
    // called once, so nothing else owns it.
    let listener = (for_us && one).then(|| unsafe { OwnedFd::from_raw_fd(FIRST) })?;
    fcntl_setfd(&listener, FdFlags::CLOEXEC).ok()?;
    Some(listener)
}
