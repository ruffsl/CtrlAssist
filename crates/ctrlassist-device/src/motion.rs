//! A DualSense's motion sensors, touchpad and click, read from its raw
//! HID node.
//!
//! The kernel's PlayStation driver puts these on input devices of their
//! own beside the pad's, a motion node and a touchpad node, which the
//! daemon doesn't read: it has the pad's event node, and a hidden pad's
//! raw node from the hiding service. So they are read from the raw node,
//! the pad's own input report, as SDL reads them: six sensor counts, the
//! time the pad read them, both contacts and the click.
//!
//! Counts become turning and gravity by the pad's own calibration report,
//! applied as SDL applies it (`HIDAPI_DriverPS5_LoadCalibrationData` and
//! `HIDAPI_DriverPS5_ApplyCalibrationData` in `SDL_hidapi_ps5.c`): the
//! stored gyroscope bias taken off, and a report SDL would doubt
//! replaced by nominal scales. Nothing is measured: the bias the pad
//! stores is the bias taken off, which is what a game reading the pad
//! through SDL is handed. The report's layout is the kernel's `struct
//! dualsense_input_report` (`drivers/hid/hid-playstation.c`).

use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use ctrlassist_core::{Contact, Motion, Touchpad};
use rustix::fs::OFlags;
use rustix::io::Errno;

use crate::PadError;
use crate::dualsense::{
    ACCEL, BUTTONS2, CALIBRATION, CALIBRATION_SIZE, CONTACTS, GYRO, INPUT_REPORT, INPUT_SIZE,
    NO_CONTACT, SENSOR_TIME, TOUCHPAD_CLICK, TOUCHPAD_X, TOUCHPAD_Y,
};
use crate::hidraw;

/// The input report over Bluetooth, once the kernel's driver has put the
/// pad in its full mode: a byte more before the same body as over USB,
/// and a checksum after it. The driver refuses one whose checksum fails
/// before the raw node is given it.
const BLUETOOTH_REPORT: u8 = 0x31;
const BLUETOOTH_SIZE: usize = 78;
/// The least of a calibration report SDL reads: shorter, it leaves the
/// pad uncalibrated.
const CALIBRATION_LEAST: usize = 35;
/// Room for one report: more than either report read here, and the
/// size of neither, so that a longer report cut short to it is passed
/// over and not taken for one.
const BUFFER: usize = 128;
const _: () = assert!(BUFFER > BLUETOOTH_SIZE && BUFFER > INPUT_SIZE);

/// What one input report says: the gyroscope's pitch, yaw and roll and
/// the accelerometer's x, y and z in counts, when the pad read them, in
/// thirds of a microsecond, wrapping, both contacts as the pad packs
/// them, and whether the touchpad is clicked.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Sample {
    gyro: [i16; 3],
    accel: [i16; 3],
    time: u32,
    contacts: [[u8; 4]; Touchpad::CONTACTS],
    click: bool,
}

impl Sample {
    /// Reads an input report over USB (0x01, 64 bytes) or Bluetooth
    /// (0x31, 78 bytes), its number at 0. Any other report, the short one
    /// a pad on Bluetooth sends before its driver switches it to the full
    /// one among them, is none.
    fn parse(report: &[u8]) -> Option<Self> {
        // The body, with each place where it is in the USB report.
        let body = match (report.first(), report.len()) {
            (Some(&INPUT_REPORT), INPUT_SIZE) => report,
            (Some(&BLUETOOTH_REPORT), BLUETOOTH_SIZE) => report.get(1..)?,
            _ => return None,
        };
        let byte = |at: usize| body.get(at).copied();
        let word = |at: usize| Some(i16::from_le_bytes([byte(at)?, byte(at + 1)?]));
        let contact = |slot: usize| {
            let at = CONTACTS + 4 * slot;
            Some([byte(at)?, byte(at + 1)?, byte(at + 2)?, byte(at + 3)?])
        };
        Some(Self {
            gyro: [word(GYRO)?, word(GYRO + 2)?, word(GYRO + 4)?],
            accel: [word(ACCEL)?, word(ACCEL + 2)?, word(ACCEL + 4)?],
            time: u32::from_le_bytes([
                byte(SENSOR_TIME)?,
                byte(SENSOR_TIME + 1)?,
                byte(SENSOR_TIME + 2)?,
                byte(SENSOR_TIME + 3)?,
            ]),
            contacts: [contact(0)?, contact(1)?],
            click: byte(BUTTONS2)? & TOUCHPAD_CLICK != 0,
        })
    }

