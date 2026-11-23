//! A virtual DualSense through uhid that is written whole input reports,
//! sensors and touchpad included. The device crate's emulator, copied and
//! trimmed: its own writes only buttons, sticks and triggers, and always
//! gives the neutral calibration, where this gives whichever it is made
//! with.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use ctrlassist_device::{DualSenseData, OWN_MARKER};
use rustix::event::{PollFd, PollFlags, poll};
use rustix::fs::OFlags;

use crate::report::{CALIBRATION, CALIBRATION_SIZE, INPUT_SIZE};

const NAME: &str = "Sony Interactive Entertainment DualSense Wireless Controller";
const VENDOR: u32 = 0x054c;
const PRODUCT: u32 = 0x0ce6;
const VERSION: u32 = 0x8111;
const BUS_USB: u16 = 3;
const DRIVER: &str = "/sys/bus/hid/drivers/playstation";

const PAIRING: u8 = 0x09;
const PAIRING_SIZE: usize = 20;
const FIRMWARE: u8 = 0x20;
const FIRMWARE_SIZE: usize = 64;
const OUTPUT_REPORT: u8 = 0x02;

const UHID_STOP: u32 = 3;
const UHID_OUTPUT: u32 = 6;
const UHID_GET_REPORT: u32 = 9;
const UHID_GET_REPORT_REPLY: u32 = 10;
const UHID_CREATE2: u32 = 11;
const UHID_INPUT2: u32 = 12;
const UHID_SET_REPORT: u32 = 13;
const UHID_SET_REPORT_REPLY: u32 = 14;
const UHID_DATA: usize = 4096;
const UHID_EVENT: usize = 4 + 128 + 64 + 64 + 2 + 2 + 4 + 4 + 4 + 4 + UHID_DATA;
const EIO: u16 = 5;

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    bytes
        .get(at..at + 4)
        .and_then(|bytes| bytes.try_into().ok())
        .map_or(0, u32::from_ne_bytes)
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    bytes
        .get(at..at + 2)
        .and_then(|bytes| bytes.try_into().ok())
        .map_or(0, u16::from_ne_bytes)
}

fn field(event: &mut Vec<u8>, text: &str, width: usize) {
    let mut bytes = text.as_bytes().to_vec();
    bytes.truncate(width - 1);
    bytes.resize(width, 0);
    event.extend_from_slice(&bytes);
}

/// The virtual pad.
#[must_use = "dropping the virtual pad removes it, which a game sees as a disconnect"]
pub struct VirtualDualSense {
    uhid: File,
    pairing: [u8; PAIRING_SIZE],
    firmware: [u8; FIRMWARE_SIZE],
    calibration: [u8; CALIBRATION_SIZE],
    stopped: bool,
    event: Box<[u8; UHID_EVENT]>,
}

