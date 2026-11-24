//! Whether a socket takes descriptors: `SO_PASSRIGHTS`, from Linux 6.16.
//!
//! A Unix socket that doesn't take them has a send that carries any fail
//! with `EPERM` at the sender, so none is ever queued on it. A connection
//! waiting to be accepted has its listener's setting from when it
//! connected, so a listener that doesn't take them leaves nothing for a
//! connection's close to release before the service has admitted it.

#![expect(
    unsafe_code,
    reason = "SO_PASSRIGHTS has no safe binding yet; each call is one setsockopt or getsockopt with its contract stated"
)]

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};

use libc::c_int;

/// `SO_PASSRIGHTS`, which libc doesn't name yet for these architectures:
/// its number in the kernel's generic socket header, the one every
/// architecture but Alpha, MIPS, PA-RISC and SPARC uses.
#[cfg(not(any(
    target_arch = "mips",
    target_arch = "mips32r6",
    target_arch = "mips64",
    target_arch = "mips64r6",
    target_arch = "sparc",
    target_arch = "sparc64"
)))]
const SO_PASSRIGHTS: c_int = 83;

/// The length of the option's value, an `int`.
fn length() -> libc::socklen_t {
    libc::socklen_t::try_from(size_of::<c_int>()).expect("an int's size fits a socklen_t")
}

/// Whether `socket` takes descriptors. Fails with `ENOPROTOOPT` on a
/// kernel from before the option, where every Unix socket takes them.
///
/// # Errors
///
/// Fails if the kernel can't say.
pub fn takes_descriptors(socket: BorrowedFd<'_>) -> io::Result<bool> {
    let mut value: c_int = 0;
    let mut size = length();
    // SAFETY: `socket` is a live descriptor for the call; `value` is an
    // int the kernel writes at most `size` bytes of, `size` its size,
    // which the kernel writes back.
    let result = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            SO_PASSRIGHTS,
            (&raw mut value).cast(),
            &raw mut size,
        )
    };
    if result == 0 {
        Ok(value != 0)
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Has `socket` take descriptors, or not. Fails with `ENOPROTOOPT` on a
/// kernel from before the option.
///
/// # Errors
///
/// Fails if the kernel won't.
pub fn take_descriptors(socket: BorrowedFd<'_>, take: bool) -> io::Result<()> {
    let value = c_int::from(take);
    // SAFETY: `socket` is a live descriptor for the call; `value` is an
    // int of the length given, which the kernel only reads.
    let result = unsafe {
        libc::setsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            SO_PASSRIGHTS,
            (&raw const value).cast(),
            length(),
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Whether `error` says the kernel has no such option.
#[must_use]
pub fn is_unknown_to_the_kernel(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ENOPROTOOPT)
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsFd;

    use rustix::net::{AddressFamily, SocketType, socket};

    use super::*;

    /// A socket takes descriptors until told not to, and says so; on a
    /// kernel without the option, both say it is unknown.
    #[test]
    fn a_socket_says_whether_it_takes_descriptors() {
        let socket = socket(AddressFamily::UNIX, SocketType::SEQPACKET, None).unwrap();
        match takes_descriptors(socket.as_fd()) {
            Ok(taken) => {
                assert!(taken, "a new socket takes them");
                take_descriptors(socket.as_fd(), false).unwrap();
                assert!(!takes_descriptors(socket.as_fd()).unwrap());
                take_descriptors(socket.as_fd(), true).unwrap();
                assert!(takes_descriptors(socket.as_fd()).unwrap());
            }
            Err(error) => {
                assert!(is_unknown_to_the_kernel(&error), "{error}");
                let error = take_descriptors(socket.as_fd(), false).unwrap_err();
                assert!(is_unknown_to_the_kernel(&error), "{error}");
            }
        }
    }
}
