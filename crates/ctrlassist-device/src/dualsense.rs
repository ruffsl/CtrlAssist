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
use crate::{descriptor, hidraw, hotplug};

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

    /// Whether a virtual pad can be made from this: a descriptor the
    /// kernel's parser takes, which declares the reports a DualSense
    /// exchanges with the kernel's driver, and a whole firmware report.
    ///
    /// A descriptor the parser refuses makes the driver give up on the
    /// pad without a word to uhid, which nothing would then end the wait
    /// for; so it is refused here, by the parser's own rules or stricter.
    #[must_use]
    pub fn is_whole(&self) -> bool {
        descriptor::is_dualsense(&self.descriptor)
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
    // Lying flat and still: no turning, and gravity on the one axis, at
    // the 8192 to the g the calibration says.
    report[24..26].copy_from_slice(&8192u16.to_le_bytes());
    // Neither touch point is in contact.
    report[33] = 0x80;
    report[37] = 0x80;
    // Charged and full, on a USB cable: a real pad on one says so, and a
    // program may go by it for what the pad can be sent.
    report[53] = 0x2a;
    report[54] = 0x08;
    report
}

/// The longest a virtual DualSense goes without a report. A real one
/// reports every 4 ms on USB. SDL2 reads once, for 16 ms, as it opens a
/// DualSense, and one that says nothing then gets no rumble: under
/// Proton, 12 opens of 12 found the pad at 8, 12 and 16 ms, and 9 and 10
/// of 12 at 20 and 33. Ten leaves room, and is what inputtino sends.
const BEAT_MS: u64 = 10;

/// Now, on the clock the input thread tells time by.
fn monotonic_ms() -> u64 {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    (now.tv_sec as u64)
        .saturating_mul(1000)
        .saturating_add(now.tv_nsec as u64 / 1_000_000)
}