    /// Both contacts: whether a finger is on each, and its place across
    /// 0 to 1919 and down 0 to 1079, from 0 to 1.
    fn touchpad(&self) -> Touchpad {
        Touchpad::new(self.contacts.map(|[first, low, middle, high]| {
            let x = u16::from(low) | (u16::from(middle & 0x0f) << 8);
            let y = u16::from(middle >> 4) | (u16::from(high) << 4);
            Contact::new(
                first & NO_CONTACT == 0,
                f32::from(x) / TOUCHPAD_X,
                f32::from(y) / TOUCHPAD_Y,
            )
        }))
    }
}

/// One sensor axis: a count is worth `(count - bias) * scale`, in
/// degrees a second for the gyroscope and in g for the accelerometer.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Axis {
    bias: f32,
    scale: f32,
}

impl Axis {
    fn value(self, count: i16) -> f32 {
        (f32::from(count) - self.bias) * self.scale
    }
}

/// SDL's units: a gyroscope's sensitivity is in parts of a degree a
/// second to the count, 1024 to the degree; an accelerometer's in parts
/// of a g, 8192 to the g.
const GYRO_RESOLUTION: f32 = 1024.0;
const ACCEL_RESOLUTION: f32 = 8192.0;
/// The sensitivity SDL takes as usual for each, against which it doubts
/// a report, and which it uses for a report it doubts: 64/1024 of a
/// degree a second to the count, and 1/8192 of a g.
const GYRO_USUAL: f32 = 64.0;
const ACCEL_USUAL: f32 = 1.0;

/// What a pad's calibration report says each sensor count is worth, as
/// SDL reads it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Calibration {
    /// Pitch, yaw and roll.
    gyro: [Axis; 3],
    /// x, y and z.
    accel: [Axis; 3],
}

impl Calibration {
    /// What SDL uses for a pad whose report it doubts or couldn't read:
    /// no bias, and the usual sensitivities.
    const NOMINAL: Self = Self {
        gyro: [Axis {
            bias: 0.0,
            scale: GYRO_USUAL / GYRO_RESOLUTION,
        }; 3],
        accel: [Axis {
            bias: 0.0,
            scale: ACCEL_USUAL / ACCEL_RESOLUTION,
        }; 3],
    };

    /// Reads a calibration report (0x05), its number at 0, as SDL does.
    ///
    /// Each gyroscope axis takes its stored bias off, and its scale is
    /// twice the speed it was calibrated at over the counts from its
    /// minus point to its plus point; each accelerometer axis's bias is
    /// the middle of its two points, each a g, and its scale two g over
    /// the counts between them. A report shorter than SDL reads is
    /// nominal; so is one SDL doubts, with a bias past 1024 counts or a
    /// sensitivity more than half off the usual one, which takes in a
    /// zero range, whose scale is infinite, and a zero speed, whose scale
    /// is nothing. So is one whose scale is no number at all, a zero
    /// speed over a zero range, which SDL's check lets through, and would
    /// hand a game as no number.
    pub(crate) fn of(report: &[u8]) -> Self {
        if report.len() < CALIBRATION_LEAST {
            return Self::NOMINAL;
        }
        let at = |place: usize| {
            let byte = |at: usize| report.get(at).copied().unwrap_or(0);
            i32::from(i16::from_le_bytes([byte(place), byte(place + 1)]))
        };
        let biases = [at(1), at(3), at(5)];
        let points = [(at(7), at(9)), (at(11), at(13)), (at(15), at(17))];
        // In i32, as SDL's arithmetic is in int: nothing here overflows.
        let speed = (at(19) + at(21)) as f32 * GYRO_RESOLUTION;
        let gyro = std::array::from_fn(|axis| {
            let (plus, minus) = points[axis];
            Axis {
                bias: biases[axis] as f32,
                scale: speed / (plus - minus) as f32 / GYRO_RESOLUTION,
            }
        });
        let accel = [(23, 25), (27, 29), (31, 33)].map(|(plus, minus)| {
            let (plus, minus) = (at(plus), at(minus));
            let range = plus - minus;
            Axis {
                // Truncated toward zero, as in C.
                bias: (plus - range / 2) as f32,
                scale: 2.0 * ACCEL_RESOLUTION / range as f32 / ACCEL_RESOLUTION,
            }
        });
        let read = Self { gyro, accel };
        if read.is_believed() {
            read
        } else {
            Self::NOMINAL
        }
    }

