//! A virtual DualSense, made through uhid.
//!
//! SDL and Steam read a DualSense through its raw HID node and ignore an
//! evdev pad carrying its numbers, so passing as one takes a HID device:
//! the kernel's PlayStation driver binds to it as to the real thing. The
//! emulator presents a real pad's report descriptor, answers the three
//! feature reports the driver reads as it binds, turns pad state into
//! the pad's input report, and reads rumble out of the output reports it
//! is sent.
//!
//! The report descriptor is a retail pad's, carried in this crate's data;
//! one copied from a connected pad can be used in its place. The
//! firmware report says only what the kernel's driver and SDL go by, the
//! pairing report carries the virtual pad's own address, and the
//! calibration is synthesized as neutral: the pads behind it are
//! calibrated by their own drivers.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use ctrlassist_core::{FeedbackError, Level, PadState, gamepad};
use rustix::event::{PollFd, PollFlags, poll};
use rustix::fs::OFlags;

use crate::sink::{OWN_MARKER, Request, hat};
use crate::{hidraw, hotplug};

/// Sony's vendor number.
pub(crate) const VENDOR: u16 = 0x054c;
/// The DualSense's product number.
pub(crate) const PRODUCT: u16 = 0x0ce6;
/// The version a DualSense reports over USB, which SDL builds its id of
/// the pad from.
const VERSION: u32 = 0x8111;
/// The name a DualSense reports over USB.
pub(crate) const NAME: &str = "Sony Interactive Entertainment DualSense Wireless Controller";
/// The bus type for USB, from linux/input.h.
const BUS_USB: u16 = 3;
/// Where the kernel's PlayStation driver is, once its module is loaded.
const DRIVER: &str = "/sys/bus/hid/drivers/playstation";

const INPUT_REPORT: u8 = 0x01;
const INPUT_SIZE: usize = 64;
const OUTPUT_REPORT: u8 = 0x02;
const CALIBRATION: u8 = 0x05;
const CALIBRATION_SIZE: usize = 41;
const PAIRING: u8 = 0x09;
const PAIRING_SIZE: usize = 20;
const FIRMWARE: u8 = 0x20;
const FIRMWARE_SIZE: usize = 64;

// Event types from linux/uhid.h.
const UHID_STOP: u32 = 3;
const UHID_OUTPUT: u32 = 6;
const UHID_GET_REPORT: u32 = 9;
const UHID_GET_REPORT_REPLY: u32 = 10;
const UHID_CREATE2: u32 = 11;
const UHID_INPUT2: u32 = 12;
const UHID_SET_REPORT: u32 = 13;
const UHID_SET_REPORT_REPLY: u32 = 14;
/// The most data one uhid event carries.
const UHID_DATA: usize = 4096;
/// The size of the largest uhid event, `UHID_CREATE2`.
const UHID_EVENT: usize = 4 + 128 + 64 + 64 + 2 + 2 + 4 + 4 + 4 + 4 + UHID_DATA;
/// The error a request for a report the emulator doesn't have is given.
const EIO: u16 = 5;

/// What a virtual DualSense is made from, copied from a connected one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DualSenseData {
    /// The pad's report descriptor over USB.
    pub descriptor: Vec<u8>,
    /// The pad's firmware report, from which the kernel's driver picks
    /// how it sends rumble.
    pub firmware: Vec<u8>,
}

/// The firmware update version the shipped data reports: the first at
/// which both the kernel's driver and SDL use the pad's newer rumble.
const SHIPPED_UPDATE: u16 = 0x0224;

impl DualSenseData {
    /// What a virtual DualSense is made from where none has been copied:
    /// a retail pad's report descriptor, and a firmware report that says
    /// only which rumble the pad takes.
    ///
    /// # Panics
    ///
    /// Never: the data is this crate's own, and a test reads it whole.
    #[must_use]
    pub fn shipped() -> Self {
        let descriptor = include_str!("../data/dualsense-usb.hex")
            .lines()
            .filter(|line| !line.starts_with('#'))
            .flat_map(|line| {
                (0..line.len() / 2).map(move |at| {
                    u8::from_str_radix(&line[at * 2..at * 2 + 2], 16)
                        .expect("the shipped descriptor is hex digits, which a test checks")
                })
            })
            .collect();
        let mut firmware = vec![0u8; FIRMWARE_SIZE];
        firmware[0] = FIRMWARE;
        firmware[44..46].copy_from_slice(&SHIPPED_UPDATE.to_le_bytes());
        Self {
            descriptor,
            firmware,
        }
    }

    /// Copies what a virtual DualSense needs from a connected one's raw
    /// HID node.
    ///
    /// # Errors
    ///
    /// Fails if the node isn't a DualSense's connected over USB (over
    /// Bluetooth its descriptor is another one), or can't be read.
    pub fn read(hidraw: BorrowedFd<'_>) -> io::Result<Self> {
        let (bus, vendor, product) = hidraw::info(hidraw)?;
        if (bus, vendor, product) != (BUS_USB, VENDOR, PRODUCT) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a DualSense connected over USB",
            ));
        }
        let data = Self {
            descriptor: hidraw::descriptor(hidraw)?,
            firmware: hidraw::feature(hidraw, FIRMWARE, FIRMWARE_SIZE)?,
        };
        if data.is_whole() {
            Ok(data)
        } else {
            Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the pad's descriptor or firmware report isn't what a DualSense gives",
            ))
        }
    }

    /// Whether a virtual pad can be made from this: a descriptor of a
    /// size the kernel takes, and a whole firmware report.
    #[must_use]
    pub fn is_whole(&self) -> bool {
        !self.descriptor.is_empty()
            && self.descriptor.len() <= UHID_DATA
            && self.firmware.len() == FIRMWARE_SIZE
            && self.firmware.first() == Some(&FIRMWARE)
    }
}

