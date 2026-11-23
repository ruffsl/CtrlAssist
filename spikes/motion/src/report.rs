//! A DualSense's input report, the motion and touch in it, and the
//! calibration that turns its sensors' counts into turning and gravity.
//!
//! Offsets and formulas are the kernel's `drivers/hid/hid-playstation.c`
//! (`struct dualsense_input_report`, `dualsense_get_calibration_data`,
//! `dualsense_parse_report`) and SDL's `SDL_hidapi_ps5.c`
//! (`HIDAPI_DriverPS5_LoadCalibrationData`,
//! `HIDAPI_DriverPS5_ApplyCalibrationData`), both read from their
//! upstream sources on 2026-11-23.

use ctrlassist_core::{PadState, gamepad};

/// A DualSense's input report over USB, with its number.
pub const INPUT_SIZE: usize = 64;
const USB_INPUT: u8 = 0x01;
/// Over Bluetooth, once the pad is in its full mode: a byte more before
/// the same body, and a checksum after it.
const BLUETOOTH_INPUT: u8 = 0x31;
const BLUETOOTH_INPUT_SIZE: usize = 78;

/// The calibration feature report, with its number.
pub const CALIBRATION: u8 = 0x05;
pub const CALIBRATION_SIZE: usize = 41;

// Places in the USB report, its number at 0.
const SEQUENCE: usize = 7;
const BUTTONS2: usize = 10;
const GYRO: usize = 16;
const ACCEL: usize = 22;
const TIMESTAMP: usize = 28;
const POINTS: std::ops::Range<usize> = 33..41;
/// The touchpad's click, in the third button byte: the kernel's
/// `DS_BUTTONS2_TOUCHPAD`, which it reports as `BTN_LEFT` on the
/// touchpad's own node and not on the pad's.
const TOUCHPAD_CLICK: u8 = 0x02;
/// A touch point's first byte has this set when no finger is on it.
const NO_CONTACT: u8 = 0x80;

/// The pad's report as the USB one: over Bluetooth the body is the same,
/// a byte later. Anything else, the short report a pad on Bluetooth
/// sends before its driver switches it to the full one included, is
/// `None`. A Bluetooth report whose checksum fails never gets here: the
/// kernel's driver refuses it before the raw node is given it.
#[must_use]
pub fn as_usb(report: &[u8]) -> Option<[u8; INPUT_SIZE]> {
    let mut usb = [0u8; INPUT_SIZE];
    match report {
        [USB_INPUT, ..] if report.len() == INPUT_SIZE => usb.copy_from_slice(report),
        [BLUETOOTH_INPUT, ..] if report.len() == BLUETOOTH_INPUT_SIZE => {
            usb[0] = USB_INPUT;
            usb[1..].copy_from_slice(&report[2..2 + INPUT_SIZE - 1]);
        }
        _ => return None,
    }
    Some(usb)
}

fn i16_at(report: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([report[at], report[at + 1]])
}

/// What a report says of the pad's sensors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Motion {
    /// Pitch, yaw and roll, in the gyroscope's counts.
    pub gyro: [i16; 3],
    /// x, y and z, in the accelerometer's counts.
    pub accel: [i16; 3],
    /// When the sensors were read, in thirds of a microsecond, wrapping.
    pub tick: u32,
}

impl Motion {
    #[must_use]
    pub fn of(usb: &[u8; INPUT_SIZE]) -> Self {
        Self {
            gyro: [0, 1, 2].map(|axis| i16_at(usb, GYRO + 2 * axis)),
            accel: [0, 1, 2].map(|axis| i16_at(usb, ACCEL + 2 * axis)),
            tick: u32::from_le_bytes([
                usb[TIMESTAMP],
                usb[TIMESTAMP + 1],
                usb[TIMESTAMP + 2],
                usb[TIMESTAMP + 3],
            ]),
        }
    }

    /// A pad lying flat and still: no turning, and gravity on y at the
    /// 8192 counts to the g the virtual pad's calibration says.
    pub const STILL: Self = Self {
        gyro: [0; 3],
        accel: [0, 8192, 0],
        tick: 0,
    };