impl VirtualDualSense {
    /// Creates the pad with `calibration` as its calibration report,
    /// answers the driver as it binds, and returns once it is bound.
    pub fn create(
        data: &DualSenseData,
        calibration: [u8; CALIBRATION_SIZE],
        address: [u8; 6],
    ) -> io::Result<Self> {
        if !Path::new(DRIVER).exists() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the kernel's PlayStation driver isn't loaded",
            ));
        }
        let firmware: [u8; FIRMWARE_SIZE] = data.firmware.as_slice().try_into().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "the firmware report isn't whole",
            )
        })?;
        let mut pairing = [0u8; PAIRING_SIZE];
        pairing[0] = PAIRING;
        for (place, byte) in pairing[1..7].iter_mut().zip(address.iter().rev()) {
            *place = *byte;
        }
        let [a, b, c, d, e, f] = address;
        let uniq = format!("{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{f:02x}");
        // The daemon's marker, so that a daemon running beside the spike
        // takes this pad for one of its own and leaves it alone.
        let phys = format!("{OWN_MARKER}spike-motion/{}/input3", std::process::id());
        let watch = udev::MonitorBuilder::new()?
            .match_subsystem("hid")?
            .listen()?;
        let uhid = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags((OFlags::NONBLOCK | OFlags::CLOEXEC).bits() as i32)
            .open("/dev/uhid")?;
        let mut pad = Self {
            uhid,
            pairing,
            firmware,
            calibration,
            stopped: false,
            event: Box::new([0; UHID_EVENT]),
        };

        let mut create = Vec::with_capacity(UHID_EVENT);
        create.extend_from_slice(&UHID_CREATE2.to_ne_bytes());
        field(&mut create, NAME, 128);
        field(&mut create, &phys, 64);
        field(&mut create, &uniq, 64);
        let size = u16::try_from(data.descriptor.len().min(UHID_DATA)).unwrap_or(0);
        create.extend_from_slice(&size.to_ne_bytes());
        create.extend_from_slice(&BUS_USB.to_ne_bytes());
        create.extend_from_slice(&VENDOR.to_ne_bytes());
        create.extend_from_slice(&PRODUCT.to_ne_bytes());
        create.extend_from_slice(&VERSION.to_ne_bytes());
        create.extend_from_slice(&0u32.to_ne_bytes());
        create.extend_from_slice(&data.descriptor[..usize::from(size)]);
        create.resize(UHID_EVENT, 0);
        pad.uhid.write_all(&create)?;

        // The driver reads its feature reports as it binds and registers
        // the pad's inputs; only once it is bound does the kernel pass on
        // what the pad reports. Waited for as events.
        loop {
            let mut fds = [
                PollFd::new(&pad.uhid, PollFlags::IN),
                PollFd::new(&watch, PollFlags::IN),
            ];
            poll(&mut fds, None)?;
            pad.serve(|_| {})?;
            if std::mem::take(&mut pad.stopped) {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "the kernel's PlayStation driver refused the pad",
                ));
            }
            for event in watch.iter() {
                let device = event.device();
                let ours = device
                    .property_value("HID_PHYS")
                    .is_some_and(|value| value.to_str() == Some(phys.as_str()));
                if event.event_type() == udev::EventType::Bind && ours {
                    if device.driver().is_none_or(|driver| driver != "playstation") {
                        return Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "the kernel's PlayStation driver didn't take the pad",
                        ));
                    }
                    return Ok(pad);
                }
            }
        }
    }

    /// Sends one input report.
    pub fn write(&mut self, report: &[u8; INPUT_SIZE]) -> io::Result<()> {
        let mut event = [0u8; 6 + INPUT_SIZE];
        event[..4].copy_from_slice(&UHID_INPUT2.to_ne_bytes());
        event[4..6].copy_from_slice(&(INPUT_SIZE as u16).to_ne_bytes());
        event[6..].copy_from_slice(report);
        self.uhid.write_all(&event)
    }

    fn feature(&self, number: u8) -> Option<&[u8]> {
        match number {
            PAIRING => Some(&self.pairing),
            FIRMWARE => Some(&self.firmware),
            CALIBRATION => Some(&self.calibration),
            _ => None,
        }
    }

    /// Answers everything the kernel has waiting, passing each output
    /// report a game sent to `output`. Returns when nothing is left.
    pub fn serve(&mut self, mut output: impl FnMut(&[u8])) -> io::Result<()> {
        loop {
            match self.uhid.read(self.event.as_mut_slice()) {
                Ok(0) => return Ok(()),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }
            let event = self.event.as_slice();
            match u32_at(event, 0) {
                UHID_STOP => self.stopped = true,
                // data[4096], then size u16.
                UHID_OUTPUT => {
                    let size = usize::from(u16_at(event, 4 + UHID_DATA)).min(UHID_DATA);
                    if let Some(report) = event.get(4..4 + size)
                        && report.first() == Some(&OUTPUT_REPORT)
                    {
                        output(report);
                    }
                }
                // id u32, then the report's number.
                UHID_GET_REPORT => {
                    let id = u32_at(event, 4);
                    let number = event.get(8).copied().unwrap_or(0);
                    tracing::debug!(number, "the driver or a game asked a feature report");
                    let mut reply = [0u8; 12 + FIRMWARE_SIZE];
                    reply[..4].copy_from_slice(&UHID_GET_REPORT_REPLY.to_ne_bytes());
                    reply[4..8].copy_from_slice(&id.to_ne_bytes());
                    let length = match self.feature(number) {
                        Some(report) => {
                            reply[10..12].copy_from_slice(&(report.len() as u16).to_ne_bytes());
                            reply[12..12 + report.len()].copy_from_slice(report);
                            12 + report.len()
                        }
                        None => {
                            reply[8..10].copy_from_slice(&EIO.to_ne_bytes());
                            12
                        }
                    };
                    self.uhid.write_all(&reply[..length])?;
                }
                UHID_SET_REPORT => {
                    let mut reply = [0u8; 10];
                    reply[..4].copy_from_slice(&UHID_SET_REPORT_REPLY.to_ne_bytes());
                    reply[4..8].copy_from_slice(&u32_at(event, 4).to_ne_bytes());
                    self.uhid.write_all(&reply)?;
                }
                _ => {}
            }
        }
    }
}

impl AsFd for VirtualDualSense {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.uhid.as_fd()
    }
}