/// An address as the kernel writes one: `aa:bb:cc:dd:ee:ff`.
pub(crate) fn address_text(address: [u8; 6]) -> String {
    let [a, b, c, d, e, f] = address;
    format!("{a:02x}:{b:02x}:{c:02x}:{d:02x}:{e:02x}:{f:02x}")
}

/// The pairing report: the pad's address, last byte first.
fn pairing(address: [u8; 6]) -> [u8; PAIRING_SIZE] {
    let mut report = [0u8; PAIRING_SIZE];
    report[0] = PAIRING;
    for (place, byte) in report[1..7].iter_mut().zip(address.iter().rev()) {
        *place = *byte;
    }
    report
}

/// A calibration report that changes nothing: no bias, and ranges that
/// are whole and symmetric, so the driver has nothing to warn of.
fn calibration() -> [u8; CALIBRATION_SIZE] {
    // Gyro: three biases, then plus and minus for pitch, yaw and roll,
    // then the speed either way. Accelerometer: plus and minus for x, y
    // and z, at the driver's own resolution of 8192 to the g.
    const VALUES: [i16; 17] = [
        0, 0, 0, 8839, -8839, 8839, -8839, 8839, -8839, 540, 540, 8192, -8192, 8192, -8192, 8192,
        -8192,
    ];
    let mut report = [0u8; CALIBRATION_SIZE];
    report[0] = CALIBRATION;
    for (place, value) in report[1..].as_chunks_mut::<2>().0.iter_mut().zip(VALUES) {
        *place = value.to_le_bytes();
    }
    report
}

/// A stick's value on the wire: 0 to 255, at rest on 128.
fn stick(value: f32) -> u8 {
    (128.0 + value * if value < 0.0 { 128.0 } else { 127.0 })
        .round()
        .clamp(0.0, 255.0) as u8
}

fn trigger(value: f32) -> u8 {
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}

/// The input report a DualSense sends over USB for `state`, with its
/// touchpad untouched, its sensors at rest and its battery full.
fn input_report(state: &PadState, sequence: u8) -> [u8; INPUT_SIZE] {
    let pressed = |control| state.get(control) > 0.0;
    let bit = |control, bit: u8| if pressed(control) { bit } else { 0 };
    let mut report = [0u8; INPUT_SIZE];
    report[0] = INPUT_REPORT;
    report[1] = stick(state.get(gamepad::LEFT_X));
    report[2] = stick(state.get(gamepad::LEFT_Y));
    report[3] = stick(state.get(gamepad::RIGHT_X));
    report[4] = stick(state.get(gamepad::RIGHT_Y));
    report[5] = trigger(state.get(gamepad::LEFT_TRIGGER));
    report[6] = trigger(state.get(gamepad::RIGHT_TRIGGER));
    report[7] = sequence;
    // The directional pad as one of eight directions, clockwise from up,
    // or eight for none.
    let direction = match (
        hat(state, gamepad::DPAD_LEFT, gamepad::DPAD_RIGHT),
        hat(state, gamepad::DPAD_UP, gamepad::DPAD_DOWN),
    ) {
        (0, -1) => 0,
        (1, -1) => 1,
        (1, 0) => 2,
        (1, 1) => 3,
        (0, 1) => 4,
        (-1, 1) => 5,
        (-1, 0) => 6,
        (-1, -1) => 7,
        _ => 8,
    };
    report[8] = direction
        | bit(gamepad::WEST, 0x10)
        | bit(gamepad::SOUTH, 0x20)
        | bit(gamepad::EAST, 0x40)
        | bit(gamepad::NORTH, 0x80);
    // The digital trigger buttons follow the trigger axes.
    report[9] = bit(gamepad::LEFT_BUMPER, 0x01)
        | bit(gamepad::RIGHT_BUMPER, 0x02)
        | bit(gamepad::LEFT_TRIGGER, 0x04)
        | bit(gamepad::RIGHT_TRIGGER, 0x08)
        | bit(gamepad::SELECT, 0x10)
        | bit(gamepad::START, 0x20)
        | bit(gamepad::LEFT_THUMB, 0x40)
        | bit(gamepad::RIGHT_THUMB, 0x80);
    report[10] = bit(gamepad::MODE, 0x01);
    // Neither touch point is in contact.
    report[33] = 0x80;
    report[37] = 0x80;
    // Charged and full.
    report[53] = 0x2a;
    report
}

/// The rumble the pad is left at by an output report. Rumble is marked
/// in one of two places, by the pad's firmware, with the levels in the
/// same place either way. A report that marks it in neither ends it: a
/// real DualSense stops rumbling at one, which is how SDL stops a rumble,
/// by sending its next report without the mark.
fn rumble_of(report: &[u8]) -> Option<Level> {
    let [OUTPUT_REPORT, flags0, _, weak, strong, ..] = *report else {
        return None;
    };
    let flags2 = report.get(39).copied().unwrap_or(0);
    if flags0 & 0x01 == 0 && flags2 & 0x04 == 0 {
        return Some(Level::default());
    }
    Some(Level {
        strong: u16::from(strong) * 257,
        weak: u16::from(weak) * 257,
    })
}

