//! Whether a descriptor a client passed proves it could read a pad.
//!
//! Possession of a descriptor proves access only if it was opened for
//! reading: one opened with `O_PATH` names any file the caller can see,
//! with no access to it at all. Nothing here may wait on another
//! process: a client can pass a descriptor on its own FUSE file system,
//! whose answers it controls.

#![expect(
    unsafe_code,
    reason = "an evdev ioctl has no safe form; the one call is checked as rustix's own patterns are"
)]

use std::os::fd::BorrowedFd;

use ctrlassist_hider::Refusal;
use rustix::fs::{AtFlags, FileType, OFlags, StatxFlags, fcntl_getfl, statx};
use rustix::ioctl::{Getter, ioctl, opcode};

/// The input subsystem's major device number, which every event node has.
const INPUT_MAJOR: u32 = 13;

/// `EVIOCGVERSION`: the evdev protocol version, an `int`.
const EVIOCGVERSION: rustix::ioctl::Opcode = opcode::read::<libc::c_int>(b'E', 0x01);

/// The device number of the input node `fd` is open on, if it was opened
/// for reading and is an event node.
pub(crate) fn event_node(fd: BorrowedFd<'_>) -> Result<libc::dev_t, Refusal> {
    // The file's own flags, which ask nothing of its file system.
    let flags = fcntl_getfl(fd).map_err(|_| Refusal::NotAPad)?;
    let readable =
        matches!(flags & OFlags::RWMODE, mode if mode == OFlags::RDONLY || mode == OFlags::RDWR);
    if flags.contains(OFlags::PATH) || !readable {
        return Err(Refusal::NotAPad);
    }
    // Its type and device number as the kernel last knew them: a FUSE
    // file answers this from its cache, without asking its server.
    let stat = statx(
        fd,
        "",
        AtFlags::EMPTY_PATH | AtFlags::STATX_DONT_SYNC,
        StatxFlags::TYPE,
    )
    .map_err(|_| Refusal::NotAPad)?;
    let character = FileType::from_raw_mode(u32::from(stat.stx_mode)) == FileType::CharacterDevice;
    if !character || stat.stx_rdev_major != INPUT_MAJOR {
        return Err(Refusal::NotAPad);
    }
    // An input node's own driver answers this, which it does only on a
    // descriptor opened for access: an `O_PATH` one is refused here too.
    // SAFETY: `EVIOCGVERSION` is a read request whose argument is one
    // `int`, which is the opcode and output type the getter declares.
    let version = unsafe { Getter::<EVIOCGVERSION, libc::c_int>::new() };
    // SAFETY: the descriptor is borrowed for the call's length and is an
    // input character device, whose driver defines the request; the
    // getter provides the `int` it writes.
    unsafe { ioctl(fd, version) }.map_err(|_| Refusal::NotAPad)?;
    Ok(libc::makedev(stat.stx_rdev_major, stat.stx_rdev_minor))
}
