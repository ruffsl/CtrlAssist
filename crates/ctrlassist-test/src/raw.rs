//! Feature reports through a raw HID node, as a program that holds the
//! node writes and reads them: the tests' own requests, apart from the
//! device crate's.

#![expect(
    unsafe_code,
    reason = "the hidraw feature requests have no safe wrapper in the crates used here"
)]

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};

/// A hidraw request that both reads and writes its buffer, as
/// linux/hidraw.h builds `HIDIOCSFEATURE` and `HIDIOCGFEATURE`.
const fn request(number: u64, size: usize) -> u64 {
    (3 << 30) | ((size as u64) << 16) | ((b'H' as u64) << 8) | number
}

/// Sets a feature report: `report` is its number, then its bytes.
/// Returns how many bytes the kernel took.
///
/// # Errors
///
/// Fails if the node refuses the request.
pub fn set_feature(raw: BorrowedFd<'_>, report: &[u8]) -> io::Result<usize> {
    let mut buffer = report.to_vec();
    let number = request(0x06, buffer.len());
    // SAFETY: HIDIOCSFEATURE reads at most the length encoded in the
    // request, which is the buffer's own; the buffer lives for the length
    // of the call.
    let result = unsafe { libc::ioctl(raw.as_raw_fd(), number as _, buffer.as_mut_ptr()) };
    usize::try_from(result).map_err(|_| io::Error::last_os_error())
}

/// Gets feature report `id`, `size` bytes with its number, as the device
/// answers it.
///
/// # Errors
///
/// Fails if the node refuses the request or the device doesn't answer.
pub fn get_feature(raw: BorrowedFd<'_>, id: u8, size: usize) -> io::Result<Vec<u8>> {
    let mut report = vec![0u8; size.max(1)];
    report[0] = id;
    let number = request(0x07, report.len());
    // SAFETY: HIDIOCGFEATURE reads the report's number from the first
    // byte and writes at most the length encoded in the request, which is
    // the buffer's own; the buffer lives for the length of the call.
    let result = unsafe { libc::ioctl(raw.as_raw_fd(), number as _, report.as_mut_ptr()) };
    let length = usize::try_from(result).map_err(|_| io::Error::last_os_error())?;
    report.truncate(length);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The request numbers are the kernel's own, from linux/hidraw.h.
    #[test]
    fn the_request_numbers_are_the_kernel_s() {
        assert_eq!(request(0x06, 64), 0xc040_4806);
        assert_eq!(request(0x07, 64), 0xc040_4807);
    }
}