/// The size of the output report a DualSense takes over USB.
const OUTPUT_SIZE: usize = 48;
/// The update version at which the kernel's driver takes the pad to use
/// its newer rumble: 2.21.
const NEWER_RUMBLE: u16 = 0x0215;

/// An output report with its rumble set to `level`, marked the way a pad
/// of that firmware takes it, whatever rumble it carried: or with rumble
/// marked nowhere for none, which is how a pad is told to stop.
fn with_rumble(mut report: [u8; OUTPUT_SIZE], level: Level, newer: bool) -> [u8; OUTPUT_SIZE] {
    report[0] = OUTPUT_REPORT;
    report[1] &= !0x03;
    report[39] &= !0x04;
    (report[3], report[4]) = (0, 0);
    if level != Level::default() {
        // The haptics are switched to rumble, and rumble marked.
        report[1] |= if newer { 0x02 } else { 0x03 };
        report[39] |= if newer { 0x04 } else { 0 };
        report[3] = (level.weak >> 8) as u8;
        report[4] = (level.strong >> 8) as u8;
    }
    report
}

/// The DualShock 4's product numbers: the first model and the second.
const DUALSHOCK4: [u16; 2] = [0x05c4, 0x09cc];
/// The bus type for Bluetooth, from linux/input.h.
const BUS_BLUETOOTH: u16 = 5;
/// The size of the output report a DualShock 4 takes over Bluetooth.
const DS4_BLUETOOTH_SIZE: usize = 78;

/// The CRC-32 of `seed` followed by `data`, as a PlayStation pad checks
/// a report over Bluetooth.
fn crc32(seed: u8, data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in std::iter::once(&seed).chain(data) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

/// The output report that sets a DualShock 4's rumble, and its lightbar
/// if a color is given, with the report's length. The pad takes what a
/// report marks and leaves the rest as it was. Over Bluetooth the
/// report says how often the pad reports, which is kept at the kernel
/// driver's own choice, and ends in a checksum.
fn dualshock4_report(
    level: Level,
    color: Option<[u8; 3]>,
    bluetooth: bool,
) -> ([u8; DS4_BLUETOOTH_SIZE], usize) {
    let [red, green, blue] = color.unwrap_or_default();
    // What is marked: the motors, and the lightbar if there is a color.
    let marks = 0x01 | if color.is_some() { 0x02 } else { 0 };
    let common = [
        marks,
        0,
        0,
        (level.weak >> 8) as u8,
        (level.strong >> 8) as u8,
        red,
        green,
        blue,
    ];
    let mut report = [0u8; DS4_BLUETOOTH_SIZE];
    if !bluetooth {
        report[0] = 0x05;
        report[1..9].copy_from_slice(&common);
        return (report, 32);
    }
    report[0] = 0x11;
    // A HID report with a checksum, at one report every 4 ms.
    report[1] = 0x80 | 0x40 | 4;
    report[3..11].copy_from_slice(&common);
    let checksum = crc32(0xa2, &report[..DS4_BLUETOOTH_SIZE - 4]);
    report[DS4_BLUETOOTH_SIZE - 4..].copy_from_slice(&checksum.to_le_bytes());
    (report, DS4_BLUETOOTH_SIZE)
}

/// Which pad a raw node is, for what it is written.
enum Kind {
    /// A DualSense over USB, and whether its firmware takes the newer
    /// rumble.
    DualSense { newer: bool },
    /// A DualShock 4, the bus it is on, and the lightbar color a game
    /// last asked for, if one has.
    DualShock4 {
        bluetooth: bool,
        color: Option<[u8; 3]>,
    },
}

/// A real PlayStation pad's raw HID node, as somewhere to write: the pad
/// is sent whole output reports, so that what a game sends a virtual
/// DualSense beyond rumble reaches the pad behind it. A DualSense gets
/// the game's report as it is, lights and trigger effects; a DualShock 4
/// gets the lightbar's color. Rumble then goes this way too, and not
/// through the kernel's own effects, so every report written carries the
/// level the pad should be at: a DualSense takes a report without the
/// rumble as the rumble's end, and the kernel's driver sends a DualShock
/// 4 its own lightbar color with every rumble.
pub(crate) struct RawOutput {
    raw: File,
    kind: Kind,
    level: Level,
}

impl RawOutput {
    /// Takes a pad's raw node, if it is one this knows how to write to:
    /// a DualSense's over USB (over Bluetooth its report is another one),
    /// or a DualShock 4's over either.
    pub(crate) fn open(raw: std::os::fd::OwnedFd) -> io::Result<Self> {
        let (bus, vendor, product) = hidraw::info(raw.as_fd())?;
        let kind = if (bus, vendor, product) == (BUS_USB, VENDOR, PRODUCT) {
            let firmware = hidraw::feature(raw.as_fd(), FIRMWARE, FIRMWARE_SIZE)?;
            let update = u16::from_le_bytes([
                firmware.get(44).copied().unwrap_or(0),
                firmware.get(45).copied().unwrap_or(0),
            ]);
            Kind::DualSense {
                newer: update >= NEWER_RUMBLE,
            }
        } else if vendor == VENDOR
            && DUALSHOCK4.contains(&product)
            && (bus == BUS_USB || bus == BUS_BLUETOOTH)
        {
            Kind::DualShock4 {
                bluetooth: bus == BUS_BLUETOOTH,
                color: None,
            }
        } else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "not a DualSense over USB or a DualShock 4",
            ));
        };
        Ok(Self {
            raw: File::from(raw),
            kind,
            level: Level::default(),
        })
    }

    /// Whether a pad with these numbers is one [`open`](Self::open)
    /// takes.
    pub(crate) fn takes(bus: u16, vendor: u16, product: u16) -> bool {
        vendor == VENDOR
            && ((bus == BUS_USB && product == PRODUCT)
                || (DUALSHOCK4.contains(&product) && (bus == BUS_USB || bus == BUS_BLUETOOTH)))
    }

    /// Writes the pad a report of what it should be at; for a DualSense,
    /// `game` is the game's report to carry it.
    fn write(&mut self, game: [u8; OUTPUT_SIZE]) -> io::Result<()> {
        match self.kind {
            Kind::DualSense { newer } => self.raw.write_all(&with_rumble(game, self.level, newer)),
            Kind::DualShock4 { bluetooth, color } => {
                let (report, length) = dualshock4_report(self.level, color, bluetooth);
                self.raw.write_all(&report[..length])
            }
        }
    }

    /// Makes the pad's motors run at `level`, with nothing else changed.
    pub(crate) fn rumble(&mut self, level: Level) -> io::Result<()> {
        if level == self.level {
            return Ok(());
        }
        self.level = level;
        self.write([0; OUTPUT_SIZE])
    }

    /// Sends the pad what a game sent a virtual DualSense, with the
    /// rumble the pad should be at in place of the report's: the whole
    /// report for a DualSense, and for a DualShock 4 the lightbar's
    /// color, if the report sets it.
    pub(crate) fn pass(&mut self, report: &[u8]) -> io::Result<()> {
        let mut whole = [0u8; OUTPUT_SIZE];
        for (place, byte) in whole.iter_mut().zip(report) {
            *place = *byte;
        }
        if let Kind::DualShock4 { color, .. } = &mut self.kind {
            // The lightbar's color, where the report says to take it.
            if whole[0] != OUTPUT_REPORT || whole[2] & 0x04 == 0 {
                return Ok(());
            }
            *color = Some([whole[45], whole[46], whole[47]]);
        }
        self.write(whole)
    }
}