/// The time the pad's sensors were read, as the report carries it: a
/// count of thirds of a microsecond that wraps. A real pad's never
/// stands still.
fn sensor_time() -> [u8; 4] {
    let now = rustix::time::clock_gettime(rustix::time::ClockId::Monotonic);
    let micros = (now.tv_sec as u64)
        .wrapping_mul(1_000_000)
        .wrapping_add(now.tv_nsec as u64 / 1000);
    (micros.wrapping_mul(3) as u32).to_le_bytes()
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

// Where each setting sits in a USB output report, the report's number
// first. The pad applies only the settings a report's three flag bytes
// mark, so a report is a change, not a state. The layout is the kernel's
// `struct dualsense_output_report_common` (drivers/hid/hid-playstation.c),
// which starts one byte in, and its `DS_OUTPUT_VALID_FLAG*` bits; the
// bytes the kernel leaves reserved are named by SDL's `DS5EffectsState_t`
// (src/joystick/hidapi/SDL_hidapi_ps5.c, at the same offset as the
// kernel's common block), and the flags that mark the trigger effects and
// the headphone volume by SDL's test program (test/testcontroller.c).

/// The first flag byte: rumble, the trigger effects and the audio.
const FLAG0: usize = 1;
/// The second: the LEDs, the microphone and more audio.
const FLAG1: usize = 2;
/// The third: the lightbar's setup and the newer rumble.
const FLAG2: usize = 39;

/// The settings a report carries that are kept, each as the flag bit
/// that marks it and the bytes it sets.
const SETTINGS: [(usize, u8, std::ops::Range<usize>); 11] = [
    // SDL's right and left trigger effects, 11 bytes each, at 10 and 21
    // of the common block; marked by bits 2 and 3, as SDL's test program
    // sets them.
    (FLAG0, 0x04, 11..22),
    (FLAG0, 0x08, 22..33),
    // The headphone volume (bit 4, as SDL's test program marks it), and
    // the kernel's speaker volume, microphone volume and audio control.
    (FLAG0, 0x10, 5..6),
    (FLAG0, 0x20, 6..7),
    (FLAG0, 0x40, 7..8),
    (FLAG0, 0x80, 8..9),
    // The kernel's microphone mute LED and power save control, which
    // mutes the microphone.
    (FLAG1, 0x01, 9..10),
    (FLAG1, 0x02, 10..11),
    // The kernel's lightbar color and player LEDs.
    (FLAG1, 0x04, 45..48),
    (FLAG1, 0x10, 44..45),
    // The kernel's second audio control.
    (FLAG1, 0x80, 38..39),
];

/// The lightbar's color and the player LEDs, which a report that gives
/// the LEDs back to the pad leaves no longer set.
const LEDS: u8 = 0x04 | 0x10;

/// The flag bits whose meaning neither the kernel nor SDL gives, kept as
/// one setting: the newest report that marks any of them gives them all,
/// with the bytes no other setting claims. They are said to be the
/// player LEDs' brightness (the third byte's bit 0, at 43) and the
/// motors' power, but that is not from either source.
const OTHER: [(usize, u8); 2] = [(FLAG1, 0x20 | 0x40), (FLAG2, 0xf9)];
/// The bytes no setting claims: SDL's unknown bytes after the trigger
/// effects, the kernel's reserved bytes before the lightbar's setup, and
/// its LED brightness.
const OTHER_BYTES: [std::ops::Range<usize>; 3] = [33..38, 40..42, 43..44];

/// What a report asks once, which is acted on as it comes and is no
/// setting to keep: the LEDs given back to the pad (the second flag
/// byte's bit 3, the kernel's `RELEASE_LEDS`, which SDL sends as "reset
/// LED state" before it sets the colors again), and the lightbar's setup
/// with its byte at 42 (the third flag byte's bit 1), which the kernel's
/// driver sends once as it binds a pad, to fade out the light it starts
/// with. Rumble is neither: every report written carries the level the
/// pad should be at.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) struct Once(u32);

const RELEASE_LEDS: u8 = 0x08;
const LIGHTBAR_SETUP: u8 = 0x02;
const LIGHTBAR_SETUP_BYTE: usize = 42;

impl Once {
    /// What `report` asks once.
    pub(crate) fn of(report: &[u8; OUTPUT_SIZE]) -> Self {
        let release = u32::from(report[FLAG1] & RELEASE_LEDS != 0);
        let setup = if report[FLAG2] & LIGHTBAR_SETUP != 0 {
            0x02 | (u32::from(report[LIGHTBAR_SETUP_BYTE]) << 8)
        } else {
            0
        };
        Self(release | setup)
    }

    /// Whether nothing is asked.
    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// This, and then `newer`: the LEDs are given back if either asks,
    /// and the newer lightbar setup is the one made.
    pub(crate) fn then(self, newer: Self) -> Self {
        let release = (self.0 | newer.0) & 0x01;
        let setup = if newer.0 & 0x02 != 0 { newer.0 } else { self.0 };
        Self(release | (setup & 0xff02))
    }

    /// As a word, for an atomic.
    pub(crate) fn bits(self) -> u32 {
        self.0
    }

    pub(crate) fn from_bits(bits: u32) -> Self {
        Self(bits & 0xff03)
    }

    /// `report` asking this as well.
    fn onto(self, mut report: [u8; OUTPUT_SIZE]) -> [u8; OUTPUT_SIZE] {
        if self.0 & 0x01 != 0 {
            report[FLAG1] |= RELEASE_LEDS;
        }
        if self.0 & 0x02 != 0 {
            report[FLAG2] |= LIGHTBAR_SETUP;
            report[LIGHTBAR_SETUP_BYTE] = (self.0 >> 8) as u8;
        }
        report
    }

    /// `report` with nothing asked once.
    pub(crate) fn strip(mut report: [u8; OUTPUT_SIZE]) -> [u8; OUTPUT_SIZE] {
        report[FLAG1] &= !RELEASE_LEDS;
        report[FLAG2] &= !LIGHTBAR_SETUP;
        report[LIGHTBAR_SETUP_BYTE] = 0;
        report
    }
}

/// What the games have set on a virtual DualSense through its output
/// reports, kept as one report: each setting from the newest report that
/// marks it, and its flag kept. A report's rumble is not kept, being
/// replaced by the level each pad should be at, and neither is what it
/// asks once, which goes with the next report passed on and no other.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Settings {
    report: [u8; OUTPUT_SIZE],
    /// Whether a game has sent any report.
    any: bool,
    /// What was asked once since it was last passed on.
    once: Once,
}

impl Settings {
    pub(crate) const NONE: Self = Self {
        report: [0; OUTPUT_SIZE],
        any: false,
        once: Once(0),
    };

    /// Takes in a game's output report.
    pub(crate) fn take(&mut self, report: &[u8; OUTPUT_SIZE]) {
        if report[0] != OUTPUT_REPORT {
            return;
        }
        let kept = &mut self.report;
        kept[0] = OUTPUT_REPORT;
        // Given back before the report's own settings are taken, so that
        // a report that gives the LEDs back and sets a color keeps it.
        if report[FLAG1] & RELEASE_LEDS != 0 {
            kept[FLAG1] &= !LEDS;
        }
        for (flag, bit, bytes) in &SETTINGS {
            if report[*flag] & bit != 0 {
                kept[*flag] |= bit;
                kept[bytes.clone()].copy_from_slice(&report[bytes.clone()]);
            }
        }
        if OTHER.iter().any(|(flag, bits)| report[*flag] & bits != 0) {
            for (flag, bits) in OTHER {
                kept[flag] = (kept[flag] & !bits) | (report[flag] & bits);
            }
            for bytes in OTHER_BYTES {
                kept[bytes.clone()].copy_from_slice(&report[bytes]);
            }
        }
        self.once = self.once.then(Once::of(report));
        self.any = true;
    }

    /// Every setting kept, for a pad that joins: nothing asked once, which
    /// went to the pads there were then.
    pub(crate) fn kept(&self) -> Option<[u8; OUTPUT_SIZE]> {
        self.any.then_some(self.report)
    }