    /// SDL's check, every bias within 1024 counts and every sensitivity
    /// within half of the usual one, made so that a sensitivity that is
    /// no number fails it.
    fn is_believed(&self) -> bool {
        let gyro = self
            .gyro
            .iter()
            .map(|axis| (axis, GYRO_USUAL, GYRO_RESOLUTION));
        let accel = self
            .accel
            .iter()
            .map(|axis| (axis, ACCEL_USUAL, ACCEL_RESOLUTION));
        gyro.chain(accel).all(|(axis, usual, resolution)| {
            let sensitivity = axis.scale * resolution;
            axis.bias.abs() <= 1024.0 && (1.0 - sensitivity / usual).abs() <= 0.5
        })
    }

    /// The turning in degrees a second and the acceleration in g that
    /// `sample`'s counts stand for.
    fn apply(&self, sample: &Sample) -> ([f32; 3], [f32; 3]) {
        let each = |axes: &[Axis; 3], counts: &[i16; 3]| {
            std::array::from_fn(|axis| axes[axis].value(counts[axis]))
        };
        (
            each(&self.gyro, &sample.gyro),
            each(&self.accel, &sample.accel),
        )
    }
}

/// What a reading says, for the pad state: the motion, the touchpad and
/// whether it is clicked.
pub(crate) type Reading = (Motion, Touchpad, bool);

/// A DualSense's raw node, read for its motion, touchpad and click.
///
/// It is a duplicate of the descriptor the pad's raw node is written on,
/// made non-blocking: the two share what the kernel keeps of the open
/// node, its flag among it, which the raw node's writes and requests
/// don't look at (`hidraw_write` and `hidraw_ioctl` in
/// `drivers/hid/hidraw.c`); only its reads do.
pub(crate) struct Reader {
    raw: OwnedFd,
    calibration: Calibration,
    /// Where each report is read to: the pad's own, so reading allocates
    /// nothing.
    buffer: [u8; BUFFER],
    /// The pad's time at its last reading; none before its first.
    last: Option<u32>,
    /// Thirds of a microsecond of the pad's time not yet counted into an
    /// interval, so that intervals in whole microseconds add up to the
    /// pad's time.
    carry: u64,
    /// How many readings the pad has given, wrapping.
    readings: u32,
}

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader")
            .field("calibration", &self.calibration)
            .field("readings", &self.readings)
            .finish_non_exhaustive()
    }
}

impl Reader {
    /// Reads a DualSense's raw node as well as writing it: a duplicate of
    /// `raw`, made non-blocking, and the pad's calibration read from it.
    /// The request for the calibration waits on the pad, so this is for
    /// the control side. A calibration that can't be read is nominal, as
    /// SDL takes one.
    ///
    /// # Errors
    ///
    /// Fails if the descriptor can't be duplicated or made non-blocking.
    pub(crate) fn new(raw: BorrowedFd<'_>) -> io::Result<Self> {
        let raw = raw.try_clone_to_owned()?;
        let flags = rustix::fs::fcntl_getfl(&raw)?;
        rustix::fs::fcntl_setfl(&raw, flags | OFlags::NONBLOCK)?;
        let calibration = hidraw::feature(raw.as_fd(), CALIBRATION, CALIBRATION_SIZE)
            .map_or(Calibration::NOMINAL, |report| Calibration::of(&report));
        Ok(Self::with(raw, calibration))
    }

    fn with(raw: OwnedFd, calibration: Calibration) -> Self {
        Self {
            raw,
            calibration,
            buffer: [0; BUFFER],
            last: None,
            carry: 0,
            readings: 0,
        }
    }

