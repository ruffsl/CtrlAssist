//! The calls with no safe wrapper here: hidraw's requests, copied from
//! the device crate, and a descriptor that reads the signals that end
//! the spike.

#![expect(
    unsafe_code,
    reason = "hidraw's requests and signalfd have no safe wrapper in the crates used here"
)]

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd};

const READ: u64 = 2;
const WRITE: u64 = 1;

/// A hidraw request number, as linux/ioctl.h builds one.
const fn request(direction: u64, number: u64, size: usize) -> u64 {
    (direction << 30) | ((size as u64) << 16) | ((b'H' as u64) << 8) | number
}

/// `struct hidraw_devinfo`, from linux/hidraw.h.
#[repr(C)]
struct Info {
    bus: u32,
    vendor: i16,
    product: i16,
}

fn checked(result: libc::c_int) -> io::Result<usize> {
    usize::try_from(result).map_err(|_| io::Error::last_os_error())
}

/// The bus, vendor and product of the device behind a raw HID node.
pub fn info(hidraw: BorrowedFd<'_>) -> io::Result<(u16, u16, u16)> {
    let mut info = Info {
        bus: 0,
        vendor: 0,
        product: 0,
    };
    let number = request(READ, 0x03, size_of::<Info>());
    // SAFETY: HIDIOCGRAWINFO writes one `hidraw_devinfo` to the pointer,
    // which is to one that lives for the length of the call.
    let result = unsafe { libc::ioctl(hidraw.as_raw_fd(), number as _, &raw mut info) };
    checked(result)?;
    Ok((
        u16::try_from(info.bus).unwrap_or(0),
        info.vendor.cast_unsigned(),
        info.product.cast_unsigned(),
    ))
}

/// The device's feature report `id`, which is `size` bytes with its id.
pub fn feature(hidraw: BorrowedFd<'_>, id: u8, size: usize) -> io::Result<Vec<u8>> {
    let mut report = vec![0u8; size.max(1)];
    report[0] = id;
    let number = request(READ | WRITE, 0x07, report.len());
    // SAFETY: HIDIOCGFEATURE reads the report's id from the first byte
    // and writes at most the length encoded in the request, which is the
    // buffer's own; the buffer lives for the length of the call.
    let result = unsafe { libc::ioctl(hidraw.as_raw_fd(), number as _, report.as_mut_ptr()) };
    let length = checked(result)?;
    report.truncate(length);
    Ok(report)
}

/// Blocks SIGINT and SIGTERM for this thread and every thread it starts
/// after, and returns a descriptor that is readable once one comes: so
/// the spike ends by its own hand, leaving the pad at rest. Call before
/// any thread is started.
pub fn ending_signals() -> io::Result<OwnedFd> {
    // SAFETY: an all-zero `sigset_t` is a valid value to initialize.
    let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
    // SAFETY: `set` is a valid, owned signal set for the call.
    unsafe { libc::sigemptyset(&raw mut set) };
    for signal in [libc::SIGINT, libc::SIGTERM] {
        // SAFETY: as above; the signal numbers are valid.
        unsafe { libc::sigaddset(&raw mut set, signal) };
    }
    // SAFETY: `set` is valid for the call and the old set isn't asked for.
    let result =
        unsafe { libc::pthread_sigmask(libc::SIG_BLOCK, &raw const set, std::ptr::null_mut()) };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result));
    }
    // SAFETY: -1 asks for a new descriptor; `set` is valid for the call.
    let fd = unsafe { libc::signalfd(-1, &raw const set, libc::SFD_CLOEXEC) };
    checked(fd)?;
    // SAFETY: signalfd returned a new descriptor that nothing else owns.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}
