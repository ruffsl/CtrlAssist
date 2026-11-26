//! A virtual Steam Controller, made through uhid.
//!
//! SDL and Steam read a Steam Controller through its raw HID node, so
//! passing as one takes a HID device, which the kernel's `hid-steam`
//! binds as it does the real thing: it makes the pad's gamepad and
//! motion devices, and the second HID device whose raw node is what
//! those two read. The emulator presents a report descriptor written
//! here, answers what the driver asks as it binds, turns pad state into
//! the pad's state report, and reads rumble out of the output reports it
//! is sent.
//!
//! The descriptor declares Valve's collection alone: the reports this
//! sends and takes, each at its length, and none of the keyboard and
//! pointer a real pad has beside them, which a virtual one would add to
//! the desktop and never use. SDL 3.4 makes the same of a stand-in
//! under this and under a real pad's descriptor; what Steam makes of it
//! nobody has seen.
//!
//! What a reader sets is kept here and goes no further: the settings
//! Steam and SDL write, lizard mode and the motion sensors' mode among
//! them, are acknowledged and answered back as set, and none reaches a
//! pad behind the virtual one. The virtual pad never types or points.
//!
//! A report is sent as the pad's state changes and not otherwise: a
//! real pad reports every 4 ms, changed or not, and neither the
//! kernel's driver nor SDL takes a quiet pad for one that has gone.
//! Whether Steam does is to be seen.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use ctrlassist_core::{FeedbackError, Level, PadState};
use rustix::event::{PollFd, PollFlags, poll};
use rustix::fs::OFlags;

use super::{
    BATTERY, BATTERY_SIZE, FEATURE_PAD, FEATURE_SIZE, PRODUCT_CABLE, RUMBLE, RUMBLE_SIZE, STATE,
    STATE_PLAIN, STATE_PLAIN_SIZE, STATE_SIZE, VENDOR, WIRELESS, WIRELESS_SIZE,
};
use crate::sink::{OWN_MARKER, Request};

/// The name a Steam Controller's HID device has on its cable. The
/// kernel's driver names the gamepad it makes itself.
pub(crate) const NAME: &str = "Valve Software Steam Controller";
/// The version a Steam Controller's HID device reports on its cable.
const VERSION: u32 = 0x0307;
/// The bus type for USB, from linux/input.h.
const BUS_USB: u16 = 3;
/// Where the kernel's driver is, once its module is loaded.
const DRIVER: &str = "/sys/bus/hid/drivers/hid-steam";
const DRIVER_NAME: &str = "hid-steam";

/// A trackpad's pulse, the output report after rumble
/// (`REPORT_ID_HAPTIC_PULSE`, `hid-steam.c:330`), and its size, its
/// number included.
const PULSE: u8 = 0x81;
const PULSE_SIZE: usize = 8;

// The commands answered with more than what was set
// (`ID_GET_ATTRIBUTES_VALUES` and `ID_GET_STRING_ATTRIBUTE`,
// `hid-steam.c:91-144`).
const GET_ATTRIBUTES: u8 = 0x83;
const GET_STRING_ATTRIBUTE: u8 = 0xae;
/// The length of the field a serial is answered in, as the kernel's
/// driver asks for it (`steam_get_serial`, `hid-steam.c:617-650`).
const SERIAL_FIELD: u8 = 0x15;
/// The most a serial is: the battery the driver names `steam-<serial>`
/// has a thermal zone, whose name is at most 19 characters.
pub(crate) const SERIAL_MOST: usize = 13;