    pub fn onto(self, usb: &mut [u8; INPUT_SIZE]) {
        for (axis, value) in self.gyro.iter().enumerate() {
            usb[GYRO + 2 * axis..GYRO + 2 * axis + 2].copy_from_slice(&value.to_le_bytes());
        }
        for (axis, value) in self.accel.iter().enumerate() {
            usb[ACCEL + 2 * axis..ACCEL + 2 * axis + 2].copy_from_slice(&value.to_le_bytes());
        }
        usb[TIMESTAMP..TIMESTAMP + 4].copy_from_slice(&self.tick.to_le_bytes());
    }
}

/// What a report says of the touchpad: its two points as the pad packs
/// them, and its click.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Touch {
    pub points: [u8; 8],
    pub click: bool,
}

impl Touch {
    pub const NONE: Self = Self {
        points: [NO_CONTACT, 0, 0, 0, NO_CONTACT, 0, 0, 0],
        click: false,
    };

    #[must_use]
    pub fn of(usb: &[u8; INPUT_SIZE]) -> Self {
        let mut points = [0u8; 8];
        points.copy_from_slice(&usb[POINTS]);
        Self {
            points,
            click: usb[BUTTONS2] & TOUCHPAD_CLICK != 0,
        }
    }

    pub fn onto(self, usb: &mut [u8; INPUT_SIZE]) {
        usb[POINTS].copy_from_slice(&self.points);
        usb[BUTTONS2] =
            (usb[BUTTONS2] & !TOUCHPAD_CLICK) | if self.click { TOUCHPAD_CLICK } else { 0 };
    }

    /// Each point: whether a finger is on it, and where, from 0 to 1919
    /// across and 0 to 1079 down.
    #[must_use]
    pub fn contacts(self) -> [(bool, u16, u16); 2] {
        [0, 4].map(|at| {
            let p = &self.points[at..at + 4];
            let x = u16::from(p[1]) | (u16::from(p[2] & 0x0f) << 8);
            let y = u16::from(p[2] >> 4) | (u16::from(p[3]) << 4);
            (p[0] & NO_CONTACT == 0, x, y)
        })
    }
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

fn hat(
    state: &PadState,
    negative: ctrlassist_core::Control,
    positive: ctrlassist_core::Control,
) -> i32 {
    i32::from(state.get(positive) > 0.0) - i32::from(state.get(negative) > 0.0)
}

/// The USB input report for `state`'s buttons, sticks and triggers, its
/// sensors still, its touchpad untouched and its battery full on a
/// cable: the device crate's emulator's report, copied.
#[must_use]
pub fn input_report(state: &PadState, sequence: u8) -> [u8; INPUT_SIZE] {
    let pressed = |control| state.get(control) > 0.0;
    let bit = |control, bit: u8| if pressed(control) { bit } else { 0 };
    let mut report = [0u8; INPUT_SIZE];
    report[0] = USB_INPUT;
    report[1] = stick(state.get(gamepad::LEFT_X));
    report[2] = stick(state.get(gamepad::LEFT_Y));
    report[3] = stick(state.get(gamepad::RIGHT_X));
    report[4] = stick(state.get(gamepad::RIGHT_Y));
    report[5] = trigger(state.get(gamepad::LEFT_TRIGGER));
    report[6] = trigger(state.get(gamepad::RIGHT_TRIGGER));
    report[SEQUENCE] = sequence;
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
    report[9] = bit(gamepad::LEFT_BUMPER, 0x01)
        | bit(gamepad::RIGHT_BUMPER, 0x02)
        | bit(gamepad::LEFT_TRIGGER, 0x04)
        | bit(gamepad::RIGHT_TRIGGER, 0x08)
        | bit(gamepad::SELECT, 0x10)
        | bit(gamepad::START, 0x20)
        | bit(gamepad::LEFT_THUMB, 0x40)
        | bit(gamepad::RIGHT_THUMB, 0x80);
    report[BUTTONS2] = bit(gamepad::MODE, 0x01);
    Motion::STILL.onto(&mut report);
    Touch::NONE.onto(&mut report);
    report[53] = 0x2a;
    report[54] = 0x08;
    report
}

/// One sensor axis's calibration: the physical value of a count is
/// `(count - bias) * scale`, in degrees a second for the gyroscope and
/// in g for the accelerometer.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Axis {
    pub bias: f64,
    pub scale: f64,
}

impl Axis {
    #[must_use]
    pub fn physical(self, count: i16) -> f64 {
        (f64::from(count) - self.bias) * self.scale
    }

