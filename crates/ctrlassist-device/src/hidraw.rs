//! Reading what a pad says of itself from its raw HID node.

#![expect(
    unsafe_code,
    reason = "the hidraw requests have no safe wrapper in the crates used here"
)]

use std::io;
use std::os::fd::{AsRawFd, BorrowedFd};

/// The most a report descriptor holds, from linux/hid.h.
const DESCRIPTOR_MAX: usize = 4096;

const READ: u64 = 2;
const WRITE: u64 = 1;

/// A hidraw request number, as linux/ioctl.h builds one.
const fn request(direction: u64, number: u64, size: usize) -> u64 {
    (direction << 30) | ((size as u64) << 16) | ((b'H' as u64) << 8) | number
}

/// `struct hidraw_report_descriptor`, from linux/hidraw.h.
#[repr(C)]
struct Descriptor {
    size: u32,
    value: [u8; DESCRIPTOR_MAX],
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
pub(crate) fn info(hidraw: BorrowedFd<'_>) -> io::Result<(u16, u16, u16)> {
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
    let bus = u16::try_from(info.bus).unwrap_or(0);
    Ok((
        bus,
        info.vendor.cast_unsigned(),
        info.product.cast_unsigned(),
    ))
}

/// The device's report descriptor.
pub(crate) fn descriptor(hidraw: BorrowedFd<'_>) -> io::Result<Vec<u8>> {
    let mut size: libc::c_int = 0;
    let number = request(READ, 0x01, size_of::<libc::c_int>());
    // SAFETY: HIDIOCGRDESCSIZE writes one `int` to the pointer, which is
    // to one that lives for the length of the call.
    let result = unsafe { libc::ioctl(hidraw.as_raw_fd(), number as _, &raw mut size) };
    checked(result)?;
    let size = usize::try_from(size).unwrap_or(0).min(DESCRIPTOR_MAX);
    let mut descriptor = Descriptor {
        size: size as u32,
        value: [0; DESCRIPTOR_MAX],
    };
    let number = request(READ, 0x02, size_of::<Descriptor>());
    // SAFETY: HIDIOCGRDESC reads the size asked for from the struct and
    // writes at most that many bytes into its array, which holds the most
    // a descriptor can be; the struct lives for the length of the call.
    let result = unsafe { libc::ioctl(hidraw.as_raw_fd(), number as _, &raw mut descriptor) };
    checked(result)?;
    Ok(descriptor.value.get(..size).unwrap_or_default().to_vec())
}

/// The device's feature report `id`, which is `size` bytes with its id.
pub(crate) fn feature(hidraw: BorrowedFd<'_>, id: u8, size: usize) -> io::Result<Vec<u8>> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The request numbers are the kernel's own, from linux/hidraw.h.
    #[test]
    fn the_request_numbers_are_the_kernel_s() {
        assert_eq!(request(READ, 0x01, 4), 0x8004_4801);
        assert_eq!(request(READ, 0x02, size_of::<Descriptor>()), 0x9004_4802);
        assert_eq!(request(READ, 0x03, size_of::<Info>()), 0x8008_4803);
        assert_eq!(request(READ | WRITE, 0x07, 64), 0xc040_4807);
    }

    #[test]
    fn something_that_is_no_raw_node_is_an_error() {
        let file = std::fs::File::open("/dev/null").unwrap();
        use std::os::fd::AsFd;
        assert!(info(file.as_fd()).is_err());
        assert!(descriptor(file.as_fd()).is_err());
        assert!(feature(file.as_fd(), 0x20, 64).is_err());
    }
}