/// Appends `text` as a fixed-width, zero-terminated field.
fn field(event: &mut Vec<u8>, text: &str, width: usize) {
    let mut bytes = text.as_bytes().to_vec();
    bytes.truncate(width - 1);
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
    event.extend_from_slice(&u32::from(PRODUCT).to_ne_bytes());
    event.extend_from_slice(&VERSION.to_ne_bytes());
    event.extend_from_slice(&0u32.to_ne_bytes());
    event.extend_from_slice(&descriptor[..usize::from(size)]);
    event.resize(UHID_EVENT, 0);
    event
}

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

/// What one uhid event asks of the emulator.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Asked {
    /// The kernel has let the device go.
    Stop,
    /// An output report set rumble.
    Rumble(Level),
    /// A feature report is wanted: the request's number, and the report's.
    Get(u32, u8),
    /// A report was set, which is only to be acknowledged.
    Set(u32),
    /// Nothing the emulator acts on.
    Nothing,
}

fn asked(event: &[u8]) -> Asked {
    match u32_at(event, 0) {
        UHID_STOP => Asked::Stop,
        // data[4096], then size u16.
        UHID_OUTPUT => {
            let size = usize::from(u16_at(event, 4 + UHID_DATA)).min(UHID_DATA);
            event
                .get(4..4 + size)
                .and_then(rumble_of)
                .map_or(Asked::Nothing, Asked::Rumble)
        }
        // id u32, then the report's number.
        UHID_GET_REPORT => Asked::Get(u32_at(event, 4), event.get(8).copied().unwrap_or(0)),
        UHID_SET_REPORT => Asked::Set(u32_at(event, 4)),
        _ => Asked::Nothing,
    }
}

/// A virtual DualSense.
pub(crate) struct Emulator {
    uhid: File,
    event_node: PathBuf,
    held: PadState,
    sequence: u8,
    pairing: [u8; PAIRING_SIZE],
    firmware: [u8; FIRMWARE_SIZE],
    calibration: [u8; CALIBRATION_SIZE],
    /// The rumble last asked for, so that a report that leaves it as it
    /// is isn't taken for a new level.
    level: Level,
    /// Whether the driver has let the pad go since this was last asked.
    stopped: bool,
    /// The last output report a game sent, and whether anything has
    /// taken it yet.
    output: [u8; OUTPUT_SIZE],
    sent: bool,
    fresh: bool,
    /// Kept between calls so that serving allocates nothing.
    event: Box<[u8; UHID_EVENT]>,
}