    /// The count that stands for `physical`, the nearest one there is.
    #[must_use]
    pub fn count(self, physical: f64) -> i16 {
        (physical / self.scale + self.bias)
            .round()
            .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
    }
}

/// Whose reading of a calibration report.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reader {
    /// The kernel's driver, for the pad's motion node: the gyroscope's
    /// bias is not subtracted, and a gyroscope axis whose range is zero
    /// falls back to the sensor's whole range.
    Kernel,
    /// SDL's HIDAPI driver, for a game reading the raw node: the
    /// gyroscope's bias is subtracted, and if any axis looks wrong (a
    /// bias past 1024 counts, a scale more than half off the usual) the
    /// whole report is ignored for nominal scales.
    Sdl,
}

/// What a pad's calibration report says each sensor count is worth.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Calibration {
    /// Pitch, yaw and roll.
    pub gyro: [Axis; 3],
    /// x, y and z.
    pub accel: [Axis; 3],
}

/// The scales SDL uses for a pad whose calibration it ignores: 64/1024
/// of a degree a second to a count, and the 8192 counts to the g the
/// kernel's driver reports in.
const SDL_NOMINAL: Calibration = Calibration {
    gyro: [Axis {
        bias: 0.0,
        scale: 64.0 / 1024.0,
    }; 3],
    accel: [Axis {
        bias: 0.0,
        scale: 1.0 / 8192.0,
    }; 3],
};

impl Calibration {
    /// Reads a calibration report as `reader` does. `report` starts with
    /// its number.
    #[must_use]
    pub fn read(report: &[u8], reader: Reader) -> Self {
        if report.len() < 35 {
            // Both leave such a pad uncalibrated; the kernel refuses to
            // bind it at all, which the pad here has already got past.
            return SDL_NOMINAL;
        }
        let at = |n: usize| i32::from(i16_at(report, n));
        let biases = [at(1), at(3), at(5)];
        let ranges = [(at(7), at(9)), (at(11), at(13)), (at(15), at(17))];
        let speed_2x = f64::from(at(19) + at(21));
        let gyro = [0, 1, 2].map(|axis| {
            let bias = biases[axis];
            let (plus, minus) = ranges[axis];
            match reader {
                Reader::Kernel => {
                    let denominator = (plus - bias).abs() + (minus - bias).abs();
                    if denominator == 0 {
                        Axis {
                            bias: 0.0,
                            scale: 2048.0 / f64::from(i16::MAX),
                        }
                    } else {
                        Axis {
                            bias: 0.0,
                            scale: speed_2x / f64::from(denominator),
                        }
                    }
                }
                Reader::Sdl => Axis {
                    bias: f64::from(bias),
                    scale: speed_2x / f64::from(plus - minus),
                },
            }
        });
        let accel = [(23, 25), (27, 29), (31, 33)].map(|(plus, minus)| {
            let (plus, minus) = (at(plus), at(minus));
            let range_2g = plus - minus;
            if range_2g == 0 {
                // The kernel's fallback; SDL's sanity check below takes
                // the infinite scale for a bad report.
                Axis {
                    bias: 0.0,
                    scale: if reader == Reader::Kernel {
                        4.0 / f64::from(i16::MAX)
                    } else {
                        f64::INFINITY
                    },
                }
            } else {
                Axis {
                    // Integer division, truncating, in both.
                    bias: f64::from(plus - range_2g / 2),
                    scale: 2.0 / f64::from(range_2g),
                }
            }
        });
        let read = Self { gyro, accel };
        if reader == Reader::Sdl && !read.sdl_takes() {
            return SDL_NOMINAL;
        }
        read
    }

    /// SDL's sanity check: every bias within 1024 counts, and every
    /// scale within half of the usual one.
    fn sdl_takes(&self) -> bool {
        let usual = self.gyro.iter().map(|axis| (axis, 64.0 / 1024.0));
        let usual = usual.chain(self.accel.iter().map(|axis| (axis, 1.0 / 8192.0)));
        usual.into_iter().all(|(axis, usual)| {
            axis.bias.abs() <= 1024.0 && (1.0 - axis.scale / usual).abs() <= 0.5
        })
    }