    /// Reads every report waiting, without waiting for more, and calls
    /// `reading` with what each input report says; any other report is
    /// passed over. Allocates nothing, and nothing a report says can make
    /// it panic.
    ///
    /// # Errors
    ///
    /// [`PadError::Gone`] when the node has gone from under the pad
    /// (`ENODEV`); [`PadError::Io`] for any other error but a read that
    /// would wait, which ends the reading without one. A raw node whose
    /// device has gone, with nothing left to read, answers `EIO`.
    pub(crate) fn read(&mut self, mut reading: impl FnMut(Reading)) -> Result<(), PadError> {
        loop {
            match rustix::io::read(&self.raw, &mut self.buffer) {
                Ok(length) => {
                    if let Some(sample) = self.buffer.get(..length).and_then(Sample::parse) {
                        reading(self.take(&sample));
                    }
                }
                // A signal can't cut short a read that never waits; were
                // one to, the node stays ready and is read again.
                Err(Errno::AGAIN | Errno::INTR) => return Ok(()),
                Err(Errno::NODEV) => return Err(PadError::Gone),
                Err(error) => return Err(PadError::Io(error.into())),
            }
        }
    }

    /// What a sample says, counted as the pad's next reading.
    fn take(&mut self, sample: &Sample) -> Reading {
        let interval_us = match self.last {
            None => 0,
            Some(last) => {
                let ticks = u64::from(sample.time.wrapping_sub(last)) + self.carry;
                self.carry = ticks % 3;
                // Under 2³² ticks and a carry, so under 2³² thirds.
                u32::try_from(ticks / 3).unwrap_or(u32::MAX)
            }
        };
        self.last = Some(sample.time);
        self.readings = self.readings.wrapping_add(1);
        let (gyro, accel) = self.calibration.apply(sample);
        (
            Motion::new(gyro, accel, self.readings, interval_us),
            sample.touchpad(),
            sample.click,
        )
    }
}

impl AsFd for Reader {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.raw.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dualsense::{calibration, count, first_report, gyro_counts};
    use ctrlassist_core::{PadState, gamepad};

    /// The calibration report of the DualSense the spike was run with,
    /// as it read it off the pad: biases of 4, 15 and -5 counts.
    const A_PAD: &str =
        "0504000f00fbff902275ddb92267dd80226edd1c021c02f71ff4df1b1f40dfea1fd9df090000000000";

    fn bytes(hex: &str) -> Vec<u8> {
        (0..hex.len() / 2)
            .map(|at| u8::from_str_radix(&hex[at * 2..at * 2 + 2], 16).unwrap())
            .collect()
    }

    /// A USB input report with these counts, sensor time, contacts and
    /// click, and the rest at rest.
    fn usb(
        gyro: [i16; 3],
        accel: [i16; 3],
        time: u32,
        contacts: [[u8; 4]; 2],
        click: bool,
    ) -> Vec<u8> {
        let mut report = first_report(&PadState::NEUTRAL).to_vec();
        for (axis, value) in gyro.iter().enumerate() {
            report[GYRO + 2 * axis..GYRO + 2 * axis + 2].copy_from_slice(&value.to_le_bytes());
        }
        for (axis, value) in accel.iter().enumerate() {
            report[ACCEL + 2 * axis..ACCEL + 2 * axis + 2].copy_from_slice(&value.to_le_bytes());
        }
        report[SENSOR_TIME..SENSOR_TIME + 4].copy_from_slice(&time.to_le_bytes());
        report[CONTACTS..CONTACTS + 4].copy_from_slice(&contacts[0]);
        report[CONTACTS + 4..CONTACTS + 8].copy_from_slice(&contacts[1]);
        if click {
            report[BUTTONS2] |= TOUCHPAD_CLICK;
        }
        report
    }

    /// The same report as the pad sends it over Bluetooth: its number,
    /// a sequence number and tag, the body, and a checksum's room.
    fn bluetooth(usb: &[u8]) -> Vec<u8> {
        let mut report = vec![BLUETOOTH_REPORT, 0x77];
        report.extend_from_slice(&usb[1..]);
        report.resize(BLUETOOTH_SIZE, 0xcc);
        report
    }

    const LIFTED: [u8; 4] = [NO_CONTACT, 0, 0, 0];