impl Emulator {
    /// Creates the pad, answers the driver as it binds, and waits for
    /// the pad's event node. A driver that starts on the pad and then
    /// refuses it, as for an address it already has, lets the pad go
    /// before it is bound, which ends the wait with an error.
    pub(crate) fn create(data: &DualSenseData, address: [u8; 6]) -> io::Result<Self> {
        let firmware: [u8; FIRMWARE_SIZE] = data
            .firmware
            .as_slice()
            .try_into()
            .ok()
            .filter(|_| data.is_whole())
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "the copied pad's data isn't whole",
                )
            })?;
        // With the driver there, it is the one to bind. Without, the
        // generic driver would, and no pad of the driver's would come.
        if !Path::new(DRIVER).exists() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the kernel's PlayStation driver isn't loaded",
            ));
        }
        // The process and a count tell virtual pads apart in the marker.
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let phys = format!("{OWN_MARKER}{}/hid/{number}", std::process::id());
        // Listening before the pad is made, so its arrival isn't missed.
        let watch = udev::MonitorBuilder::new()?
            .match_subsystem("hid")?
            .match_subsystem("input")?
            .listen()?;
        let uhid = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags((OFlags::NONBLOCK | OFlags::CLOEXEC).bits() as i32)
            .open("/dev/uhid")?;
        let mut pad = Self {
            uhid,
            event_node: PathBuf::new(),
            held: PadState::NEUTRAL,
            sequence: 0,
            pairing: pairing(address),
            firmware,
            calibration: calibration(),
            level: Level::default(),
            stopped: false,
            output: [0; OUTPUT_SIZE],
            sent: false,
            fresh: false,
            event: Box::new([0; UHID_EVENT]),
        };
        let uniq = address_text(address);
        pad.uhid
            .write_all(&create2(&phys, &uniq, &data.descriptor))?;
        // The driver reads its three feature reports as it binds, then
        // registers the pad's input devices, and only once it is bound
        // does the kernel pass on what the pad reports. All of that is
        // waited for here, as events, however long the kernel takes.
        let mut bound = false;
        loop {
            if bound && !pad.event_node.as_os_str().is_empty() {
                return Ok(pad);
            }
            let mut fds = [
                PollFd::new(&pad.uhid, PollFlags::IN),
                PollFd::new(&watch, PollFlags::IN),
            ];
            poll(&mut fds, None)?;
            pad.serve(|_| Ok(()))?;
            if std::mem::take(&mut pad.stopped) && !bound {
                // It bound and then gave the pad up, as for an address
                // it already has.
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
                    bound = true;
                }
                // The driver gives its input devices the pad's address
                // and no `phys`; the pad itself is the one of its name
                // alone, beside its sensors and its touchpad.
                let node = hotplug::found(&device).filter(|found| {
                    found.identifiers.address.as_deref() == Some(uniq.as_str())
                        && found.identifiers.name == NAME
                });
                if let Some(found) = node {
                    pad.event_node = found.node;
                }
            }
        }
    }

    pub(crate) fn event_node(&self) -> &Path {
        &self.event_node
    }

    /// Makes the pad hold `state`: one input report, or none if nothing
    /// a report says differs.
    pub(crate) fn write(&mut self, state: &PadState) -> io::Result<()> {
        if input_report(state, 0) != input_report(&self.held, 0) {
            self.sequence = self.sequence.wrapping_add(1);
            let report = input_report(state, self.sequence);
            // UHID_INPUT2: type u32, size u16, then the report.
            let mut event = [0u8; 6 + INPUT_SIZE];
            event[..4].copy_from_slice(&UHID_INPUT2.to_ne_bytes());
            event[4..6].copy_from_slice(&(INPUT_SIZE as u16).to_ne_bytes());
            event[6..].copy_from_slice(&report);
            self.uhid.write_all(&event)?;
        }
        self.held = *state;
        Ok(())
    }

    /// Keeps the event just read if it is an output report: what the
    /// pads behind this one are sent.
    fn keep_output(&mut self) {
        let event = self.event.as_slice();
        if u32_at(event, 0) != UHID_OUTPUT || event.get(4) != Some(&OUTPUT_REPORT) {
            return;
        }
        let size = usize::from(u16_at(event, 4 + UHID_DATA)).min(OUTPUT_SIZE);
        let mut output = [0u8; OUTPUT_SIZE];
        if let Some(report) = event.get(4..4 + size) {
            for (place, byte) in output.iter_mut().zip(report) {
                *place = *byte;
            }
        }
        self.output = output;
        (self.sent, self.fresh) = (true, true);
    }

    /// The last output report a game sent, if one came since this was
    /// last asked.
    pub(crate) fn take_output(&mut self) -> Option<&[u8]> {
        std::mem::take(&mut self.fresh).then_some(self.output.as_slice())
    }

    /// The last output report a game sent, if any ever came.
    pub(crate) fn last_output(&self) -> Option<&[u8]> {
        self.sent.then_some(self.output.as_slice())
    }

    fn feature(&self, number: u8) -> Option<&[u8]> {
        match number {
            PAIRING => Some(&self.pairing),
            FIRMWARE => Some(&self.firmware),
            CALIBRATION => Some(&self.calibration),
            _ => None,
        }
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
            self.keep_output();
            match asked(self.event.as_slice()) {
                // The driver let the pad go. That is the pad's end only
                // while it is being made; later it is a driver bound
                // again, as hiding the pad does, and the pad carries on.
                Asked::Stop => self.stopped = true,
                Asked::Rumble(level) if level != self.level => {
                    self.level = level;
                    // Nothing waits on this; a refusal has nowhere to go.
                    let _ = handle(Request::Level(level));
                }
                Asked::Get(id, number) => {
                    // UHID_GET_REPORT_REPLY: type u32, id u32, err u16,
                    // size u16, then the report.
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
                Asked::Set(id) => {
                    // UHID_SET_REPORT_REPLY: type u32, id u32, err u16.
                    let mut reply = [0u8; 10];
                    reply[..4].copy_from_slice(&UHID_SET_REPORT_REPLY.to_ne_bytes());
                    reply[4..8].copy_from_slice(&id.to_ne_bytes());
                    self.uhid.write_all(&reply)?;
                }
                Asked::Rumble(_) | Asked::Nothing => {}
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

    /// The shipped data is whole, and declares the reports the emulator
    /// and the kernel's driver exchange.
    #[test]
    fn the_shipped_data_makes_a_pad() {
        let data = DualSenseData::shipped();
        assert!(data.is_whole());
        assert_eq!(data.descriptor.len(), 273);
        // A gamepad on the generic desktop page, then report 1.
        assert_eq!(
            data.descriptor[..8],
            [0x05, 0x01, 0x09, 0x05, 0xa1, 0x01, 0x85, 0x01]
        );
        for report in [INPUT_REPORT, OUTPUT_REPORT, CALIBRATION, PAIRING, FIRMWARE] {
            let declared = data
                .descriptor
                .windows(2)
                .any(|pair| pair == [0x85, report]);
            assert!(declared, "report {report:#04x}");
        }
        assert_eq!(
            u16::from_le_bytes([data.firmware[44], data.firmware[45]]),
            0x0224
        );
    }

    #[test]
    fn a_pad_at_rest_is_the_report_a_dualsense_sends_untouched() {
        let report = input_report(&PadState::NEUTRAL, 7);
        assert_eq!(report[..11], [1, 128, 128, 128, 128, 0, 0, 7, 8, 0, 0]);
        // Neither touch point in contact, and a full battery.
        assert_eq!((report[33], report[37], report[53]), (0x80, 0x80, 0x2a));
        let rest: Vec<usize> = (11..INPUT_SIZE)
            .filter(|index| report[*index] != 0)
            .collect();
        assert_eq!(rest, [33, 37, 53], "nothing else is set");
    }

    #[test]
    fn every_gamepad_control_has_its_place_in_the_report() {
        let held = |control| input_report(&PadState::NEUTRAL.with(control, 1.0), 0);
        let face = [
            (gamepad::WEST, 0x18),
            (gamepad::SOUTH, 0x28),
            (gamepad::EAST, 0x48),
            (gamepad::NORTH, 0x88),
        ];
        for (control, byte) in face {
            assert_eq!(held(control)[8], byte, "{control:?}");
        }
        let second = [
            (gamepad::LEFT_BUMPER, 0x01),
            (gamepad::RIGHT_BUMPER, 0x02),
            (gamepad::SELECT, 0x10),
            (gamepad::START, 0x20),
            (gamepad::LEFT_THUMB, 0x40),
            (gamepad::RIGHT_THUMB, 0x80),
        ];
        for (control, byte) in second {
            assert_eq!(held(control)[9], byte, "{control:?}");
        }
        assert_eq!(held(gamepad::MODE)[10], 0x01);
        // A trigger is its axis and its button at once.
        let left = held(gamepad::LEFT_TRIGGER);
        assert_eq!((left[5], left[9]), (255, 0x04));
        let right = held(gamepad::RIGHT_TRIGGER);
        assert_eq!((right[6], right[9]), (255, 0x08));
        // The sticks, end to end.
        let ends = PadState::NEUTRAL
            .with(gamepad::LEFT_X, -1.0)
            .with(gamepad::LEFT_Y, 1.0)
            .with(gamepad::RIGHT_X, 0.5)
            .with(gamepad::RIGHT_Y, -0.5);
        assert_eq!(input_report(&ends, 0)[1..5], [0, 255, 192, 64]);
        assert_eq!(
            stick(f32::NAN),
            0,
            "what isn't a number is an end, not a panic"
        );
    }

    #[test]
    fn the_directional_pad_is_one_of_eight_directions_or_none() {
        let hat = |held: &[ctrlassist_core::Control]| {
            let state = held.iter().fold(PadState::NEUTRAL, |state, control| {
                state.with(*control, 1.0)
            });
            input_report(&state, 0)[8]
        };
        let (up, right, down, left) = (
            gamepad::DPAD_UP,
            gamepad::DPAD_RIGHT,
            gamepad::DPAD_DOWN,
            gamepad::DPAD_LEFT,
        );
        assert_eq!(hat(&[up]), 0);
        assert_eq!(hat(&[up, right]), 1);
        assert_eq!(hat(&[right]), 2);
        assert_eq!(hat(&[right, down]), 3);
        assert_eq!(hat(&[down]), 4);
        assert_eq!(hat(&[down, left]), 5);
        assert_eq!(hat(&[left]), 6);
        assert_eq!(hat(&[left, up]), 7);
        assert_eq!(hat(&[]), 8);
        assert_eq!(hat(&[left, right]), 8, "opposed directions cancel");
    }

    #[test]
    fn rumble_is_read_from_an_output_report_in_either_of_the_driver_s_layouts() {
        let mut report = [0u8; 63];
        report[0] = OUTPUT_REPORT;
        (report[3], report[4]) = (0x40, 0x80);
        assert_eq!(
            rumble_of(&report),
            Some(Level::default()),
            "with rumble marked nowhere, the pad doesn't rumble"
        );
        let level = Level {
            strong: 0x8080,
            weak: 0x4040,
        };
        report[1] = 0x03;
        assert_eq!(rumble_of(&report), Some(level), "the older layout");
        report[1] = 0x02;
        report[39] = 0x04;
        assert_eq!(rumble_of(&report), Some(level), "the newer layout");
        (report[3], report[4]) = (0xff, 0xff);
        let full = rumble_of(&report).unwrap();
        assert_eq!((full.strong, full.weak), (u16::MAX, u16::MAX));
        // Another report, and one cut short.
        report[0] = 0x31;
        assert_eq!(rumble_of(&report), None);
        assert_eq!(rumble_of(&[OUTPUT_REPORT, 0x01]), None);
        assert_eq!(
            rumble_of(&[OUTPUT_REPORT, 0x01, 0, 9, 9]).unwrap().strong,
            9 * 257
        );
    }

    #[test]
    fn a_report_for_a_real_pad_carries_the_rumble_it_should_be_at() {
        // A game's report that sets a light and its own rumble.
        let mut game = [0u8; OUTPUT_SIZE];
        game[0] = OUTPUT_REPORT;
        (game[1], game[2], game[3], game[4]) = (0x03, 0x04, 0x11, 0x22);
        (game[39], game[45]) = (0x06, 0xff);
        let level = Level {
            strong: 0x8000,
            weak: 0x4000,
        };
        let newer = with_rumble(game, level, true);
        assert_eq!(newer[1..5], [0x02, 0x04, 0x40, 0x80]);
        assert_eq!((newer[39], newer[45]), (0x06, 0xff), "the light is kept");
        assert_eq!(rumble_of(&newer), Some(level_of(0x80, 0x40)));
        let older = with_rumble(game, level, false);
        assert_eq!(older[1..5], [0x03, 0x04, 0x40, 0x80]);
        assert_eq!(older[39], 0x02, "rumble is marked in the older place alone");
        // At no rumble, it is marked nowhere, which stops the pad.
        let none = with_rumble(game, Level::default(), true);
        assert_eq!(none[1..5], [0x00, 0x04, 0, 0]);
        assert_eq!((none[39], none[45]), (0x02, 0xff));
        assert_eq!(rumble_of(&none), Some(Level::default()));
        // Rumble alone is a report that changes nothing else.
        let alone = with_rumble([0; OUTPUT_SIZE], level, true);
        assert_eq!(alone[1], 0x02);
        assert!(
            alone[5..39]
                .iter()
                .chain(&alone[40..])
                .all(|byte| *byte == 0)
        );
    }

    #[test]
    fn a_dualshock_4_is_sent_its_rumble_and_the_lightbar_s_color() {
        // The check value every CRC-32 is known by, with no seed byte
        // of its own: the seed is the first byte.
        assert_eq!(crc32(b'1', b"23456789"), 0xcbf4_3926);
        let level = Level {
            strong: 0x8000,
            weak: 0x4000,
        };
        let (usb, length) = dualshock4_report(level, None, false);
        assert_eq!(length, 32);
        assert_eq!(usb[..9], [0x05, 0x01, 0, 0, 0x40, 0x80, 0, 0, 0]);
        assert!(usb[9..].iter().all(|byte| *byte == 0));
        let (usb, _) = dualshock4_report(level, Some([1, 2, 3]), false);
        assert_eq!(usb[..9], [0x05, 0x03, 0, 0, 0x40, 0x80, 1, 2, 3]);
        let (bluetooth, length) = dualshock4_report(Level::default(), Some([9, 8, 7]), true);
        assert_eq!(length, 78);
        assert_eq!(bluetooth[..11], [0x11, 0xc4, 0, 0x03, 0, 0, 0, 0, 9, 8, 7]);
        let checksum = u32::from_le_bytes(bluetooth[74..].try_into().unwrap());
        assert_eq!(checksum, crc32(0xa2, &bluetooth[..74]));
        assert!(RawOutput::takes(BUS_USB, VENDOR, PRODUCT));
        assert!(!RawOutput::takes(BUS_BLUETOOTH, VENDOR, PRODUCT));
        assert!(RawOutput::takes(BUS_BLUETOOTH, VENDOR, 0x05c4));
        assert!(RawOutput::takes(BUS_USB, VENDOR, 0x09cc));
        assert!(!RawOutput::takes(BUS_USB, 0x045e, 0x05c4));
    }

    fn level_of(strong: u8, weak: u8) -> Level {
        Level {
            strong: u16::from(strong) * 257,
            weak: u16::from(weak) * 257,
        }
    }

    #[test]
    fn the_pairing_report_carries_the_pad_s_own_address_last_byte_first() {
        let report = pairing([0x02, 0xca, 0x55, 0x10, 0x20, 0x30]);
        assert_eq!(report[..7], [PAIRING, 0x30, 0x20, 0x10, 0x55, 0xca, 0x02]);
        assert!(report[7..].iter().all(|byte| *byte == 0));
        assert_eq!(
            address_text([0x02, 0xca, 0x55, 0x10, 0x20, 0x30]),
            "02:ca:55:10:20:30"
        );
    }

    /// As the kernel's driver reads it: no bias, and for each axis a
    /// range that is whole, so nothing is scaled by zero.
    #[test]
    fn the_calibration_is_whole_and_changes_nothing() {
        let report = calibration();
        assert_eq!(report[0], CALIBRATION);
        let value = |at: usize| i32::from(i16::from_le_bytes([report[at], report[at + 1]]));
        assert_eq!((value(1), value(3), value(5)), (0, 0, 0));
        for axis in 0..3 {
            let (plus, minus) = (value(7 + axis * 4), value(9 + axis * 4));
            assert!(plus.abs() + minus.abs() > 0, "gyro axis {axis}");
            let (plus, minus) = (value(23 + axis * 4), value(25 + axis * 4));
            // Twice the driver's resolution over the range is one.
            assert_eq!(plus - minus, 2 * 8192, "accelerometer axis {axis}");
            assert_eq!(plus - (plus - minus) / 2, 0, "no bias on axis {axis}");
        }
        assert!(value(19) + value(21) > 0, "the gyro's speed");
        assert!(report[35..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn what_the_kernel_asks_is_read_from_its_events() {
        let event = |kind: u32, fill: &dyn Fn(&mut [u8])| {
            let mut event = vec![0u8; UHID_EVENT];
            event[..4].copy_from_slice(&kind.to_ne_bytes());
            fill(&mut event);
            event
        };
        assert_eq!(asked(&event(UHID_STOP, &|_| {})), Asked::Stop);
        assert_eq!(asked(&event(2, &|_| {})), Asked::Nothing, "a start");
        let get = event(UHID_GET_REPORT, &|event| {
            event[4..8].copy_from_slice(&77u32.to_ne_bytes());
            event[8] = FIRMWARE;
        });
        assert_eq!(asked(&get), Asked::Get(77, FIRMWARE));
        let set = event(UHID_SET_REPORT, &|event| {
            event[4..8].copy_from_slice(&78u32.to_ne_bytes());
        });
        assert_eq!(asked(&set), Asked::Set(78));
        let output = event(UHID_OUTPUT, &|event| {
            event[4..9].copy_from_slice(&[OUTPUT_REPORT, 0x01, 0, 1, 2]);
            event[4 + UHID_DATA..6 + UHID_DATA].copy_from_slice(&63u16.to_ne_bytes());
        });
        let level = Level {
            strong: 2 * 257,
            weak: 257,
        };
        assert_eq!(asked(&output), Asked::Rumble(level));
        // An output report that sets a light and marks no rumble ends
        // whatever rumble there was, as on the pad itself.
        let light = event(UHID_OUTPUT, &|event| {
            event[4..6].copy_from_slice(&[OUTPUT_REPORT, 0x00]);
            event[4 + UHID_DATA..6 + UHID_DATA].copy_from_slice(&63u16.to_ne_bytes());
        });
        assert_eq!(asked(&light), Asked::Rumble(Level::default()));
        let other = event(UHID_OUTPUT, &|event| {
            event[4] = 0x31;
            event[4 + UHID_DATA..6 + UHID_DATA].copy_from_slice(&63u16.to_ne_bytes());
        });
        assert_eq!(asked(&other), Asked::Nothing, "another report");
        assert_eq!(asked(&[]), Asked::Nothing, "an event cut short");
    }

    #[test]
    fn the_pad_is_made_as_the_kernel_expects_and_only_from_whole_data() {
        let data = DualSenseData {
            descriptor: vec![1, 2, 3],
            firmware: {
                let mut firmware = vec![0; FIRMWARE_SIZE];
                firmware[0] = FIRMWARE;
                firmware
            },
        };
        assert!(data.is_whole());
        let event = create2(
            "ctrlassist/virtual/1/hid/0",
            "02:00:00:00:00:01",
            &data.descriptor,
        );
        assert_eq!(event.len(), UHID_EVENT);
        assert_eq!(u32_at(&event, 0), UHID_CREATE2);
        assert!(event[4..].starts_with(NAME.as_bytes()));
        assert!(event[4 + 128..].starts_with(b"ctrlassist/virtual/1/hid/0\0"));
        assert!(event[4 + 192..].starts_with(b"02:00:00:00:00:01\0"));
        let numbers = 4 + 256;
        assert_eq!(u16_at(&event, numbers), 3, "the descriptor's size");
        assert_eq!(u16_at(&event, numbers + 2), BUS_USB);
        assert_eq!(u32_at(&event, numbers + 4), u32::from(VENDOR));
        assert_eq!(u32_at(&event, numbers + 8), u32::from(PRODUCT));
        assert_eq!(event[numbers + 20..numbers + 23], [1, 2, 3]);

        let short = DualSenseData {
            firmware: vec![FIRMWARE; 3],
            ..data.clone()
        };
        let empty = DualSenseData {
            descriptor: Vec::new(),
            ..data.clone()
        };
        let other = DualSenseData {
            firmware: vec![0; FIRMWARE_SIZE],
            ..data
        };
        for broken in [short, empty, other] {
            assert!(!broken.is_whole());
            assert!(Emulator::create(&broken, [2, 0, 0, 0, 0, 1]).is_err());
        }
    }
}