    /// `motion`'s counts under this calibration, as the counts that say
    /// the same under `to`.
    #[must_use]
    pub fn convert(&self, motion: Motion, to: &Self) -> Motion {
        let each = |from: &[Axis; 3], to: &[Axis; 3], counts: [i16; 3]| {
            [0, 1, 2].map(|axis| to[axis].count(from[axis].physical(counts[axis])))
        };
        Motion {
            gyro: each(&self.gyro, &to.gyro, motion.gyro),
            accel: each(&self.accel, &to.accel, motion.accel),
            tick: motion.tick,
        }
    }
}

/// The calibration report the device crate's virtual DualSense gives,
/// copied: no bias, symmetric ranges, a usual pad's numbers. Its
/// gyroscope is not one count to one unit: 1080 degrees a second over
/// 17678 counts, about 16.4 counts to the degree a second.
#[must_use]
pub fn neutral_calibration() -> [u8; CALIBRATION_SIZE] {
    const VALUES: [i16; 17] = [
        0, 0, 0, 8839, -8839, 8839, -8839, 8839, -8839, 540, 540, 8192, -8192, 8192, -8192, 8192,
        -8192,
    ];
    let mut report = [0u8; CALIBRATION_SIZE];
    report[0] = CALIBRATION;
    for (place, value) in report[1..].as_chunks_mut::<2>().0.iter_mut().zip(VALUES) {
        place.copy_from_slice(&value.to_le_bytes());
    }
    report
}

/// The virtual pad's own sensor clock, advanced by the time between the
/// source pad's readings: so the gaps a game integrates over are the
/// pad's own, and the clock is the virtual pad's, to stay continuous
/// whatever pad the readings come from.
#[derive(Clone, Copy, Default, Debug)]
pub struct Clock {
    last: Option<u32>,
    now: u32,
}