    /// Every setting kept and what was asked once since this was last
    /// asked, for the pads a new report goes to.
    pub(crate) fn fresh(&mut self) -> Option<[u8; OUTPUT_SIZE]> {
        let once = std::mem::take(&mut self.once);
        self.any.then(|| once.onto(self.report))
    }
}

/// The output report a DualSense takes over Bluetooth, and its size.
const BLUETOOTH_OUTPUT_REPORT: u8 = 0x31;
const BLUETOOTH_OUTPUT_SIZE: usize = 78;
/// What marks a Bluetooth output report as one, after its sequence
/// number: the tag the kernel's driver writes.
const BLUETOOTH_OUTPUT_TAG: u8 = 0x10;

/// A USB output report as a DualSense takes it over Bluetooth: another
/// report number, a sequence number that counts reports round from 0
/// to 15, a tag, the same block of settings the USB report carries
/// after its own number, and a checksum of it all, as the kernel's
/// driver writes one.
fn over_bluetooth(usb: [u8; OUTPUT_SIZE], sequence: u8) -> [u8; BLUETOOTH_OUTPUT_SIZE] {
    let mut report = [0u8; BLUETOOTH_OUTPUT_SIZE];
    report[0] = BLUETOOTH_OUTPUT_REPORT;
    report[1] = (sequence & 0x0f) << 4;
    report[2] = BLUETOOTH_OUTPUT_TAG;
    report[3..3 + OUTPUT_SIZE - 1].copy_from_slice(&usb[1..]);
    let checksum = crc32(0xa2, &report[..BLUETOOTH_OUTPUT_SIZE - 4]);
    report[BLUETOOTH_OUTPUT_SIZE - 4..].copy_from_slice(&checksum.to_le_bytes());
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
/// report ends in a checksum, and says nothing of how often the pad
/// reports: the kernel's driver sets that once as it binds the pad, and
/// leaves it out of every report after, as this does.
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
    // A HID report with a checksum. The low six bits, the interval,
    // stay clear, as in the driver's own reports after the first.
    report[1] = 0x80 | 0x40;
    report[3..11].copy_from_slice(&common);
    let checksum = crc32(0xa2, &report[..DS4_BLUETOOTH_SIZE - 4]);
    report[DS4_BLUETOOTH_SIZE - 4..].copy_from_slice(&checksum.to_le_bytes());
    (report, DS4_BLUETOOTH_SIZE)
}

/// Which pad a raw node is, for what it is written.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Kind {
    /// A DualSense, whether its firmware takes the newer rumble, and
    /// whether it is on Bluetooth, where its report is another one.
    DualSense { newer: bool, bluetooth: bool },
    /// A DualShock 4, and whether it is on Bluetooth.
    DualShock4 { bluetooth: bool },
}