// Event types from linux/uhid.h.
const UHID_STOP: u32 = 3;
const UHID_OUTPUT: u32 = 6;
const UHID_GET_REPORT: u32 = 9;
const UHID_GET_REPORT_REPLY: u32 = 10;
const UHID_CREATE2: u32 = 11;
const UHID_INPUT2: u32 = 12;
const UHID_SET_REPORT: u32 = 13;
const UHID_SET_REPORT_REPLY: u32 = 14;
/// The size of a uhid event's data.
const UHID_DATA: usize = 4096;
/// The size of the largest uhid event, `UHID_CREATE2`.
const UHID_EVENT: usize = 4 + 128 + 64 + 64 + 2 + 2 + 4 + 4 + 4 + 4 + UHID_DATA;
/// `EIO`, for a feature report asked for before any command was set.
const EIO: u16 = 5;
/// The state report's and a feature report's sizes as uhid takes them.
const STATE_LENGTH: u16 = 54;
const FEATURE_LENGTH: u16 = 64;
const _: () =
    assert!(STATE_LENGTH as usize == STATE_SIZE && FEATURE_LENGTH as usize == FEATURE_SIZE);

/// A short item of a report descriptor with one byte of data.
const fn item(prefix: u8, data: u8) -> [u8; 2] {
    [prefix | 1, data]
}

/// One report of Valve's collection: its number, its length after the
/// number in bytes, and the main item it is.
fn declared(id: u8, size: usize, main: u8) -> [u8; 8] {
    let after = u8::try_from(size.saturating_sub(1)).unwrap_or(u8::MAX);
    let [a, b] = item(0x84, id); // Report ID.
    let [c, d] = item(0x94, after); // Report count.
    let [e, f] = item(0x08, id); // Usage.
    let [g, h] = item(main, 0x02); // Data, variable, absolute.
    [a, b, c, d, e, f, g, h]
}

const INPUT: u8 = 0x80;
const OUTPUT: u8 = 0x90;
const FEATURE: u8 = 0xb0;

/// The report descriptor: Valve's vendor collection, each report a run
/// of bytes of 0 to 255. Every report a Steam Controller on its cable
/// sends that the kernel's driver or SDL reads is declared at its
/// length, since the kernel drops one undeclared or short; the two
/// output reports for rumble and a trackpad's pulse; and the feature
/// report commands go by.
pub(crate) fn descriptor() -> Vec<u8> {
    let mut descriptor = vec![
        0x06, 0x00, 0xff, // Usage page: vendor 0xff00.
        0x09, 0x01, // Usage 1.
        0xa1, 0x01, // Collection: application.
        0x15, 0x00, // Logical minimum 0.
        0x26, 0xff, 0x00, // Logical maximum 255.
        0x75, 0x08, // Report size: 8 bits.
    ];
    for (id, size) in [
        (STATE, STATE_SIZE),
        (STATE_PLAIN, STATE_PLAIN_SIZE),
        (BATTERY, BATTERY_SIZE),
        (WIRELESS, WIRELESS_SIZE),
    ] {
        descriptor.extend(declared(id, size, INPUT));
    }
    descriptor.extend(declared(RUMBLE, RUMBLE_SIZE, OUTPUT));
    descriptor.extend(declared(PULSE, PULSE_SIZE, OUTPUT));
    descriptor.extend(declared(FEATURE_PAD, FEATURE_SIZE, FEATURE));
    descriptor.push(0xc0); // End of the collection.
    descriptor
}

/// Appends `text` as a fixed-width, zero-terminated field.
fn field(event: &mut Vec<u8>, text: &str, width: usize) {
    let mut bytes = text.as_bytes().to_vec();
    bytes.truncate(width.saturating_sub(1));
    bytes.resize(width, 0);
    event.extend_from_slice(&bytes);
}