    #[test]
    fn a_usb_and_a_bluetooth_report_say_the_same() {
        // A finger at 717 across and 45 down, as the spike recorded one.
        let touch = [
            0x05,
            717u16 as u8,
            ((717 >> 8) as u8) | ((45 & 0x0f) << 4),
            45 >> 4,
        ];
        let report = usb(
            [3, 7, 1],
            [390, 7760, 1347],
            1_705_776_474,
            [touch, LIFTED],
            true,
        );
        let sample = Sample::parse(&report).unwrap();
        assert_eq!(
            sample,
            Sample {
                gyro: [3, 7, 1],
                accel: [390, 7760, 1347],
                time: 1_705_776_474,
                contacts: [touch, LIFTED],
                click: true,
            }
        );
        assert_eq!(Sample::parse(&bluetooth(&report)), Some(sample));
        let [first, second] = sample.touchpad().contacts();
        assert!(first.down() && !second.down());
        assert!((first.x() * TOUCHPAD_X - 717.0).abs() < 1e-3);
        assert!((first.y() * TOUCHPAD_Y - 45.0).abs() < 1e-3);
    }

    #[test]
    fn short_long_and_other_reports_are_passed_over() {
        let report = usb([1, 2, 3], [4, 5, 6], 7, [LIFTED; 2], false);
        let bluetooth = bluetooth(&report);
        assert!(Sample::parse(&[]).is_none());
        assert!(Sample::parse(&report[..INPUT_SIZE - 1]).is_none());
        assert!(Sample::parse(&[&report[..], &[0]].concat()).is_none());
        assert!(Sample::parse(&bluetooth[..BLUETOOTH_SIZE - 1]).is_none());
        // The short report a pad on Bluetooth sends before its full mode.
        let mut short = [0u8; 10];
        short[0] = INPUT_REPORT;
        assert!(Sample::parse(&short).is_none());
        // A report of another number, of either size.
        let mut other = report;
        other[0] = 0x02;
        assert!(Sample::parse(&other).is_none());
        let mut other = bluetooth;
        other[0] = INPUT_REPORT;
        assert!(Sample::parse(&other).is_none());
    }

    #[test]
    fn what_the_virtual_dualsense_writes_reads_back() {
        let state = PadState::NEUTRAL
            .with(gamepad::TOUCHPAD, 1.0)
            .with_motion(Motion::new(
                [12.5, -250.0, 1000.0],
                [0.25, -1.0, 2.5],
                3,
                4000,
            ))
            .with_touchpad(Touchpad::new([
                Contact::new(true, 0.25, 0.5),
                Contact::new(false, 1.0, 0.75),
            ]));
        let neutral = Calibration::of(&calibration());
        assert_ne!(
            neutral,
            Calibration::NOMINAL,
            "SDL believes the neutral report"
        );
        let sample = Sample::parse(&first_report(&state)).unwrap();
        let (gyro, accel) = neutral.apply(&sample);
        for (read, written) in gyro.iter().zip(state.motion().gyro()) {
            // Within a count, 1080/17678 of a degree a second.
            assert!(
                (read - written).abs() <= 0.5 / gyro_counts(),
                "{read} {written}"
            );
        }
        assert_eq!(accel, state.motion().accel());
        assert!(sample.click);
        let read = sample.touchpad().contacts();
        for (read, written) in read.iter().zip(state.touchpad().contacts()) {
            assert_eq!(read.down(), written.down());
            assert!((read.x() - written.x()).abs() < 1e-3 && (read.y() - written.y()).abs() < 1e-3);
        }
    }

    #[test]
    fn a_pad_s_counts_are_its_turning_and_gravity_as_sdl_reads_them() {
        let pad = Calibration::of(&bytes(A_PAD));
        assert_ne!(pad, Calibration::NOMINAL, "SDL believes the pad's report");
        assert_eq!(pad.gyro.map(|axis| axis.bias), [4.0, 15.0, -5.0]);
        assert_eq!(pad.accel.map(|axis| axis.bias), [-10.0, -210.0, -30.0]);
        // Readings the spike recorded from the pad, each with the counts
        // it wrote under the neutral calibration, by SDL's reading.
        for (gyro, accel, written) in [
            ([5, 6, 0], [47, 7833, 1383], [1, -9, 5, 57, 8061, 1412]),
            ([3, 7, 1], [390, 7760, 1347], [-1, -8, 6, 400, 7988, 1376]),
            ([5, 7, -5], [385, 7859, 1425], [1, -8, 0, 395, 8087, 1453]),
        ] {
            let sample = Sample::parse(&usb(gyro, accel, 0, [LIFTED; 2], false)).unwrap();
            let (rates, gravity) = pad.apply(&sample);
            let counts: Vec<i16> = rates
                .iter()
                .map(|rate| count(*rate, gyro_counts()))
                .chain(gravity.iter().map(|g| count(*g, 8192.0)))
                .collect();
            assert_eq!(counts, written, "{gyro:?} {accel:?}");
        }
        // The pad at its stored bias is still.
        let sample = Sample::parse(&usb([4, 15, -5], [0; 3], 0, [LIFTED; 2], false)).unwrap();
        assert_eq!(pad.apply(&sample).0, [0.0; 3]);
    }