impl Kind {
    /// Which pad a raw node is, if it is one this knows how to write
    /// to: a DualSense or a DualShock 4, over USB or Bluetooth.
    pub(crate) fn of(raw: BorrowedFd<'_>) -> io::Result<Self> {
        let (bus, vendor, product) = hidraw::info(raw)?;
        let wired_or_not = bus == BUS_USB || bus == BUS_BLUETOOTH;
        if (vendor, product) == (VENDOR, PRODUCT) && wired_or_not {
            let firmware = hidraw::feature(raw, FIRMWARE, FIRMWARE_SIZE)?;
            let update = u16::from_le_bytes([
                firmware.get(44).copied().unwrap_or(0),
                firmware.get(45).copied().unwrap_or(0),
            ]);
            Ok(Self::DualSense {
                newer: update >= NEWER_RUMBLE,
                bluetooth: bus == BUS_BLUETOOTH,
            })
        } else if vendor == VENDOR && DUALSHOCK4.contains(&product) && wired_or_not {
            Ok(Self::DualShock4 {
                bluetooth: bus == BUS_BLUETOOTH,
            })
        } else {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "not a DualSense or a DualShock 4",
            ))
        }
    }

    /// Whether a pad with these numbers is one [`of`](Self::of) takes.
    pub(crate) fn takes(bus: u16, vendor: u16, product: u16) -> bool {
        vendor == VENDOR
            && (product == PRODUCT || DUALSHOCK4.contains(&product))
            && (bus == BUS_USB || bus == BUS_BLUETOOTH)
    }

    /// Whether a game's report says anything this pad is sent: all of it
    /// to a DualSense, and to a DualShock 4 the lightbar's color, where
    /// the report says to take it.
    pub(crate) fn wants(self, game: &[u8; OUTPUT_SIZE]) -> bool {
        match self {
            Self::DualSense { .. } => true,
            Self::DualShock4 { .. } => game[0] == OUTPUT_REPORT && game[2] & 0x04 != 0,
        }
    }

    /// The lights the kernel's driver keeps for this pad, which it set
    /// as it bound the pad and shows while no game sets them: read from
    /// the LED devices it registers under the pad's HID device, `device`
    /// in sysfs.
    ///
    /// For a DualSense the player LEDs are read as the driver holds them
    /// (its `dualsense_player_led_get_brightness`). Its lightbar is the
    /// blue the driver sets as it binds every DualSense, unless the
    /// lightbar's LED device was given a color, which sysfs shows only
    /// then: the driver sets its blue without telling the LED device.
    /// For a DualShock 4 the lightbar's color is read as the driver holds
    /// it, its player's color (its `dualshock4_led_get_brightness`), and
    /// off if its LED device is.
    pub(crate) fn driver_lights(self, device: Option<&Path>) -> Lights {
        let leds: Vec<(String, PathBuf)> = device
            .and_then(|device| std::fs::read_dir(device.join("leds")).ok())
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| {
                        (
                            entry.file_name().to_string_lossy().into_owned(),
                            entry.path(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let led = |suffix: &str| {
            leds.iter()
                .find(|(name, _)| name.ends_with(suffix))
                .map(|(_, path)| path.as_path())
        };
        let number = |dir: Option<&Path>, file: &str| -> Option<u32> {
            std::fs::read_to_string(dir?.join(file))
                .ok()?
                .trim()
                .parse()
                .ok()
        };
        match self {
            Self::DualSense { .. } => {
                let given = led(":rgb:indicator").and_then(|dir| {
                    let intensity = std::fs::read_to_string(dir.join("multi_intensity")).ok()?;
                    let intensity: Vec<u32> = intensity
                        .split_whitespace()
                        .filter_map(|value| value.parse().ok())
                        .collect();
                    let brightness = number(Some(dir), "brightness")?;
                    // The LED class's own sum: each color's intensity
                    // scaled by the brightness, out of the lightbar's 255.
                    let [red, green, blue] = <[u32; 3]>::try_from(intensity).ok()?;
                    // Saturating, both being whatever sysfs says: a sum
                    // that overflowed would panic, or wrap to a dim color.
                    let scaled =
                        |value: u32| (value.saturating_mul(brightness) / 255).min(255) as u8;
                    (red | green | blue != 0).then(|| [scaled(red), scaled(green), scaled(blue)])
                });
                let mut players = Some(0u8);
                for (bit, player) in (1..=5).enumerate() {
                    let on = number(led(&format!(":white:player-{player}")), "brightness");
                    players = players
                        .zip(on)
                        .map(|(bits, on)| bits | (u8::from(on > 0) << bit));
                }
                Lights {
                    color: Some(given.unwrap_or(DRIVER_BLUE)),
                    players,
                }
            }
            Self::DualShock4 { .. } => {
                let red = number(led(":red"), "brightness");
                let green = number(led(":green"), "brightness");
                let blue = number(led(":blue"), "brightness");
                let on = number(led(":global"), "brightness");
                let color = match (red, green, blue, on) {
                    (Some(_), Some(_), Some(_), Some(0)) => Some([0; 3]),
                    (Some(red), Some(green), Some(blue), Some(_)) => {
                        Some([red, green, blue].map(|value| value.min(255) as u8))
                    }
                    _ => None,
                };
                Lights {
                    color,
                    players: None,
                }
            }
        }
    }
}

/// The lightbar's color the kernel's driver gives every DualSense as it
/// binds it: `dualsense_set_lightbar(ds, 0, 0, 128)`.
const DRIVER_BLUE: [u8; 3] = [0, 0, 128];

/// A pad's lights as its kernel driver keeps them, where they are known.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub(crate) struct Lights {
    /// The lightbar's color.
    pub(crate) color: Option<[u8; 3]>,
    /// A DualSense's five player LEDs, a bit each from the left.
    pub(crate) players: Option<u8>,
}

/// The trigger effect that turns one off: mode 5, which SDL's test
/// program sends to clear an effect, with nothing after it.
const TRIGGER_OFF: u8 = 0x05;

/// The size of the game's report a pad is passed.
pub(crate) const GAME_SIZE: usize = OUTPUT_SIZE;
/// The longest report written to a pad's raw node.
pub(crate) const RAW_SIZE: usize = BLUETOOTH_OUTPUT_SIZE;

/// Builds the reports written to one pad's raw node. Each carries the
/// level the pad should be at: a DualSense takes a report without the
/// rumble as the rumble's end, and the kernel's driver sends a DualShock
/// 4 its own lightbar color with every rumble.
pub(crate) struct Composer {
    kind: Kind,
    /// Over Bluetooth, the number of the next report.
    sequence: u8,
    /// The lightbar color a game last asked of a DualShock 4, if one has.
    color: Option<[u8; 3]>,
}

impl Composer {
    pub(crate) fn new(kind: Kind) -> Self {
        Self {
            kind,
            sequence: 0,
            color: None,
        }
    }

    /// The report that leaves the pad at `level`, carrying `game`, the
    /// games' settings not yet written: a DualSense's whole, or the
    /// lightbar's color for a DualShock 4. Without them, a DualSense's
    /// report changes nothing but rumble, and a DualShock 4's keeps the
    /// color it was last given. A DualSense's carries what is asked
    /// `once` too. Returns the report and its length.
    pub(crate) fn compose(
        &mut self,
        game: Option<&[u8; OUTPUT_SIZE]>,
        level: Level,
        once: Once,
    ) -> ([u8; RAW_SIZE], usize) {
        match self.kind {
            Kind::DualSense { newer, bluetooth } => {
                let settings = once.onto(game.copied().unwrap_or([0; OUTPUT_SIZE]));
                let usb = with_rumble(settings, level, newer);
                if bluetooth {
                    let report = over_bluetooth(usb, self.sequence);
                    self.sequence = (self.sequence + 1) & 0x0f;
                    (report, BLUETOOTH_OUTPUT_SIZE)
                } else {
                    let mut report = [0u8; RAW_SIZE];
                    report[..OUTPUT_SIZE].copy_from_slice(&usb);
                    (report, OUTPUT_SIZE)
                }
            }
            Kind::DualShock4 { bluetooth } => {
                if let Some(game) = game.filter(|game| self.kind.wants(game)) {
                    self.color = Some([game[45], game[46], game[47]]);
                }
                dualshock4_report(level, self.color, bluetooth)
            }
        }
    }

    /// The last report a pad is written, as its setup lets it go: its
    /// motors stopped, a DualSense's trigger effects off, and the lights
    /// its driver keeps, where they are known. Whatever else a game set,
    /// its audio and its microphone's LED, is left as it is: the driver
    /// keeps no state of those to put back.
    pub(crate) fn rest(&mut self, lights: Lights) -> ([u8; RAW_SIZE], usize) {
        match self.kind {
            Kind::DualSense { .. } => {
                let mut rest = [0u8; OUTPUT_SIZE];
                rest[0] = OUTPUT_REPORT;
                rest[FLAG0] = 0x04 | 0x08;
                (rest[11], rest[22]) = (TRIGGER_OFF, TRIGGER_OFF);
                if let Some(color) = lights.color {
                    rest[FLAG1] |= 0x04;
                    rest[45..48].copy_from_slice(&color);
                }
                if let Some(players) = lights.players {
                    rest[FLAG1] |= 0x10;
                    rest[44] = players;
                }
                self.compose(Some(&rest), Level::default(), Once::default())
            }
            Kind::DualShock4 { bluetooth } => {
                // Unknown, the color is left as the game set it.
                dualshock4_report(Level::default(), lights.color, bluetooth)
            }
        }
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
    /// A report was set, which is only to be acknowledged: the
    /// request's number, and the report's.
    Set(u32, u8),
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
        UHID_SET_REPORT => Asked::Set(u32_at(event, 4), event.get(8).copied().unwrap_or(0)),
        _ => Asked::Nothing,
    }
}

/// A virtual DualSense.
pub(crate) struct Emulator {
    uhid: File,
    event_node: PathBuf,
    held: PadState,
    sequence: u8,
    /// When it last sent an input report, on the monotonic clock.
    reported_ms: u64,
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
    /// What the games have set through every output report, for the
    /// pads behind it.
    settings: Settings,
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
        // It ends as a real pad's does on USB, where the pad is the fourth
        // interface of its device: Wine takes a raw HID device's interface
        // number from there, and a program may look for the pad by it.
        let phys = format!("{OWN_MARKER}{}/hid/{number}/input3", std::process::id());
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
            reported_ms: 0,
            pairing: pairing(address),
            firmware,
            calibration: calibration(),
            level: Level::default(),
            stopped: false,
            output: [0; OUTPUT_SIZE],
            sent: false,
            fresh: false,
            settings: Settings::NONE,
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
            let mut report = input_report(state, self.sequence);
            report[28..32].copy_from_slice(&sensor_time());
            // UHID_INPUT2: type u32, size u16, then the report.
            let mut event = [0u8; 6 + INPUT_SIZE];
            event[..4].copy_from_slice(&UHID_INPUT2.to_ne_bytes());
            event[4..6].copy_from_slice(&(INPUT_SIZE as u16).to_ne_bytes());
            event[6..].copy_from_slice(&report);
            self.uhid.write_all(&event)?;
            self.reported_ms = monotonic_ms();
        }
        self.held = *state;
        Ok(())
    }

    /// Keeps the event just read if it is an output report, and takes in
    /// what it sets, which the pads behind this one are sent.
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
        self.settings.take(&output);
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

    /// What the games have set, as one report, if any report came: with
    /// what was asked once since this was last asked `fresh`, for the
    /// pads a new report goes to, or without, for a pad that joins.
    pub(crate) fn settings(&mut self, fresh: bool) -> Option<[u8; OUTPUT_SIZE]> {
        if fresh {
            self.settings.fresh()
        } else {
            self.settings.kept()
        }
    }

    /// Sends the report the pad holds once more if it has sent none for
    /// [`BEAT_MS`], and says when it is next due. A real pad reports all
    /// the time, changed or not, and a program that opens one and hears
    /// nothing in its first moments takes it for a pad that can't be
    /// sent rumble.
    pub(crate) fn beat(&mut self, now_ms: u64) -> io::Result<u64> {
        let due = self.reported_ms.saturating_add(BEAT_MS);
        if now_ms < due {
            return Ok(due);
        }
        self.reported_ms = now_ms;
        self.sequence = self.sequence.wrapping_add(1);
        let mut report = input_report(&self.held, self.sequence);
        report[28..32].copy_from_slice(&sensor_time());
        let mut event = [0u8; 6 + INPUT_SIZE];
        event[..4].copy_from_slice(&UHID_INPUT2.to_ne_bytes());
        event[4..6].copy_from_slice(&(INPUT_SIZE as u16).to_ne_bytes());
        event[6..].copy_from_slice(&report);
        self.uhid.write_all(&event)?;
        Ok(now_ms.saturating_add(BEAT_MS))
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
                    let answered = self.feature(number).is_some();
                    let _ = handle(Request::Feature {
                        number,
                        set: false,
                        answered,
                    });
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
                Asked::Set(id, number) => {
                    let _ = handle(Request::Feature {
                        number,
                        set: true,
                        answered: true,
                    });
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
        // Neither touch point in contact, a full battery, a USB cable,
        // and gravity on the one axis: a real pad lying on a desk.
        assert_eq!((report[33], report[37], report[53]), (0x80, 0x80, 0x2a));
        assert_eq!((report[54], report[24], report[25]), (0x08, 0x00, 0x20));
        let rest: Vec<usize> = (11..INPUT_SIZE)
            .filter(|index| report[*index] != 0)
            .collect();
        assert_eq!(rest, [25, 33, 37, 53, 54], "nothing else is set");
        assert_ne!(sensor_time(), [0; 4]);
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
        assert_eq!(bluetooth[..11], [0x11, 0xc0, 0, 0x03, 0, 0, 0, 0, 9, 8, 7]);
        let checksum = u32::from_le_bytes(bluetooth[74..].try_into().unwrap());
        assert_eq!(checksum, crc32(0xa2, &bluetooth[..74]));
        assert!(Kind::takes(BUS_USB, VENDOR, PRODUCT));
        assert!(Kind::takes(BUS_BLUETOOTH, VENDOR, PRODUCT));
        assert!(!Kind::takes(6, VENDOR, PRODUCT), "another bus");
        assert!(Kind::takes(BUS_BLUETOOTH, VENDOR, 0x05c4));
        assert!(Kind::takes(BUS_USB, VENDOR, 0x09cc));
        assert!(!Kind::takes(BUS_USB, 0x045e, 0x05c4));
    }

    /// Over Bluetooth a DualSense's report is the USB one's settings
    /// under another number, after a sequence number and a tag, and
    /// before a checksum of it all.
    #[test]
    fn a_dualsense_s_report_over_bluetooth_wraps_the_usb_one() {
        // A game's report: rumble marked, a lightbar color at its end.
        let mut game = [0u8; OUTPUT_SIZE];
        game[0] = OUTPUT_REPORT;
        game[2] = 0x04;
        (game[45], game[46], game[47]) = (0x11, 0x22, 0x33);
        let usb = with_rumble(game, level_of(0x80, 0x40), true);
        let report = over_bluetooth(usb, 5);
        assert_eq!(report.len(), 78);
        assert_eq!(report[..3], [0x31, 0x50, 0x10]);
        // The settings, as the USB report has them after its number.
        assert_eq!(report[3..50], usb[1..]);
        assert_eq!(rumble_of(&usb), Some(level_of(0x80, 0x40)));
        assert_eq!(report[47..50], [0x11, 0x22, 0x33]);
        assert!(report[50..74].iter().all(|byte| *byte == 0));
        let checksum = u32::from_le_bytes(report[74..].try_into().unwrap());
        assert_eq!(checksum, crc32(0xa2, &report[..74]));
        // The sequence number counts round in the upper half of its byte.
        assert_eq!(over_bluetooth(usb, 15)[1], 0xf0);
        assert_eq!(over_bluetooth(usb, 16)[1], 0x00);
        // The checksum is the CRC-32 everyone's is, over the seed byte
        // and then the rest: its check value, for "123456789".
        assert_eq!(crc32(b'1', b"23456789"), 0xcbf4_3926);
    }

    /// What each report written to a pad's raw node carries besides the
    /// one thing that changed.
    #[test]
    fn each_report_written_carries_what_the_pad_is_owed() {
        // A DualShock 4 keeps the color it was given at each rumble, and
        // a report that sets no color leaves it.
        let mut ds4 = Composer::new(Kind::DualShock4 { bluetooth: false });
        let (report, length) = ds4.compose(None, level_of(0x80, 0), Once::default());
        assert_eq!(length, 32);
        assert_eq!(report[..9], [0x05, 0x01, 0, 0, 0, 0x80, 0, 0, 0]);
        let mut game = [0u8; OUTPUT_SIZE];
        game[0] = OUTPUT_REPORT;
        (game[2], game[45], game[46], game[47]) = (0x04, 1, 2, 3);
        ds4.compose(Some(&game), level_of(0x80, 0), Once::default());
        game[2] = 0;
        game[45] = 9;
        ds4.compose(Some(&game), level_of(0x80, 0), Once::default());
        let (report, _) = ds4.compose(None, Level::default(), Once::default());
        assert_eq!(report[..9], [0x05, 0x03, 0, 0, 0, 0, 1, 2, 3]);
        // A DualSense's report without a game's changes nothing but
        // rumble; one with it carries it whole.
        let mut usb = Composer::new(Kind::DualSense {
            newer: true,
            bluetooth: false,
        });
        let (alone, length) = usb.compose(None, level_of(0x80, 0x40), Once::default());
        assert_eq!(length, OUTPUT_SIZE);
        assert_eq!(
            alone[..OUTPUT_SIZE],
            with_rumble([0; OUTPUT_SIZE], level_of(0x80, 0x40), true)
        );
        let (whole, _) = usb.compose(Some(&game), Level::default(), Once::default());
        assert_eq!(
            whole[..OUTPUT_SIZE],
            with_rumble(game, Level::default(), true)
        );
        // Over Bluetooth each report written is numbered on from the last.
        let mut bluetooth = Composer::new(Kind::DualSense {
            newer: true,
            bluetooth: true,
        });
        let numbers: Vec<u8> = (0..17)
            .map(|_| bluetooth.compose(None, Level::default(), Once::default()).0[1] >> 4)
            .collect();
        assert_eq!(numbers[..4], [0, 1, 2, 3]);
        assert_eq!(numbers[15..], [15, 0]);
    }

    /// A game's output report that marks `flags` (the three flag bytes)
    /// and sets `bytes`.
    fn marked(flags: [u8; 3], bytes: &[(usize, u8)]) -> [u8; OUTPUT_SIZE] {
        let mut report = [0u8; OUTPUT_SIZE];
        report[0] = OUTPUT_REPORT;
        (report[FLAG0], report[FLAG1], report[FLAG2]) = (flags[0], flags[1], flags[2]);
        for (at, byte) in bytes {
            report[*at] = *byte;
        }
        report
    }

    /// The pad takes only what a report marks, so reports that each set
    /// one thing are kept as one that sets them all, each from the newest
    /// report that set it.
    #[test]
    fn each_setting_is_kept_from_the_newest_report_that_set_it() {
        let mut settings = Settings::NONE;
        assert_eq!(settings.kept(), None, "nothing sent yet");
        assert_eq!(settings.fresh(), None);
        // The lightbar's color and the right trigger's effect.
        settings.take(&marked(
            [0x04, 0x04, 0],
            &[(11, 0x01), (13, 110), (45, 1), (46, 2), (47, 3)],
        ));
        // The player LEDs alone: the color stays.
        settings.take(&marked([0, 0x10, 0], &[(44, 0x05)]));
        // A new color, with rumble, which is not kept, and the left
        // trigger's effect.
        settings.take(&marked(
            [0x0b, 0x04, 0x04],
            &[(3, 9), (4, 9), (22, 0x02), (45, 4), (46, 5), (47, 6)],
        ));
        // Another report than the output report is no setting.
        let mut other = marked([0xff, 0xff, 0xff], &[(45, 0xff)]);
        other[0] = 0x31;
        settings.take(&other);
        let kept = settings.kept().unwrap();
        assert_eq!((kept[FLAG0], kept[FLAG1], kept[FLAG2]), (0x0c, 0x14, 0));
        assert_eq!(kept[3..5], [0, 0], "rumble isn't kept");
        assert_eq!((kept[11], kept[13], kept[22]), (0x01, 110, 0x02));
        assert_eq!(kept[44..48], [0x05, 4, 5, 6]);
        assert_eq!(settings.fresh(), Some(kept), "nothing was asked once");
        // Every setting has its place in the report, and none overlaps.
        let mut claimed = [false; OUTPUT_SIZE];
        let places = SETTINGS
            .iter()
            .map(|(_, _, bytes)| bytes.clone())
            .chain(OTHER_BYTES)
            .chain([3..5, LIGHTBAR_SETUP_BYTE..LIGHTBAR_SETUP_BYTE + 1]);
        for at in places.flatten() {
            assert!(!claimed[at], "byte {at} is claimed twice");
            claimed[at] = true;
        }
        let flags = [0, FLAG0, FLAG1, FLAG2];
        let unclaimed: Vec<usize> = (0..OUTPUT_SIZE)
            .filter(|at| !claimed[*at] && !flags.contains(at))
            .collect();
        assert!(unclaimed.is_empty(), "{unclaimed:?}");
    }

    /// What neither source names is kept as one setting, whole from the
    /// newest report that marks any of it.
    #[test]
    fn what_no_source_names_is_kept_whole_from_the_newest_report_that_marks_it() {
        let mut settings = Settings::NONE;
        settings.take(&marked([0, 0, 0x01], &[(43, 0x7f)]));
        settings.take(&marked([0, 0x04, 0], &[(43, 0x11), (45, 9)]));
        let kept = settings.kept().unwrap();
        assert_eq!((kept[FLAG2], kept[43]), (0x01, 0x7f), "unmarked: kept");
        settings.take(&marked([0, 0x40, 0], &[(37, 3)]));
        let kept = settings.kept().unwrap();
        assert_eq!((kept[FLAG1], kept[FLAG2]), (0x44, 0), "replaced whole");
        assert_eq!((kept[37], kept[43], kept[45]), (3, 0, 9));
    }

    /// What a report asks once goes with the next settings passed on to
    /// the pads there are, and to no pad that joins later.
    #[test]
    fn what_a_report_asks_once_goes_once() {
        let mut settings = Settings::NONE;
        settings.take(&marked([0, 0x14, 0], &[(44, 0x05), (45, 1)]));
        // The LEDs given back to the pad: the colors set before are no
        // longer what it shows. And the lightbar set up twice, the newer
        // the one made.
        settings.take(&marked([0, 0x08, 0x02], &[(LIGHTBAR_SETUP_BYTE, 0x02)]));
        settings.take(&marked([0, 0, 0x02], &[(LIGHTBAR_SETUP_BYTE, 0x01)]));
        let kept = settings.kept().unwrap();
        assert_eq!(
            (kept[FLAG1], kept[FLAG2], kept[LIGHTBAR_SETUP_BYTE]),
            (0, 0, 0)
        );
        let fresh = settings.fresh().unwrap();
        assert_eq!(
            (fresh[FLAG1], fresh[FLAG2], fresh[LIGHTBAR_SETUP_BYTE]),
            (0x08, 0x02, 0x01)
        );
        assert_eq!(settings.fresh(), Some(kept), "once only");
        // Given back and set again in one report: the color set is kept.
        settings.take(&marked([0, 0x0c, 0], &[(45, 7)]));
        let kept = settings.kept().unwrap();
        assert_eq!((kept[FLAG1], kept[45]), (0x04, 7));
        assert_eq!(settings.fresh().unwrap()[FLAG1], 0x0c);

        let release = Once::of(&marked([0, 0x08, 0], &[]));
        let setup = |byte| Once::of(&marked([0, 0, 0x02], &[(LIGHTBAR_SETUP_BYTE, byte)]));
        assert!(Once::default().is_empty() && !release.is_empty());
        assert_eq!(release.then(setup(3)), setup(3).then(release));
        assert_eq!(setup(3).then(setup(4)), setup(4));
        assert_eq!(setup(4).then(Once::default()), setup(4));
        let both = release.then(setup(3));
        assert_eq!(Once::from_bits(both.bits()), both);
        let stripped = Once::strip(both.onto([0; OUTPUT_SIZE]));
        assert_eq!(stripped, [0; OUTPUT_SIZE]);
        // Over Bluetooth it is in the same place in the settings, two
        // bytes on.
        let mut bluetooth = Composer::new(Kind::DualSense {
            newer: true,
            bluetooth: true,
        });
        let (report, _) = bluetooth.compose(None, Level::default(), both);
        assert_eq!((report[FLAG1 + 2], report[FLAG2 + 2]), (0x08, 0x02));
        assert_eq!(report[LIGHTBAR_SETUP_BYTE + 2], 3);
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
        assert_eq!(asked(&set), Asked::Set(78, 0));
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
            firmware: {
                let mut firmware = vec![0; FIRMWARE_SIZE];
                firmware[0] = FIRMWARE;
                firmware
            },
            ..DualSenseData::shipped()
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
        assert_eq!(u16_at(&event, numbers), 273, "the descriptor's size");
        assert_eq!(u16_at(&event, numbers + 2), BUS_USB);
        assert_eq!(u32_at(&event, numbers + 4), u32::from(VENDOR));
        assert_eq!(u32_at(&event, numbers + 8), u32::from(PRODUCT));
        assert_eq!(event[numbers + 20..numbers + 293], data.descriptor[..]);

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
            ..data.clone()
        };
        // A descriptor cut short, which the kernel's parser refuses: the
        // driver would give the pad up without a word.
        let corrupt = DualSenseData {
            descriptor: data.descriptor[..100].to_vec(),
            ..data
        };
        for broken in [short, empty, other, corrupt] {
            assert!(!broken.is_whole());
            assert!(Emulator::create(&broken, [2, 0, 0, 0, 0, 1]).is_err());
        }
    }

    /// A lightbar's LED device whose numbers overflow their product is
    /// read as the brightest it can be, not as a panic on the writer.
    #[test]
    fn a_lightbar_s_numbers_too_large_to_multiply_read_as_the_brightest() {
        let device =
            std::env::temp_dir().join(format!("ctrlassist-device-lights-{}", std::process::id()));
        let led = device.join("leds").join("input9:rgb:indicator");
        std::fs::create_dir_all(&led).unwrap();
        std::fs::write(led.join("multi_intensity"), "4000000000 0 1\n").unwrap();
        std::fs::write(led.join("brightness"), "255\n").unwrap();
        let usb = Kind::DualSense {
            newer: true,
            bluetooth: false,
        };
        let lights = usb.driver_lights(Some(&device));
        std::fs::remove_dir_all(&device).unwrap();
        assert_eq!(lights.color, Some([255, 0, 1]));
    }
}