/// `UHID_CREATE2`, packed: type u32, name[128], phys[64], uniq[64],
/// rd_size u16, bus u16, vendor u32, product u32, version u32, country
/// u32, rd_data[4096].
fn create2(phys: &str, uniq: &str, descriptor: &[u8]) -> Vec<u8> {
    let mut event = Vec::with_capacity(UHID_EVENT);
    event.extend_from_slice(&UHID_CREATE2.to_ne_bytes());
    field(&mut event, NAME, 128);
    field(&mut event, phys, 64);
    field(&mut event, uniq, 64);
    let size = u16::try_from(descriptor.len().min(UHID_DATA)).unwrap_or(0);
    event.extend_from_slice(&size.to_ne_bytes());
    event.extend_from_slice(&BUS_USB.to_ne_bytes());
    event.extend_from_slice(&u32::from(VENDOR).to_ne_bytes());
    event.extend_from_slice(&u32::from(PRODUCT_CABLE).to_ne_bytes());
    event.extend_from_slice(&VERSION.to_ne_bytes());
    event.extend_from_slice(&0u32.to_ne_bytes());
    event.extend(descriptor.iter().take(usize::from(size)));
    event.resize(UHID_EVENT, 0);
    event
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    bytes
        .get(at..)
        .and_then(|bytes| bytes.first_chunk::<4>())
        .map_or(0, |bytes| u32::from_ne_bytes(*bytes))
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    bytes
        .get(at..)
        .and_then(|bytes| bytes.first_chunk::<2>())
        .map_or(0, |bytes| u16::from_ne_bytes(*bytes))
}

/// The firmware's build time a virtual pad says, in seconds since 1970:
/// the latest a signed 32 bits hold, so that a reader that offers a pad
/// newer firmware than its own finds none newer.
const FIRMWARE_BUILT: u32 = 0x7fff_ffff;

/// The answer to a GET of the commands' feature report, from the command
/// last set on it: the serial for a string asked for, the pad's
/// attributes, and for anything else the command as it was set, which is
/// how a pad acknowledges one.
fn answer(serial: &str, last: &[u8; FEATURE_SIZE]) -> [u8; FEATURE_SIZE] {
    let mut reply = *last;
    let [_, command, _, which, ..] = *last;
    match command {
        GET_STRING_ATTRIBUTE => {
            reply = [0; FEATURE_SIZE];
            // The field's length, which string, then the string: the
            // serial for the unit's and the board's alike.
            let head = [FEATURE_PAD, command, SERIAL_FIELD, which];
            let text = serial.as_bytes();
            let text = text.get(..text.len().min(SERIAL_MOST)).unwrap_or(text);
            for (place, byte) in reply.iter_mut().zip(head.iter().chain(text)) {
                *place = *byte;
            }
        }
        GET_ATTRIBUTES => {
            reply = [0; FEATURE_SIZE];
            // Each attribute a tag and a 32-bit value, as a pad on its
            // cable answers: its product, its capabilities, its
            // bootloader's and firmware's build times, its board, and
            // the interval it reports at, 4000 µs, which the kernel's
            // driver reads (`steam_get_attributes`).
            let attributes: [(u8, u32); 6] = [
                (1, u32::from(PRODUCT_CABLE)),
                (2, 0),
                (10, FIRMWARE_BUILT),
                (4, FIRMWARE_BUILT),
                (9, 0x4a),
                (11, 4000),
            ];
            let length = u8::try_from(attributes.len().saturating_mul(5)).unwrap_or(0);
            let mut body = Vec::with_capacity(FEATURE_SIZE);
            body.extend_from_slice(&[FEATURE_PAD, command, length]);
            for (tag, value) in attributes {
                body.push(tag);
                body.extend_from_slice(&value.to_le_bytes());
            }
            for (place, byte) in reply.iter_mut().zip(body) {
                *place = byte;
            }
        }
        _ => {}
    }
    reply
}