    /// The spike's pad's report with one value put in at a place.
    fn changed(place: usize, value: i16) -> Vec<u8> {
        let mut report = bytes(A_PAD);
        report[place..place + 2].copy_from_slice(&value.to_le_bytes());
        report
    }

    #[test]
    fn a_report_sdl_doubts_is_nominal() {
        // A pitch bias of 2000 counts.
        assert_eq!(Calibration::of(&changed(1, 2000)), Calibration::NOMINAL);
        assert_ne!(Calibration::of(&changed(1, 1000)), Calibration::NOMINAL);
        // A gyroscope whose speed is twice the usual.
        assert_eq!(Calibration::of(&changed(19, 1620)), Calibration::NOMINAL);
        // Too short for SDL to read.
        assert_eq!(Calibration::of(&bytes(A_PAD)[..34]), Calibration::NOMINAL);
        assert_ne!(Calibration::of(&bytes(A_PAD)[..35]), Calibration::NOMINAL);
        assert_eq!(Calibration::of(&[]), Calibration::NOMINAL);
    }

    #[test]
    fn no_scale_of_nothing_or_past_every_number_is_kept() {
        let mut flat = bytes(A_PAD);
        // The accelerometer's x range: plus and minus the same.
        flat[23..27].fill(0);
        // Every gyroscope range, and no speed: nothing over nothing,
        // which SDL's check lets through.
        let mut nothing = bytes(A_PAD);
        nothing[7..23].fill(0);
        // No speed over a range: a scale of nothing.
        let mut still = bytes(A_PAD);
        still[19..23].fill(0);
        for report in [flat, nothing, still] {
            let read = Calibration::of(&report);
            assert_eq!(read, Calibration::NOMINAL);
        }
        // Whatever the report, every scale is a number, and none nothing.
        for place in (1..35).step_by(2) {
            for value in [i16::MIN, -1, 0, 1, i16::MAX] {
                let read = Calibration::of(&changed(place, value));
                for axis in read.gyro.iter().chain(&read.accel) {
                    assert!(
                        axis.scale.is_finite() && axis.scale > 0.0,
                        "{place} {value}"
                    );
                    assert!(axis.bias.is_finite(), "{place} {value}");
                }
            }
        }
    }

    #[test]
    fn readings_are_counted_and_spaced_by_the_pad_s_clock() {
        let file = std::fs::File::open("/dev/null").unwrap();
        let mut reader = Reader::with(file.into(), Calibration::NOMINAL);
        let mut at = |time| {
            reader.take(
                &Sample::parse(&usb([16, 0, 0], [0, 8192, 0], time, [LIFTED; 2], false)).unwrap(),
            )
        };
        // The first has no interval; then the pad's own, across its wrap,
        // the thirds of a microsecond carried so that none is lost.
        let mut time = u32::MAX - 5999;
        let (first, touchpad, click) = at(time);
        assert_eq!((first.readings(), first.interval_us()), (1, 0));
        assert_eq!(first.gyro(), [1.0, 0.0, 0.0]);
        assert_eq!(first.accel(), [0.0, 1.0, 0.0]);
        assert_eq!((touchpad, click), (Touchpad::UNTOUCHED, false));
        let mut intervals = Vec::new();
        for step in [11_281, 11_282, 12_000, 2, 1] {
            time = time.wrapping_add(step);
            let (motion, _, _) = at(time);
            intervals.push((motion.readings(), motion.interval_us()));
        }
        // 11281 thirds is 3760 and one over, which the next 11282 makes
        // 3761; 12000 is 4000; two are none yet, and one more a whole one.
        assert_eq!(intervals, [(2, 3760), (3, 3761), (4, 4000), (5, 0), (6, 1)]);
    }
}