impl Clock {
    /// Takes a reading made at the pad's `tick`, and says the virtual
    /// pad's tick for it. The first reading is at the clock's start.
    pub fn advance(&mut self, tick: u32) -> u32 {
        let delta = self.last.map_or(0, |last| tick.wrapping_sub(last));
        self.last = Some(tick);
        self.now = self.now.wrapping_add(delta);
        self.now
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pad's calibration, numbers of the kind a retail pad gives.
    fn a_pad() -> [u8; CALIBRATION_SIZE] {
        const VALUES: [i16; 17] = [
            -3, 5, 2, 8875, -8820, 8901, -8790, 8850, -8860, 540, 540, 8310, -8070, 8250, -8130,
            8200, -8190,
        ];
        let mut report = [0u8; CALIBRATION_SIZE];
        report[0] = CALIBRATION;
        for (place, value) in report[1..].as_chunks_mut::<2>().0.iter_mut().zip(VALUES) {
            place.copy_from_slice(&value.to_le_bytes());
        }
        report
    }

    #[test]
    fn a_bluetooth_report_is_the_usb_one_a_byte_later() {
        let mut bluetooth = [0u8; BLUETOOTH_INPUT_SIZE];
        bluetooth[0] = BLUETOOTH_INPUT;
        bluetooth[1] = 0x77;
        for (index, byte) in bluetooth[2..].iter_mut().enumerate() {
            *byte = index as u8;
        }
        let usb = as_usb(&bluetooth).unwrap();
        assert_eq!(usb[0], USB_INPUT);
        assert_eq!(usb[1], 0);
        assert_eq!(usb[GYRO], (GYRO - 1) as u8);
        assert_eq!(usb[63], 62);
        assert!(as_usb(&bluetooth[..10]).is_none(), "the short report");
        let mut short = [0u8; 10];
        short[0] = USB_INPUT;
        assert!(as_usb(&short).is_none());
    }

    #[test]
    fn motion_and_touch_go_back_where_they_came_from() {
        let mut usb = input_report(&PadState::NEUTRAL, 3);
        let motion = Motion {
            gyro: [-1, 300, -32768],
            accel: [8192, -5, 32767],
            tick: 0xdead_beef,
        };
        motion.onto(&mut usb);
        assert_eq!(Motion::of(&usb), motion);
        let touch = Touch {
            points: [0x05, 0x80, 0x37, 0x21, 0x86, 0xff, 0xf7, 0x43],
            click: true,
        };
        touch.onto(&mut usb);
        assert_eq!(Touch::of(&usb), touch);
        assert_eq!(usb[BUTTONS2] & 0x01, 0, "the PS button is left alone");
        let [first, second] = touch.contacts();
        assert_eq!(first, (true, 0x780, 0x213));
        assert_eq!(second, (false, 0x7ff, 0x43f));
        // The rest of the report is the pad state's.
        assert_eq!(usb[..8], input_report(&PadState::NEUTRAL, 3)[..8]);
    }

    #[test]
    fn the_neutral_calibration_reads_the_same_to_both_and_sdl_takes_it() {
        let neutral = neutral_calibration();
        let kernel = Calibration::read(&neutral, Reader::Kernel);
        let sdl = Calibration::read(&neutral, Reader::Sdl);
        assert_eq!(kernel, sdl);
        assert_ne!(sdl, SDL_NOMINAL, "SDL's sanity check passes");
        assert!((kernel.gyro[0].scale - 1080.0 / 17678.0).abs() < 1e-12);
        assert_eq!(
            kernel.accel[1],
            Axis {
                bias: 0.0,
                scale: 1.0 / 8192.0
            }
        );
        // Every count survives the trip there and back.
        for count in [i16::MIN, -1234, -1, 0, 1, 8192, i16::MAX] {
            for axis in kernel.gyro.iter().chain(&kernel.accel) {
                assert_eq!(axis.count(axis.physical(count)), count);
            }
        }
    }

    #[test]
    fn a_pad_s_counts_become_the_virtual_pad_s_for_the_same_turn() {
        let pad = a_pad();
        let neutral = Calibration::read(&neutral_calibration(), Reader::Sdl);
        let sdl = Calibration::read(&pad, Reader::Sdl);
        let kernel = Calibration::read(&pad, Reader::Kernel);
        // SDL subtracts the gyroscope's bias; the kernel doesn't.
        assert_eq!(sdl.gyro[0].bias, -3.0);
        assert_eq!(kernel.gyro[0].bias, 0.0);
        // The kernel's denominator measures from the bias; SDL's from
        // minus to plus. Here they agree, the bias being between.
        assert!((sdl.gyro[1].scale - kernel.gyro[1].scale).abs() < 1e-12);
        let resting = Motion {
            gyro: [-3, 5, 2],
            accel: [120, 8250, 5],
            tick: 9,
        };
        let by_sdl = sdl.convert(resting, &neutral);
        assert_eq!(by_sdl.gyro, [0, 0, 0], "the bias is the pad at rest");
        assert_eq!(by_sdl.tick, 9);
        let by_kernel = kernel.convert(resting, &neutral);
        assert_ne!(by_kernel.gyro, [0, 0, 0]);
        // Gravity on y: the plus point is 1 g, so 8192 counts here.
        assert_eq!(by_sdl.accel[1], 8192);
        assert_eq!(by_kernel.accel, by_sdl.accel);
        // 100 degrees a second of yaw, as each reads it.
        let yaw = sdl.gyro[1].count(100.0);
        let turned = sdl.convert(
            Motion {
                gyro: [0, yaw, 0],
                ..resting
            },
            &neutral,
        );
        assert!((neutral.gyro[1].physical(turned.gyro[1]) - 100.0).abs() < 0.07);
    }

    #[test]
    fn a_report_sdl_doubts_is_read_as_nominal() {
        let mut report = a_pad();
        // A pitch bias of 2000 counts.
        report[1..3].copy_from_slice(&2000i16.to_le_bytes());
        assert_eq!(Calibration::read(&report, Reader::Sdl), SDL_NOMINAL);
        assert_ne!(Calibration::read(&report, Reader::Kernel), SDL_NOMINAL);
        // A range of nothing.
        let mut flat = a_pad();
        flat[23..27].fill(0);
        assert_eq!(Calibration::read(&flat, Reader::Sdl), SDL_NOMINAL);
        let kernel = Calibration::read(&flat, Reader::Kernel);
        assert_eq!(kernel.accel[0].scale, 4.0 / 32767.0);
        assert_eq!(
            Calibration::read(&[CALIBRATION; 10], Reader::Sdl),
            SDL_NOMINAL
        );
    }

    #[test]
    fn the_clock_starts_at_zero_and_follows_the_pad_round_its_wrap() {
        let mut clock = Clock::default();
        assert_eq!(clock.advance(u32::MAX - 5999), 0);
        assert_eq!(clock.advance(u32::MAX - 2999), 3000);
        // 4 ms at three ticks a microsecond, across the wrap.
        assert_eq!(clock.advance(9000), 3000 + 12000);
    }
}