/// A virtual Steam Controller.
pub(crate) struct Emulator {
    uhid: File,
    /// Its raw node, the client HID device's: what SDL and Steam read.
    raw_node: PathBuf,
    serial: String,
    /// The report last sent, from its buttons on, so that a state that
    /// changes nothing a report says sends none.
    sent: Option<[u8; STATE_SIZE]>,
    sequence: u8,
    /// The sensor clock, in microseconds, and the reading it was last
    /// advanced for.
    time: u32,
    readings: u32,
    /// The command last set on the commands' feature report, which the
    /// next GET of it answers.
    command: Option<[u8; FEATURE_SIZE]>,
    /// The rumble last asked for, so that a report that leaves it as it
    /// is isn't taken for a new level.
    level: Level,
    /// Whether the driver has let the pad go since this was last asked.
    stopped: bool,
    /// Kept between calls so that serving allocates nothing.
    event: Box<[u8; UHID_EVENT]>,
}

impl Emulator {
    /// Creates the pad with `serial`, answers the driver as it binds,
    /// and waits for the raw node of the client device the driver makes.
    pub(crate) fn create(serial: &str) -> io::Result<Self> {
        // With the driver there, it is the one to bind. Without, the
        // generic driver would, and the pad would be no Steam Controller
        // to anything.
        if !Path::new(DRIVER).exists() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the kernel's Steam Controller driver isn't loaded, or is older than Linux 7.3",
            ));
        }
        // The process and a count tell virtual pads apart in the marker.
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let phys = format!("{OWN_MARKER}{}/steam/{number}", std::process::id());
        // Listening before the pad is made, so its arrival isn't missed.
        let watch = udev::MonitorBuilder::new()?
            .match_subsystem("hid")?
            .match_subsystem("hidraw")?
            .listen()?;
        let uhid = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags((OFlags::NONBLOCK | OFlags::CLOEXEC).bits() as i32)
            .open("/dev/uhid")?;
        let mut pad = Self {
            uhid,
            raw_node: PathBuf::new(),
            serial: serial.chars().take(SERIAL_MOST).collect(),
            sent: None,
            sequence: 0,
            time: 0,
            readings: 0,
            command: None,
            level: Level::default(),
            stopped: false,
            event: Box::new([0; UHID_EVENT]),
        };
        let uniq = pad.serial.clone();
        pad.uhid.write_all(&create2(&phys, &uniq, &descriptor()))?;
        // The driver asks the pad's serial and attributes as it binds,
        // and makes the client device, whose raw node comes after. All
        // of that is waited for here, as events, however long it takes.
        let mut bound = false;
        loop {
            if bound && !pad.raw_node.as_os_str().is_empty() {
                return Ok(pad);
            }
            let mut fds = [
                PollFd::new(&pad.uhid, PollFlags::IN),
                PollFd::new(&watch, PollFlags::IN),
            ];
            poll(&mut fds, None)?;
            pad.serve(|_| Ok(()))?;
            if std::mem::take(&mut pad.stopped) && bound {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "the kernel's Steam Controller driver let the pad go as it was made",
                ));
            }
            for event in watch.iter() {
                let device = event.device();
                let hid = if device.subsystem().is_some_and(|name| name == "hid") {
                    Some(device.clone())
                } else {
                    device.parent()
                };
                let ours = hid.as_ref().is_some_and(|hid| {
                    hid.property_value("HID_PHYS")
                        .is_some_and(|value| value.to_str() == Some(phys.as_str()))
                });
                if !ours {
                    continue;
                }
                let is_hid = device.subsystem().is_some_and(|name| name == "hid");
                if is_hid && event.event_type() == udev::EventType::Bind {
                    if device.driver().is_none_or(|driver| driver != DRIVER_NAME) {
                        return Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "the kernel's Steam Controller driver didn't take the pad",
                        ));
                    }
                    bound = true;
                }
                if !is_hid
                    && event.event_type() == udev::EventType::Add
                    && let Some(node) = device.devnode()
                {
                    pad.raw_node = node.to_path_buf();
                }
            }
        }
    }

    /// The pad's raw node: where SDL and Steam read it. Its gamepad's
    /// event node is no place to point a person to: the driver takes it
    /// away while anything has the raw node open, and it says nothing
    /// while the driver has the pad in lizard mode, as it does until a
    /// player holds its menu button.
    pub(crate) fn node(&self) -> &Path {
        &self.raw_node
    }

    /// Makes the pad hold `state`: one state report, or none if nothing
    /// a report says differs and its motion is no fresh reading.
    pub(crate) fn write(&mut self, state: &PadState) -> io::Result<()> {
        let motion = state.motion();
        let fresh = motion.readings() != self.readings;
        if fresh {
            self.readings = motion.readings();
            self.time = self.time.wrapping_add(motion.interval_us());
        }
        let mut report = super::report(state, 0, self.time);
        if !fresh && self.sent == Some(report) {
            return Ok(());
        }
        self.sent = Some(report);
        self.sequence = self.sequence.wrapping_add(1);
        if let Some(sequence) = report.get_mut(1) {
            *sequence = self.sequence;
        }
        // UHID_INPUT2: type u32, size u16, then the report.
        let mut event = [0u8; 6 + STATE_SIZE];
        let (head, body) = event.split_at_mut(6);
        let (kind, size) = head.split_at_mut(4);
        kind.copy_from_slice(&UHID_INPUT2.to_ne_bytes());
        size.copy_from_slice(&STATE_LENGTH.to_ne_bytes());
        body.copy_from_slice(&report);
        self.uhid.write_all(&event)
    }

    /// Answers everything the kernel has waiting, passing a change of
    /// rumble to `handle`. Returns when nothing is left.
    pub(crate) fn serve(
        &mut self,
        mut handle: impl FnMut(Request) -> Result<(), FeedbackError>,
    ) -> io::Result<()> {
        loop {
            match self.uhid.read(self.event.as_mut_slice()) {
                Ok(0) => return Ok(()),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) => return Err(error),
            }
            let event = self.event.as_slice();
            match u32_at(event, 0) {
                // The driver let the pad go: a driver bound again, and
                // the pad carries on.
                UHID_STOP => self.stopped = true,
                // data[4096], then size u16.
                UHID_OUTPUT => {
                    let size = usize::from(u16_at(event, 4 + UHID_DATA)).min(UHID_DATA);
                    let level = event
                        .get(4..size.saturating_add(4))
                        .and_then(super::rumble_of);
                    if let Some(level) = level
                        && level != self.level
                    {
                        self.level = level;
                        // Nothing waits on this; a refusal has nowhere to go.
                        let _ = handle(Request::Level(level));
                    }
                }
                // id u32, then the report's number.
                UHID_GET_REPORT => {
                    let (id, number) = (u32_at(event, 4), event.get(8).copied().unwrap_or(0));
                    let report = self
                        .command
                        .filter(|_| number == FEATURE_PAD)
                        .map(|last| answer(&self.serial, &last));
                    let _ = handle(Request::Feature {
                        number,
                        set: false,
                        answered: report.is_some(),
                    });
                    // UHID_GET_REPORT_REPLY: type u32, id u32, err u16,
                    // size u16, then the report.
                    let mut reply = [0u8; 12 + FEATURE_SIZE];
                    let (head, body) = reply.split_at_mut(12);
                    if let Some(kind) = head.first_chunk_mut::<4>() {
                        *kind = UHID_GET_REPORT_REPLY.to_ne_bytes();
                    }
                    if let Some(place) = head.get_mut(4..8) {
                        place.copy_from_slice(&id.to_ne_bytes());
                    }
                    let length = if let Some(report) = report {
                        if let Some(place) = head.get_mut(10..12) {
                            place.copy_from_slice(&FEATURE_LENGTH.to_ne_bytes());
                        }
                        body.copy_from_slice(&report);
                        12 + FEATURE_SIZE
                    } else {
                        if let Some(place) = head.get_mut(8..10) {
                            place.copy_from_slice(&EIO.to_ne_bytes());
                        }
                        12
                    };
                    self.uhid.write_all(reply.get(..length).unwrap_or(&reply))?;
                }
                // id u32, the report's number, its type, size u16, then
                // the report.
                UHID_SET_REPORT => {
                    let (id, number) = (u32_at(event, 4), event.get(8).copied().unwrap_or(0));
                    let size = usize::from(u16_at(event, 10)).min(FEATURE_SIZE);
                    if number == FEATURE_PAD {
                        let mut command = [0u8; FEATURE_SIZE];
                        if let Some(data) = event.get(12..size.saturating_add(12)) {
                            for (place, byte) in command.iter_mut().zip(data) {
                                *place = *byte;
                            }
                        }
                        self.command = Some(command);
                    }
                    let _ = handle(Request::Feature {
                        number,
                        set: true,
                        answered: true,
                    });
                    // UHID_SET_REPORT_REPLY: type u32, id u32, err u16.
                    let mut reply = [0u8; 10];
                    if let Some(kind) = reply.first_chunk_mut::<4>() {
                        *kind = UHID_SET_REPORT_REPLY.to_ne_bytes();
                    }
                    if let Some(place) = reply.get_mut(4..8) {
                        place.copy_from_slice(&id.to_ne_bytes());
                    }
                    self.uhid.write_all(&reply)?;
                }
                _ => {}
            }
        }
    }
}

impl AsFd for Emulator {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.uhid.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(bytes: &[u8]) -> [u8; FEATURE_SIZE] {
        let mut command = [0u8; FEATURE_SIZE];
        command[..bytes.len()].copy_from_slice(bytes);
        command
    }

    /// The driver's two questions are answered as a pad answers them,
    /// and any other command is acknowledged as it was set.
    #[test]
    fn the_driver_s_questions_are_answered_and_the_rest_acknowledged() {
        let serial = answer("CAVIRTUAL0001", &command(&[1, 0xae, 0x16, 0x01]));
        assert_eq!(serial[..4], [1, 0xae, 0x15, 0x01]);
        assert_eq!(&serial[4..17], b"CAVIRTUAL0001");
        assert_eq!(serial[17], 0);
        // A serial too long for the driver's battery is cut.
        let long = answer("0123456789ABCDEFGH", &command(&[1, 0xae, 0x16, 0x01]));
        assert_eq!(&long[4..17], b"0123456789ABC");
        assert_eq!(long[17], 0);

        let attributes = answer("X", &command(&[1, 0x83]));
        assert_eq!(attributes[..3], [1, 0x83, 30]);
        assert_eq!(attributes[3..8], [1, 0x02, 0x13, 0, 0]);
        // The interval the driver reads, 4000 µs, is the last.
        assert_eq!(attributes[28..33], [11, 0xa0, 0x0f, 0, 0]);

        let settings = command(&[1, 0x87, 3, 9, 0, 0]);
        assert_eq!(answer("X", &settings), settings);
    }

    /// The descriptor declares each report at the length it is sent or
    /// taken at, its number aside, and nothing of a keyboard or pointer.
    #[test]
    fn the_descriptor_declares_each_report_at_its_length() {
        let descriptor = descriptor();
        let declares = |id: u8, after: u8, main: u8| {
            descriptor
                .windows(8)
                .any(|window| window == [0x85, id, 0x95, after, 0x09, id, main | 1, 0x02])
        };
        assert!(declares(0x42, 53, INPUT));
        assert!(declares(0x45, 45, INPUT));
        assert!(declares(0x43, 14, INPUT));
        assert!(declares(0x79, 1, INPUT));
        assert!(declares(0x80, 9, OUTPUT));
        assert!(declares(0x81, 7, OUTPUT));
        assert!(declares(0x01, 63, FEATURE));
        // One application collection, the vendor's.
        assert_eq!(descriptor.iter().filter(|byte| **byte == 0xa1).count(), 1);
        assert_eq!(descriptor[..3], [0x06, 0x00, 0xff]);
        assert_eq!(descriptor.last(), Some(&0xc0));
    }
}
