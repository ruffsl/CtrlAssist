//! A Steam Controller's reports, read and written as plain code that
//! opens nothing: bytes in, values out, and values in, bytes out; and
//! what they say made the pad's state, by a [`Reader`] of its raw node.
//!
//! The pad is Valve's 2026 Steam Controller, which SDL calls Triton and
//! the kernel Ibex. It is read from its raw HID node, as SDL reads it,
//! and not from the input devices the kernel's driver makes of it, which
//! leave out much of what it reports. Its reports are the same on every
//! transport: a cable, the Puck, and Bluetooth.
//!
//! The pad is read from its raw node and nothing else. While anything
//! holds that node open the kernel's driver removes the pad's gamepad and
//! motion devices, whose readers get `ENODEV`, and sends the pad nothing
//! itself (`steam_client_ll_open` and `steam_work_connect_cb`,
//! `hid-steam.c:1460-1484, 1651-1660`): so whoever holds it turns lizard
//! mode off, by [`take_over`], and sends the rumble again, by
//! [`RUMBLE_AGAIN`]. The pad's writer does both (`output.rs`).
//!
//! What the pad state has no place for is not carried: a trackpad's
//! pressure, the pad's own reckoning of its orientation and the battery.
//! A virtual pad's rumble reaches the pad as a level, as it reaches every
//! other pad; the pad's haptics beyond rumble ([`Haptic`]) reach it only
//! from a virtual Steam Controller, whole, as a reader wrote them there.
//!
//! Two sources say what the bytes are, and this file is meant to be read
//! against them:
//!
//! - the kernel's driver, `drivers/hid/hid-steam.c` at Linux 7.3.4, cited
//!   here as `hid-steam.c` with its line numbers;
//! - SDL's driver, `src/joystick/hidapi/SDL_hidapi_steam_triton.c` on
//!   SDL's main branch at `3ebae2a7`, cited as `SDL_hidapi_steam_triton.c`
//!   with its line numbers, and the Valve headers beside it,
//!   `src/joystick/hidapi/steam/controller_structs.h` and
//!   `controller_constants.h`, cited by the names they define.
//!
//! Where they disagree or neither says, the choice made here is written
//! where it is made; what only a real pad can settle is said so.
//!
//! Every report is read with its number at 0, as the raw node hands it
//! over, and every offset below counts that byte, as the kernel's do.

// Nothing a report says may panic the input thread: no indexing, no
// arithmetic that can overflow, no cast that can truncate.
#![warn(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::string_slice,
    clippy::cast_possible_truncation
)]

use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::time::Duration;

use ctrlassist_core::{Contact, Control, Level, Motion, PadState, Touchpad, gamepad};
use rustix::fs::OFlags;
use rustix::io::Errno;

use crate::PadError;

/// Valve's vendor number (`USB_VENDOR_ID_VALVE`, `hid-ids.h:1392`).
pub(crate) const VENDOR: u16 = 0x28de;
/// The pad on a USB cable: Ibex to the kernel, Triton to SDL
/// (`USB_DEVICE_ID_STEAM_CONTROLLER_IBEX`, `hid-ids.h:1396`; the device
/// table, `hid-steam.c:2751-2755`).
pub(crate) const PRODUCT_CABLE: u16 = 0x1302;
/// The pad over Bluetooth LE (`USB_DEVICE_ID_STEAM_CONTROLLER_IBEX_BLE`,
/// `hid-ids.h:1397`, matched as a Bluetooth device at
/// `hid-steam.c:2756-2760`).
pub(crate) const PRODUCT_BLUETOOTH: u16 = 0x1303;
/// The Puck, the USB receiver a pad joins wirelessly: Proteus
/// (`USB_DEVICE_ID_STEAM_CONTROLLER_PROTEUS`, `hid-ids.h:1398`,
/// `hid-steam.c:2761-2765`). A pad behind it carries this number, not
/// its own.
pub(crate) const PRODUCT_PUCK: u16 = 0x1304;
/// The receiver built into a Steam Machine: Nereid
/// (`USB_DEVICE_ID_STEAM_CONTROLLER_NEREID`, `hid-ids.h:1399`,
/// `hid-steam.c:2766-2770`). SDL treats it as it does the Puck
/// (`IsProteusDongle`, `SDL_hidapi_steam_triton.c:124-128`).
pub(crate) const PRODUCT_RECEIVER: u16 = 0x1305;

/// The buses, as the kernel numbers them (`linux/input.h`).
const BUS_USB: u16 = 3;
const BUS_BLUETOOTH: u16 = 5;

/// Whether a HID device with these numbers is a Steam Controller this
/// reads: the pad on a cable or over Bluetooth, or a slot of a Puck or of
/// a Steam Machine's receiver, as the kernel's device table pairs each
/// product with its bus (`hid-steam.c:2751-2770`). A slot is the pad that
/// joined it; it carries the receiver's number, not the pad's.
pub(crate) fn is_pad(bus: u16, vendor: u16, product: u16) -> bool {
    is_model(vendor, product)
        && match product {
            PRODUCT_CABLE | PRODUCT_PUCK | PRODUCT_RECEIVER => bus == BUS_USB,
            _ => bus == BUS_BLUETOOTH,
        }
}

/// Whether `vendor` and `product` are one of the Steam Controller's, on
/// whichever bus: for a pad remembered, whose bus isn't known.
#[must_use]
pub fn is_model(vendor: u16, product: u16) -> bool {
    vendor == VENDOR
        && matches!(
            product,
            PRODUCT_CABLE | PRODUCT_PUCK | PRODUCT_RECEIVER | PRODUCT_BLUETOOTH
        )
}

/// The state report, with the pad's orientation at its end
/// (`REPORT_ID_INPUT`, `hid-steam.c:323`; `ID_TRITON_CONTROLLER_STATE`;
/// SDL's `TritonMTUFull_t`).
pub(crate) const STATE: u8 = 0x42;
/// Its size, its number included. The kernel takes it at exactly this
/// (`hid-steam.c:2553`); SDL takes any `0x42` of 46 bytes or more and
/// reads it as the plain layout, never reading the orientation
/// (`SDL_hidapi_steam_triton.c:532-544`). This follows the kernel's size
/// as a least: a `0x42` shorter than 54 bytes is none, and one of 54 or
/// more is read with its orientation.
pub(crate) const STATE_SIZE: usize = 54;
/// The state report without the orientation, otherwise the same
/// (`REPORT_ID_INPUT2`, `hid-steam.c:325`;
/// `ID_TRITON_CONTROLLER_STATE_BLE`; SDL's `TritonMTUNoQuat_t`).
pub(crate) const STATE_PLAIN: u8 = 0x45;
/// Its size (`hid-steam.c:2572`).
pub(crate) const STATE_PLAIN_SIZE: usize = 46;
/// The state report SDL's main branch calls the new Ibex packet: a
/// trackpad time put in before the trackpads, and the sensor time
/// narrowed to 16 bits of 32 µs (`ID_TRITON_CONTROLLER_STATE_TIMESTAMP`;
/// SDL's `TritonMTUNoQuat32TS_t`; read at
/// `SDL_hidapi_steam_triton.c:545-556`). The kernel ignores it.
pub(crate) const STATE_TIMESTAMPED: u8 = 0x47;
/// Its size: SDL's struct and its number.
pub(crate) const STATE_TIMESTAMPED_SIZE: usize = 46;
/// The battery report (`REPORT_ID_BATTERY`, `hid-steam.c:324`;
/// `ID_TRITON_BATTERY_STATUS`).
pub(crate) const BATTERY: u8 = 0x43;
/// Its size (`hid-steam.c:2591`; SDL's `TritonBatteryStatus_t` and its
/// number).
pub(crate) const BATTERY_SIZE: usize = 15;
/// A wireless event: a pad joining or leaving a receiver's slot
/// (`REPORT_ID_WIRELESS_EVENT`, `hid-steam.c:326`;
/// `ID_TRITON_WIRELESS_STATUS`).
pub(crate) const WIRELESS: u8 = 0x79;
/// Another report SDL reads as a wireless event, the same way
/// (`ID_TRITON_WIRELESS_STATUS_X`, `SDL_hidapi_steam_triton.c:563-569`).
/// The kernel doesn't know it.
pub(crate) const WIRELESS_X: u8 = 0x46;
/// A wireless event's size (`hid-steam.c:2606`).
pub(crate) const WIRELESS_SIZE: usize = 2;

/// The rumble output report (`REPORT_ID_HAPTIC_RUMBLE`,
/// `hid-steam.c:329`; `ID_OUT_REPORT_HAPTIC_RUMBLE`).
pub(crate) const RUMBLE: u8 = 0x80;
/// Its size, its number included (`hid_hw_output_report(…, 10)`,
/// `hid-steam.c:813`; `HID_RUMBLE_OUTPUT_REPORT_BYTES`).
pub(crate) const RUMBLE_SIZE: usize = 10;

/// The feature report commands go to the pad by
/// (`REPORT_ID_FEATURES_CONTROLLER`, `hid-steam.c:319`).
pub(crate) const FEATURE_PAD: u8 = 1;
/// The feature report commands go to a receiver by
/// (`REPORT_ID_FEATURES_DONGLE`, `hid-steam.c:320`).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "a receiver's own commands, its wireless state among them, are for when a Puck is read whole"
    )
)]
pub(crate) const FEATURE_RECEIVER: u8 = 2;
/// A feature report's size, its number included: 64 bytes, the kernel
/// refusing to send one declared shorter (`hid-steam.c:532-534`) and
/// SDL sending exactly this
/// (`HID_FEATURE_REPORT_BYTES`, `SDL_hidapi_steam_triton.c:135`).
pub(crate) const FEATURE_SIZE: usize = 64;

/// How often the pad's rumble is sent again while either motor runs: the
/// pad stops by itself some 50 ms after the last rumble report, so SDL
/// sends it every 40 ms (`TRITON_RUMBLE_RESEND_INTERVAL_MS` and its
/// comment, `SDL_hidapi_steam_triton.c:44-45`, used at 507-511). This is
/// the pad's protocol, not a choice of CtrlAssist's. The kernel sends it
/// every 50 ms instead (`HZ / 20`, `hid-steam.c:855-856`).
pub(crate) const RUMBLE_AGAIN: Duration = Duration::from_millis(40);

/// The commands a feature report carries, by their first byte
/// (`hid-steam.c:91-144`; `controller_constants.h`).
const CLEAR_DIGITAL_MAPPINGS: u8 = 0x81;
const SET_DEFAULT_DIGITAL_MAPPINGS: u8 = 0x85;
const SET_SETTINGS_VALUES: u8 = 0x87;
const LOAD_DEFAULT_SETTINGS: u8 = 0x8e;

/// The settings this file writes, by number: the kernel's and SDL's
/// enums agree, each counting from 0 (`hid-steam.c:147-247`, these at
/// 158, 205 and 234; `controller_constants.h`).
const SETTING_LIZARD_MODE: u8 = 9;
const SETTING_IMU_MODE: u8 = 48;
/// The pad's watchdog: on, the pad puts lizard mode back by itself a
/// while after it was last turned off, for a Steam that has gone. How
/// long it waits is published nowhere.
const SETTING_STEAM_WATCHDOG_ENABLE: u8 = 71;
const WATCHDOG_OFF: u16 = 0;
/// Lizard mode off (`steam_set_lizard_mode`, `hid-steam.c:897`;
/// `LIZARD_MODE_OFF`).
const LIZARD_MODE_OFF: u16 = 0;
/// The motion sensors' raw accelerometer and gyroscope sent, and nothing
/// of the pad's own working out of them (`SETTING_IMU_MODE_SEND_RAW_ACCEL
/// | SETTING_IMU_MODE_SEND_RAW_GYRO`, `hid-steam.c:293-301`, written at
/// 958-960; SDL's `SETTING_GYRO_MODE_SEND_RAW_*`).
const IMU_MODE_RAW: u16 = 0x18;
const IMU_MODE_OFF: u16 = 0;
/// The most settings one write carries: three bytes each after the
/// report's number, the command and the length. SDL's struct holds 20
/// (`MsgSetSettingsValues`), and the kernel's 64-byte buffer as many
/// (`steam_write_settings`, `hid-steam.c:593-613`).
const MOST_SETTINGS: usize = 20;

/// The full scale of a stick, a trackpad's place and a trigger: the
/// kernel declares each axis to this, and reads the report's -32768 as
/// -32767, so that the range is the same each side and can be negated
/// (`steam_le16` and its comment, `hid-steam.c:1888-1898`; the ranges
/// at 1159-1176). The same is done here.
const FULL: i16 = 32767;
/// A trackpad's pressure at its fullest, as SDL reads it: it divides by
/// this (`SDL_hidapi_steam_triton.c:273, 285`). The kernel doesn't read
/// pressure at all. A pressure past it is read as full.
const FULL_PRESSURE: f32 = 32768.0;
/// The gyroscope's degrees a second to the count: 2000 at full scale of
/// 16 bits (`SDL_hidapi_steam_triton.c:294-296`; the kernel's comment
/// at `hid-steam.c:80`, which rounds the resolution to 16).
const DEGREES_PER_COUNT: f32 = 2000.0 / 32768.0;
/// The accelerometer's counts to a g: 2 g at full scale of 16 bits
/// (`STEAM_ACCEL_RES_PER_G`, `hid-steam.c:76`;
/// `SDL_hidapi_steam_triton.c:299-301`).
const COUNTS_PER_G: f32 = 16384.0;
/// One tick of the narrow sensor time of [`STATE_TIMESTAMPED`], in
/// microseconds (`SDL_hidapi_steam_triton.c:347-348`).
const NARROW_TICK_US: u32 = 32;

/// What an input report was.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Input {
    /// The pad's state: [`STATE`], [`STATE_PLAIN`] or
    /// [`STATE_TIMESTAMPED`].
    State(Sample),
    /// Its battery: [`BATTERY`].
    Battery(Battery),
    /// A wireless event: [`WIRELESS`] or [`WIRELESS_X`].
    Wireless(Wireless),
}

/// Reads an input report, its number at 0. A report of another number,
/// or shorter than its number's size, is none; bytes past the size are
/// passed over, as SDL passes them over (it checks only that a report is
/// at least its struct's size, `SDL_hidapi_steam_triton.c:540, 552, 558,
/// 565`). The kernel takes each at exactly its size (`hid-steam.c:2553,
/// 2572, 2591, 2606`). Nothing in any bytes can make this panic.
pub(crate) fn parse(report: &[u8]) -> Option<Input> {
    match *report.first()? {
        STATE => Sample::parse(report, Layout::Orientation).map(Input::State),
        STATE_PLAIN => Sample::parse(report, Layout::Plain).map(Input::State),
        STATE_TIMESTAMPED => Sample::parse(report, Layout::Timestamped).map(Input::State),
        BATTERY => Battery::parse(report).map(Input::Battery),
        WIRELESS | WIRELESS_X => Wireless::parse(report).map(Input::Wireless),
        _ => None,
    }
}

/// `N` bytes at `at`, if the report has them.
fn bytes<const N: usize>(report: &[u8], at: usize) -> Option<[u8; N]> {
    report.get(at..)?.first_chunk::<N>().copied()
}

fn i16_at(report: &[u8], at: usize) -> Option<i16> {
    bytes(report, at).map(i16::from_le_bytes)
}

fn u16_at(report: &[u8], at: usize) -> Option<u16> {
    bytes(report, at).map(u16::from_le_bytes)
}

fn u32_at(report: &[u8], at: usize) -> Option<u32> {
    bytes(report, at).map(u32::from_le_bytes)
}

/// One bit of the 32-bit button word, by its value in the word: a
/// button, a trackpad's click or touch, or a capacitive sensor touched.
///
/// The word is little-endian at bytes 2 to 5, and the bits are the
/// kernel's table (`hid-steam.c:2356-2390`, its byte.bit being byte 2's
/// bit 0 the word's bit 0) and SDL's `TritonButtons`
/// (`SDL_hidapi_steam_triton.c:61-101`), which agree bit for bit.
/// Bits 30 and 31 are named by neither and so not here; [`Buttons`]
/// keeps them.
///
/// The two middle buttons are named by what they do, because the
/// sources' names for them are opposite: bit 6 is the kernel's
/// `BTN_START`, which it labels "menu" (and holds for lizard mode's
/// switch, `hid-steam.c:2447`), and SDL's `TRITON_LBUTTON_VIEW`, which
/// SDL sends as START (`SDL_hidapi_steam_triton.c:180-181`); bit 14 is
/// the kernel's `BTN_SELECT`, labeled "view", and SDL's
/// `TRITON_LBUTTON_MENU`, sent as BACK (178-179). Both send bit 6 as
/// start and bit 14 as select, whatever the buttons are printed with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u32)]
pub(crate) enum Button {
    /// A, the bottom face button (kernel `BTN_A`, SDL SOUTH).
    South = 1 << 0,
    /// B, the right face button (`BTN_B`, EAST).
    East = 1 << 1,
    /// X, the left face button (`BTN_X`, WEST).
    West = 1 << 2,
    /// Y, the top face button (`BTN_Y`, NORTH).
    North = 1 << 3,
    /// The quick access button (`BTN_BASE`; `TRITON_HBUTTON_QAM`).
    QuickAccess = 1 << 4,
    /// The right stick clicked (`BTN_THUMBR`; `TRITON_LBUTTON_R3`).
    RightStickClick = 1 << 5,
    /// Start: see above.
    Start = 1 << 6,
    /// The upper right back button, R4: SDL's first paddle
    /// (`BTN_GRIPR`, "right top grip button"; `TRITON_HBUTTON_R4`).
    R4 = 1 << 7,
    /// The lower right back button, R5: SDL's third paddle
    /// (`BTN_GRIPR2`, "right bottom grip button"; `TRITON_LBUTTON_R5`).
    R5 = 1 << 8,
    /// The right bumper (`BTN_TR`; `TRITON_LBUTTON_R`).
    RightBumper = 1 << 9,
    /// The D-pad's down (`BTN_DPAD_DOWN`; `TRITON_LBUTTON_DPAD_DOWN`).
    DpadDown = 1 << 10,
    /// The D-pad's right.
    DpadRight = 1 << 11,
    /// The D-pad's left.
    DpadLeft = 1 << 12,
    /// The D-pad's up.
    DpadUp = 1 << 13,
    /// Select, or back: see above.
    Select = 1 << 14,
    /// The left stick clicked (`BTN_THUMBL`; `TRITON_LBUTTON_L3`).
    LeftStickClick = 1 << 15,
    /// The Steam button (`BTN_MODE`; `TRITON_LBUTTON_STEAM`, sent as
    /// GUIDE).
    Steam = 1 << 16,
    /// The upper left back button, L4: SDL's second paddle
    /// (`BTN_GRIPL`, "left top grip button"; `TRITON_HBUTTON_L4`).
    L4 = 1 << 17,
    /// The lower left back button, L5: SDL's fourth paddle
    /// (`BTN_GRIPL2`, "left bottom grip button"; `TRITON_LBUTTON_L5`).
    L5 = 1 << 18,
    /// The left bumper (`BTN_TL`; `TRITON_LBUTTON_L`).
    LeftBumper = 1 << 19,
    /// The right stick touched (capacitive; the kernel doesn't map it;
    /// `TRITON_RIGHT_JOYSTICK_TOUCH`).
    RightStickTouch = 1 << 20,
    /// The right trackpad touched (`TRITON_RIGHT_TOUCHPAD_TOUCH`; the
    /// kernel reads it to zero the trackpad's place, `hid-steam.c:2461`).
    RightTrackpadTouch = 1 << 21,
    /// The right trackpad clicked (`BTN_THUMB2`;
    /// `TRITON_RIGHT_TOUCHPAD_CLICK`).
    RightTrackpadClick = 1 << 22,
    /// The right trigger pressed home (`BTN_TR2`;
    /// `TRITON_RIGHT_TRIGGER_CLICK`).
    RightTriggerClick = 1 << 23,
    /// The left stick touched (`TRITON_LEFT_JOYSTICK_TOUCH`).
    LeftStickTouch = 1 << 24,
    /// The left trackpad touched (`TRITON_LEFT_TOUCHPAD_TOUCH`;
    /// `hid-steam.c:2460`).
    LeftTrackpadTouch = 1 << 25,
    /// The left trackpad clicked (`BTN_THUMB`;
    /// `TRITON_LEFT_TOUCHPAD_CLICK`).
    LeftTrackpadClick = 1 << 26,
    /// The left trigger pressed home (`BTN_TL2`;
    /// `TRITON_LEFT_TRIGGER_CLICK`).
    LeftTriggerClick = 1 << 27,
    /// The right grip touched (capacitive; `TRITON_RIGHT_GRIP_TOUCH`).
    RightGripTouch = 1 << 28,
    /// The left grip touched (capacitive; `TRITON_LEFT_GRIP_TOUCH`).
    LeftGripTouch = 1 << 29,
}

impl Button {
    /// Every bit the sources name, lowest first.
    pub(crate) const ALL: [Self; 30] = [
        Self::South,
        Self::East,
        Self::West,
        Self::North,
        Self::QuickAccess,
        Self::RightStickClick,
        Self::Start,
        Self::R4,
        Self::R5,
        Self::RightBumper,
        Self::DpadDown,
        Self::DpadRight,
        Self::DpadLeft,
        Self::DpadUp,
        Self::Select,
        Self::LeftStickClick,
        Self::Steam,
        Self::L4,
        Self::L5,
        Self::LeftBumper,
        Self::RightStickTouch,
        Self::RightTrackpadTouch,
        Self::RightTrackpadClick,
        Self::RightTriggerClick,
        Self::LeftStickTouch,
        Self::LeftTrackpadTouch,
        Self::LeftTrackpadClick,
        Self::LeftTriggerClick,
        Self::RightGripTouch,
        Self::LeftGripTouch,
    ];

    /// Its bit in the word.
    const fn mask(self) -> u32 {
        self as u32
    }
}

/// The button word as the report has it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct Buttons(u32);

impl Buttons {
    /// Whether `button`'s bit is set: pressed, or touched.
    pub(crate) const fn has(self, button: Button) -> bool {
        self.0 & button.mask() != 0
    }

    /// The whole word, the bits no source names among it.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the tests read the word whole; the pad state takes it bit by bit"
        )
    )]
    pub(crate) const fn bits(self) -> u32 {
        self.0
    }
}

/// One trackpad, as a state report has it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Trackpad {
    /// Whether a finger is on it, by its touch bit in the button word.
    pub(crate) touched: bool,
    /// Its place across, from 0 at the left edge to 1 at the right.
    ///
    /// The place, across and down, is whatever the report's fields say,
    /// touched or not: this module passes them through. What a pad
    /// reports in them while no finger is on it no source says; the
    /// kernel reports its axes at 0 then (`hid-steam.c:2466-2468`), and
    /// SDL keeps the last place touched
    /// (`SDL_hidapi_steam_triton.c:264-268`). Whoever builds the pad state
    /// keeps the last touched place itself, as a lifted contact keeps the
    /// place its finger left (`ctrlassist_core::Contact::new`).
    pub(crate) x: f32,
    /// Its place down, from 0 at the top edge to 1 at the bottom, as a
    /// DualSense's touchpad is read.
    ///
    /// The sources conflict on which way the report's value grows. SDL
    /// reads its larger values as higher (`-sLeftPadY / 65536 + 0.5`,
    /// `SDL_hidapi_steam_triton.c:267, 279`). The kernel passes the value
    /// unchanged as `ABS_HAT0Y` and `ABS_HAT1Y` (`hid-steam.c:2465,
    /// 2473`), where by evdev's convention for a hat axis larger is
    /// lower; yet it negates the 2015 pad's left trackpad on the same
    /// axis (`hid-steam.c:2058`), so its 2026 path may simply not have
    /// been checked. This follows SDL, whose driver was written for this
    /// pad; only a real pad can settle it.
    pub(crate) y: f32,
    /// How hard it is pressed, 0 to 1, as SDL reads it ([`FULL_PRESSURE`]).
    /// Whether it means anything, and its real range, only a real pad
    /// can say.
    pub(crate) pressure: f32,
}

/// When the motion sensors were read, by the pad's own clock.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SensorTime {
    /// Microseconds in 32 bits, as [`STATE`] and [`STATE_PLAIN`] carry it
    /// (`MSC_TIMESTAMP`, `hid-steam.c:2492`; SDL takes the difference as
    /// microseconds, `SDL_hidapi_steam_triton.c:290`).
    Wide(u32),
    /// Ticks of 32 µs in 16 bits, as [`STATE_TIMESTAMPED`] carries it
    /// (`SDL_hidapi_steam_triton.c:347-348`).
    Narrow(u16),
}

impl SensorTime {
    /// The microseconds from `earlier` to this time, across the clock's
    /// wrap at its own width; the only way to space two readings, since
    /// a narrow time in microseconds wraps at 2,097,152, where nothing
    /// subtracting two of them would know to. None when the two are of
    /// different widths: one clock's time says nothing of the other's.
    ///
    /// The wide clock wraps as SDL takes it, in 32 bits
    /// (`SDL_hidapi_steam_triton.c:290`). The narrow one is wrapped at 16
    /// bits here on purpose, where SDL doesn't: SDL subtracts its two
    /// 16-bit times as `int`, so a reading across the wrap comes out
    /// negative and is cast to an interval of over an hour
    /// (`SDL_hidapi_steam_triton.c:348`).
    pub(crate) fn since(self, earlier: Self) -> Option<u32> {
        match (earlier, self) {
            (Self::Wide(earlier), Self::Wide(now)) => Some(now.wrapping_sub(earlier)),
            (Self::Narrow(earlier), Self::Narrow(now)) => {
                Some(u32::from(now.wrapping_sub(earlier)).saturating_mul(NARROW_TICK_US))
            }
            _ => None,
        }
    }
}

/// What one state report says, in the units the rest of this crate uses.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Sample {
    /// The report's sequence number, byte 1, wrapping.
    pub(crate) sequence: u8,
    /// The button word.
    pub(crate) buttons: Buttons,
    /// The left and right triggers, from 0 at rest to 1, by the kernel's
    /// declared range of 0 to 32767 (`hid-steam.c:1168-1169`); SDL
    /// takes the same range (`* 2 - 32768`,
    /// `SDL_hidapi_steam_triton.c:237-240`). A value below 0 is read as
    /// 0. Whether a real trigger reaches 32767 only a real pad can say.
    pub(crate) triggers: [f32; 2],
    /// The left stick, x then y, each -1 to 1, right and down positive.
    ///
    /// Down is positive because it is in the rest of this crate: a
    /// pad's `ABS_Y` is taken unchanged as its stick's y
    /// (`layout.rs`), which the kernel's gamepad layout has growing
    /// downward, and the virtual DualSense writes -1 as its report's 0,
    /// the top (`dualsense.rs`). The report's y grows upward, which both
    /// sources negate (the kernel's sign of -1, `hid-steam.c:2423, 2425`;
    /// SDL's `-sLeftStickY`, `SDL_hidapi_steam_triton.c:243-248`).
    pub(crate) left_stick: [f32; 2],
    /// The right stick, as the left.
    pub(crate) right_stick: [f32; 2],
    /// The left trackpad.
    pub(crate) left_trackpad: Trackpad,
    /// The right trackpad.
    pub(crate) right_trackpad: Trackpad,
    /// The rate of turning about x, y and z, in degrees a second, on the
    /// axes the pad state uses: see [`motion`].
    pub(crate) gyro: [f32; 3],
    /// The acceleration along x, y and z, in g, gravity included, on the
    /// axes the pad state uses: see [`motion`].
    pub(crate) accel: [f32; 3],
    /// When the sensors were read.
    pub(crate) time: SensorTime,
    /// When the trackpads were read, in a unit neither source says,
    /// which neither reads: only [`STATE_TIMESTAMPED`] has it
    /// (`unTrackpadTimestamp`).
    pub(crate) trackpad_time: Option<u16>,
    /// The pad's own reckoning of its orientation, a quaternion's w, x,
    /// y and z in counts whose scale neither source says, which neither
    /// reads: only [`STATE`] has it (`hid-steam.c:2351-2354`;
    /// `TritonMTUIMU_t`).
    pub(crate) orientation: Option<[i16; 4]>,
}

/// The three layouts of a state report. They agree up to the trackpads,
/// and on the sensors' six values at 34 to 45.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Layout {
    /// [`STATE`], laid out at `hid-steam.c:2324-2354`.
    Orientation,
    /// [`STATE_PLAIN`], the same without its last 8 bytes.
    Plain,
    /// [`STATE_TIMESTAMPED`], laid out as SDL's `TritonMTUNoQuat32TS_t`.
    Timestamped,
}

// Places in a state report, its number at 0, from the kernel's table
// (`hid-steam.c:2324-2354`), which SDL's structs agree with, each a
// byte on for the report's number.
const SEQUENCE: usize = 1;
const BUTTONS: usize = 2;
const LEFT_TRIGGER: usize = 6;
const RIGHT_TRIGGER: usize = 8;
const LEFT_STICK: usize = 10;
const RIGHT_STICK: usize = 14;
/// The left trackpad's x, y and pressure, and the right's, in
/// [`STATE`] and [`STATE_PLAIN`].
const LEFT_TRACKPAD: usize = 18;
const RIGHT_TRACKPAD: usize = 24;
/// The sensor time, 32 bits.
const SENSOR_TIME: usize = 30;
/// The accelerometer's x, y and z, then the gyroscope's.
const ACCEL: usize = 34;
const GYRO: usize = 40;
/// The orientation's w, x, y and z, in [`STATE`] alone.
const ORIENTATION: usize = 46;
// In [`STATE_TIMESTAMPED`], from `TritonMTUNoQuat32TS_t`: the trackpad
// time where the left trackpad was, each trackpad two bytes on, and the
// sensor time narrowed to the two bytes before the accelerometer.
const TIMESTAMPED_TRACKPAD_TIME: usize = 18;
const TIMESTAMPED_LEFT_TRACKPAD: usize = 20;
const TIMESTAMPED_RIGHT_TRACKPAD: usize = 26;
const TIMESTAMPED_SENSOR_TIME: usize = 32;

impl Layout {
    const fn size(self) -> usize {
        match self {
            Self::Orientation => STATE_SIZE,
            Self::Plain => STATE_PLAIN_SIZE,
            Self::Timestamped => STATE_TIMESTAMPED_SIZE,
        }
    }

    /// Where the left and right trackpads start.
    const fn trackpads(self) -> (usize, usize) {
        match self {
            Self::Orientation | Self::Plain => (LEFT_TRACKPAD, RIGHT_TRACKPAD),
            Self::Timestamped => (TIMESTAMPED_LEFT_TRACKPAD, TIMESTAMPED_RIGHT_TRACKPAD),
        }
    }
}

impl Sample {
    fn parse(report: &[u8], layout: Layout) -> Option<Self> {
        if report.len() < layout.size() {
            return None;
        }
        let i16_at = |at| i16_at(report, at);
        let pair = |at: [usize; 2]| Some([i16_at(at[0])?, i16_at(at[1])?]);
        let buttons = Buttons(u32_at(report, BUTTONS)?);
        let trackpad = |at: usize, touch: Button| {
            let [x, y, pressure]: [[u8; 2]; 3] =
                bytes::<6>(report, at)?.as_chunks::<2>().0.try_into().ok()?;
            Some(Trackpad {
                touched: buttons.has(touch),
                x: across(i16::from_le_bytes(x)),
                y: down(i16::from_le_bytes(y)),
                pressure: pressure_of(u16::from_le_bytes(pressure)),
            })
        };
        let (left, right) = layout.trackpads();
        let triple = |at: [usize; 3]| Some([i16_at(at[0])?, i16_at(at[1])?, i16_at(at[2])?]);
        let (gyro, accel) = motion(
            triple([GYRO, GYRO + 2, GYRO + 4])?,
            triple([ACCEL, ACCEL + 2, ACCEL + 4])?,
        );
        let (time, trackpad_time, orientation) = match layout {
            Layout::Orientation => (
                SensorTime::Wide(u32_at(report, SENSOR_TIME)?),
                None,
                Some([
                    i16_at(ORIENTATION)?,
                    i16_at(ORIENTATION + 2)?,
                    i16_at(ORIENTATION + 4)?,
                    i16_at(ORIENTATION + 6)?,
                ]),
            ),
            Layout::Plain => (SensorTime::Wide(u32_at(report, SENSOR_TIME)?), None, None),
            Layout::Timestamped => (
                SensorTime::Narrow(u16_at(report, TIMESTAMPED_SENSOR_TIME)?),
                Some(u16_at(report, TIMESTAMPED_TRACKPAD_TIME)?),
                None,
            ),
        };
        Some(Self {
            sequence: *report.get(SEQUENCE)?,
            buttons,
            triggers: [
                trigger(i16_at(LEFT_TRIGGER)?),
                trigger(i16_at(RIGHT_TRIGGER)?),
            ],
            left_stick: stick(pair([LEFT_STICK, LEFT_STICK + 2])?),
            right_stick: stick(pair([RIGHT_STICK, RIGHT_STICK + 2])?),
            left_trackpad: trackpad(left, Button::LeftTrackpadTouch)?,
            right_trackpad: trackpad(right, Button::RightTrackpadTouch)?,
            gyro,
            accel,
            time,
            trackpad_time,
            orientation,
        })
    }
}

/// A count held to the kernel's symmetric range.
fn held(count: i16) -> f32 {
    f32::from(count.max(-FULL))
}

/// A stick's x and y counts as -1 to 1, right and down positive: the
/// report's y negated.
fn stick([x, y]: [i16; 2]) -> [f32; 2] {
    let full = f32::from(FULL);
    [held(x) / full, -held(y) / full]
}

/// A trigger's count as 0 to 1.
fn trigger(count: i16) -> f32 {
    f32::from(count.max(0)) / f32::from(FULL)
}

/// A trackpad's x count as 0 at the left edge to 1 at the right: SDL's
/// `x / 65536 + 0.5`, over the symmetric range so that both edges are
/// reached and the middle is exactly a half.
fn across(count: i16) -> f32 {
    let full = f32::from(FULL);
    (held(count) + full) / (2.0 * full)
}

/// A trackpad's y count as 0 at the top edge to 1 at the bottom: the
/// report's larger values higher, as SDL reads them.
fn down(count: i16) -> f32 {
    let full = f32::from(FULL);
    (full - held(count)) / (2.0 * full)
}

fn pressure_of(count: u16) -> f32 {
    (f32::from(count) / FULL_PRESSURE).min(1.0)
}

/// The gyroscope's and the accelerometer's counts, x, y and z as the
/// report has them, as the pad state has them: degrees a second and g,
/// on its axes.
///
/// The pad state's axes are SDL's for every pad, a DualSense's as it
/// reports them (`ctrlassist_core::Motion`): held in front of the
/// player, x to their right, y up, and z toward them. SDL hands a Steam
/// Controller's to a game as the report's x, z and minus y
/// (`SDL_hidapi_steam_triton.c:294-301`), and the kernel's motion device
/// has the same (`ABS_X` from x, `ABS_Y` from z, `ABS_Z` from minus y,
/// and so for the gyroscope, `hid-steam.c:2431-2439`). So, by the two
/// of them, the report's axes are x to the right, y away from the player
/// and z up, which only a real pad can confirm: a pad lying
/// flat and still reads its 1 g on the report's z, which is the pad
/// state's y. Taking (x, y, z) to (x, z, -y) is a quarter turn about x,
/// a rotation and not a mirror, so the gyroscope's rates turn with it
/// as the accelerometer's do.
///
/// No count is held to the symmetric range here, as the kernel holds it:
/// the kernel does so to negate in 16 bits, and these are negated as
/// floats, where -32768 is the full scale of 2000 degrees a second or
/// 2 g, as SDL reads it.
fn motion(gyro: [i16; 3], accel: [i16; 3]) -> ([f32; 3], [f32; 3]) {
    let turned = |[x, y, z]: [f32; 3]| [x, z, -y];
    (
        turned(gyro.map(|count| f32::from(count) * DEGREES_PER_COUNT)),
        turned(accel.map(|count| f32::from(count) / COUNTS_PER_G)),
    )
}

/// Whether the pad is charging, by the battery report's first byte
/// (`CHARGE_STATE_*`, `hid-steam.c:338-342`; SDL's `EChargeState`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Charge {
    /// 1.
    Discharging,
    /// 2.
    Charging,
    /// 4: charging done.
    Full,
    /// Any other: SDL's `EChargeState` names 0 reset and 3 a source being
    /// validated; the kernel reads every other as unknown
    /// (`hid-steam.c:2513-2526`).
    Other(u8),
}

/// What a battery report says (`struct steam_ibex_battery_status`,
/// `hid-steam.c:356-365`, read at 2499-2528; SDL's
/// `TritonBatteryStatus_t`). Each value is the report's, unchanged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Battery {
    /// Whether it is charging.
    pub(crate) charge: Charge,
    /// How full it is, in percent (the kernel's `CAPACITY`,
    /// `hid-steam.c:1025-1026`).
    pub(crate) percent: u8,
    /// The battery's voltage, in mV (`hid-steam.c:1016-1017`).
    pub(crate) battery_mv: u16,
    /// The pad's own supply voltage, read by neither source; in mV, by
    /// the battery's (an inference).
    pub(crate) system_mv: u16,
    /// The charging input's voltage, read by neither source; in mV, by
    /// the battery's (an inference).
    pub(crate) input_mv: u16,
    /// The battery's current, in mA (`hid-steam.c:1019-1020`).
    pub(crate) battery_ma: u16,
    /// The charging input's current, read by neither source; in mA, by
    /// the battery's (an inference).
    pub(crate) input_ma: u16,
    /// The temperature, in thousandths of a degree Celsius
    /// (`hid-steam.c:1022-1023`).
    pub(crate) temperature_millidegrees: u16,
}

impl Battery {
    fn parse(report: &[u8]) -> Option<Self> {
        if report.len() < BATTERY_SIZE {
            return None;
        }
        let u16_at = |at| u16_at(report, at);
        Some(Self {
            charge: match *report.get(1)? {
                1 => Charge::Discharging,
                2 => Charge::Charging,
                4 => Charge::Full,
                other => Charge::Other(other),
            },
            percent: *report.get(2)?,
            battery_mv: u16_at(3)?,
            system_mv: u16_at(5)?,
            input_mv: u16_at(7)?,
            battery_ma: u16_at(9)?,
            input_ma: u16_at(11)?,
            temperature_millidegrees: u16_at(13)?,
        })
    }
}

/// A wireless event: what became of the pad in a receiver's slot
/// (`WIRELESS_EVENT_*`, `hid-steam.c:345-349`; SDL's
/// `ETritonWirelessState`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Wireless {
    /// 1: the pad left the slot (`WIRELESS_EVENT_DISCONNECT`).
    Disconnected,
    /// 2: a pad joined it (`WIRELESS_EVENT_CONNECT`).
    Connected,
    /// 3: the slot is pairing, which the kernel names and neither source
    /// acts on.
    Pairing,
    /// Any other.
    Other(u8),
}

impl Wireless {
    fn parse(report: &[u8]) -> Option<Self> {
        if report.len() < WIRELESS_SIZE {
            return None;
        }
        Some(match *report.get(1)? {
            1 => Self::Disconnected,
            2 => Self::Connected,
            3 => Self::Pairing,
            other => Self::Other(other),
        })
    }
}

/// The quick access button's place: the controller database's `misc1`,
/// where SDL's mapping for this pad puts it (`misc1:b11`,
/// `SDL_gamepad.c:1263-1265`, b11 being its `TRITON_QAM`,
/// `SDL_hidapi_steam_triton.c:49`). No profile names it.
pub(crate) const QUICK_ACCESS: Control = Control::Button(15);
/// The left trigger's click and the right's: the controller database's
/// `misc3` and `misc4`, where SDL's mapping for this pad puts them
/// (`misc3:b18,misc4:b19`, `SDL_gamepad.c:1265`; its
/// `TRITON_LEFT_TRIGGER_CLICK` and `TRITON_RIGHT_TRIGGER_CLICK`,
/// `SDL_hidapi_steam_triton.c:56-57`). No profile names them and no
/// virtual pad shows them.
pub(crate) const LEFT_TRIGGER_CLICK: Control = Control::Button(22);
pub(crate) const RIGHT_TRIGGER_CLICK: Control = Control::Button(23);

/// Where a bit of the button word goes in the pad state, if anywhere.
///
/// The face buttons go by where they are, as every pad's do. The back
/// buttons are the paddles in SDL's order, R4, L4, R5 and L5. The right
/// trackpad is the first touchpad, as a DualSense's touchpad is, and its
/// click the first touchpad's click; the left is the second. SDL numbers
/// them the other way, its first touchpad the left and its `touchpad`
/// button the left's click (`SDL_hidapi_steam_triton.c:262-285`,
/// `SDL_gamepad.c:1265`). The right one is first here because the first
/// touchpad is what a virtual DualSense shows, and the right trackpad is
/// the one chosen to stand for a DualSense's touchpad: a profile that
/// maps the pad to a DualSense gives the game the right trackpad.
///
/// A trackpad's touch is no button: it is whether its contact is down. A
/// trigger's click is kept apart from its axis, which carries the pull:
/// the click is that pull pressed home, which the pad senses for itself,
/// and is kept where SDL keeps it rather than dropped, as the kernel's
/// gamepad layout drops a click beside an analog trigger, so that a
/// later identity that shows it has it to show.
const fn place(button: Button) -> Option<Control> {
    Some(match button {
        Button::South => gamepad::SOUTH,
        Button::East => gamepad::EAST,
        Button::West => gamepad::WEST,
        Button::North => gamepad::NORTH,
        Button::QuickAccess => QUICK_ACCESS,
        Button::RightStickClick => gamepad::RIGHT_THUMB,
        Button::Start => gamepad::START,
        Button::R4 => gamepad::PADDLE_1,
        Button::R5 => gamepad::PADDLE_3,
        Button::RightBumper => gamepad::RIGHT_BUMPER,
        Button::DpadDown => gamepad::DPAD_DOWN,
        Button::DpadRight => gamepad::DPAD_RIGHT,
        Button::DpadLeft => gamepad::DPAD_LEFT,
        Button::DpadUp => gamepad::DPAD_UP,
        Button::Select => gamepad::SELECT,
        Button::LeftStickClick => gamepad::LEFT_THUMB,
        Button::Steam => gamepad::MODE,
        Button::L4 => gamepad::PADDLE_2,
        Button::L5 => gamepad::PADDLE_4,
        Button::LeftBumper => gamepad::LEFT_BUMPER,
        Button::RightStickTouch => gamepad::RIGHT_STICK_TOUCH,
        Button::RightTrackpadClick => gamepad::TOUCHPAD,
        Button::LeftStickTouch => gamepad::LEFT_STICK_TOUCH,
        Button::LeftTrackpadClick => gamepad::SECOND_TOUCHPAD,
        Button::RightGripTouch => gamepad::RIGHT_GRIP_TOUCH,
        Button::LeftGripTouch => gamepad::LEFT_GRIP_TOUCH,
        Button::LeftTriggerClick => LEFT_TRIGGER_CLICK,
        Button::RightTriggerClick => RIGHT_TRIGGER_CLICK,
        Button::RightTrackpadTouch | Button::LeftTrackpadTouch => return None,
    })
}

/// A trackpad as one of the pad state's touchpads: one contact, down
/// where the finger is, or lifted where the finger last was. The report's
/// place while no finger is on the trackpad is not used: what a real pad
/// sends there no source says (see [`Trackpad::x`]), and a lifted
/// contact keeps the place its finger left (`Contact::new`). The second
/// contact is never down: the pad reports one finger a trackpad.
fn touchpad(was: Touchpad, now: Trackpad) -> Touchpad {
    let [last, _] = was.contacts();
    let contact = if now.touched {
        Contact::new(true, now.x, now.y)
    } else {
        Contact::new(false, last.x(), last.y())
    };
    Touchpad::new([contact, Contact::UP])
}

/// What a Steam Controller's state reports have said so far, as the pad
/// state: each report's sample taken in turn, whole.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Reading {
    state: PadState,
    /// When the sensors were last read, by the pad's clock: none before
    /// the first report.
    last: Option<SensorTime>,
    /// How many readings the pad has given, wrapping.
    readings: u32,
}

impl Reading {
    /// Nothing reported yet: the pad at rest.
    pub(crate) const NEW: Self = Self {
        state: PadState::NEUTRAL,
        last: None,
        readings: 0,
    };

    /// The pad's state as `sample` leaves it. Every control is the
    /// sample's, but for a trackpad's place while it is lifted.
    ///
    /// The motion is one reading more, spaced from the one before by the
    /// pad's own clock. The first has no interval, as every pad's first
    /// reading has none, and so has one whose clock is the other width
    /// from the reading before's, a pad that sent `0x45` and then `0x47`:
    /// neither clock says anything of the other, and the new one is
    /// counted from there.
    ///
    /// A report whose sensor clock hasn't moved since the reading before
    /// is no new reading: its other controls are taken, and the motion is
    /// kept as it was, its count and interval with it.
    pub(crate) fn take(&mut self, sample: &Sample) -> &PadState {
        let state = &mut self.state;
        for button in Button::ALL {
            if let Some(control) = place(button) {
                state.set(control, f32::from(u8::from(sample.buttons.has(button))));
            }
        }
        let [left_x, left_y] = sample.left_stick;
        let [right_x, right_y] = sample.right_stick;
        let [left_trigger, right_trigger] = sample.triggers;
        for (control, value) in [
            (gamepad::LEFT_X, left_x),
            (gamepad::LEFT_Y, left_y),
            (gamepad::RIGHT_X, right_x),
            (gamepad::RIGHT_Y, right_y),
            (gamepad::LEFT_TRIGGER, left_trigger),
            (gamepad::RIGHT_TRIGGER, right_trigger),
        ] {
            state.set(control, value);
        }
        state.set_touchpad(touchpad(state.touchpad(), sample.right_trackpad));
        state.set_second_touchpad(touchpad(state.second_touchpad(), sample.left_trackpad));
        // A report whose sensor clock hasn't moved since the last is no new
        // reading of the sensors, as SDL reads it
        // (`SDL_hidapi_steam_triton.c:289, 346`): its motion is passed over,
        // the last kept with its count. Counted, its interval of nothing
        // would be counted again on top of the next one's by a virtual pad
        // that advances its own clock by each.
        if self.last != Some(sample.time) {
            let interval_us = self
                .last
                .and_then(|last| sample.time.since(last))
                .unwrap_or(0);
            self.last = Some(sample.time);
            self.readings = self.readings.wrapping_add(1);
            state.set_motion(Motion::new(
                sample.gyro,
                sample.accel,
                self.readings,
                interval_us,
            ));
        }
        &self.state
    }
}

/// Room for one report: more than any this pad sends, so that one is
/// never cut short.
const BUFFER: usize = 128;
const _: () = assert!(BUFFER > STATE_SIZE && BUFFER > FEATURE_SIZE);

/// A Steam Controller's raw node, read for the pad's whole state.
///
/// It is a duplicate of the descriptor the pad's writer writes on, made
/// non-blocking: the two share what the kernel keeps of the open node,
/// its flag among it, which the raw node's writes and requests don't look
/// at (`hidraw_write` and `hidraw_ioctl` in `drivers/hid/hidraw.c`); only
/// its reads do.
pub(crate) struct Reader {
    raw: OwnedFd,
    /// Where each report is read to: the pad's own, so reading allocates
    /// nothing.
    buffer: [u8; BUFFER],
    reading: Reading,
    /// Whether the input thread has listened to the node yet.
    begun: bool,
}

impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reader")
            .field("readings", &self.reading.readings)
            .finish_non_exhaustive()
    }
}

impl Reader {
    /// Reads a pad's raw node as well as writing it: a duplicate of
    /// `raw`, made non-blocking, with the reports it kept while nobody
    /// read it read away, being from before now. The pad starts at rest,
    /// and is what its next state report says.
    ///
    /// # Errors
    ///
    /// Fails if the descriptor can't be duplicated or made non-blocking.
    pub(crate) fn new(raw: BorrowedFd<'_>) -> io::Result<Self> {
        let raw = raw.try_clone_to_owned()?;
        let flags = rustix::fs::fcntl_getfl(&raw)?;
        rustix::fs::fcntl_setfl(&raw, flags | OFlags::NONBLOCK)?;
        crate::motion::forget(raw.as_fd());
        Ok(Self::with(raw))
    }

    fn with(raw: OwnedFd) -> Self {
        Self {
            raw,
            buffer: [0; BUFFER],
            reading: Reading::NEW,
            begun: false,
        }
    }

    /// Reads away, once, the reports the node kept since this was made:
    /// called as the input thread first listens to the node, for the
    /// reports of the moments between the pad being given its node and
    /// its setup running, which a pad that reports every 4 ms fills, are
    /// from before then too. Again after that it does nothing, so that a
    /// pad listened to anew, as a running setup takes a new graph, loses
    /// nothing. Waits for nothing and allocates nothing.
    pub(crate) fn begin(&mut self) {
        if !std::mem::replace(&mut self.begun, true) {
            crate::motion::forget(self.raw.as_fd());
        }
    }

    /// Reads every report waiting, without waiting for more, and calls
    /// `step` with the pad's whole state after each state report. A
    /// battery report changes nothing in the pad state; a report this
    /// doesn't read, or one too short to, is passed over. Allocates
    /// nothing, and nothing a report says can make it panic.
    ///
    /// # Errors
    ///
    /// [`PadError::Gone`] when the pad has gone: the node unplugged from
    /// under it, which answers `ENODEV`, or `EIO` once nothing is left to
    /// read (`hidraw_read`); a receiver's slot saying its pad left it; or
    /// any other error of a read, or a read of nothing, the node being
    /// all there is of the pad, so one that can't be read is a pad that
    /// can't be.
    pub(crate) fn read(&mut self, mut step: impl FnMut(&PadState)) -> Result<(), PadError> {
        loop {
            match rustix::io::read(&self.raw, &mut self.buffer) {
                // No report is empty: a read of nothing is a node at its
                // end, which would be ready, and read as nothing, for ever.
                Ok(0) => return Err(PadError::Gone),
                Ok(length) => match parse(self.buffer.get(..length).unwrap_or_default()) {
                    Some(Input::State(sample)) => step(self.reading.take(&sample)),
                    Some(Input::Wireless(Wireless::Disconnected)) => return Err(PadError::Gone),
                    Some(Input::Battery(_) | Input::Wireless(_)) | None => {}
                },
                // A signal can't cut short a read that never waits; were
                // one to, the node stays ready and is read again.
                Err(Errno::AGAIN | Errno::INTR) => return Ok(()),
                Err(_) => return Err(PadError::Gone),
            }
        }
    }
}

impl AsFd for Reader {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.raw.as_fd()
    }
}

/// The left motor's gain, and the right's, as SDL sends them: both 0
/// (`SDL_hidapi_steam_triton.c:616, 618`). SDL's driver was written for
/// this pad, and its field is a signed gain, by Valve's header in
/// decibels (`MsgHapticRumble`'s `int8_t gain`, beside the `gain_db` of
/// the other haptic messages). The kernel sends 2 and 0
/// (`steam_haptic_rumble(steam, 0, left, right, 2, 0)`,
/// `hid-steam.c:839-840, 850-851`), from callbacks it shares with the
/// Steam Deck. What the difference does only a real pad can show.
const LEFT_GAIN: u8 = 0;
const RIGHT_GAIN: u8 = 0;

/// The rumble output report for `level`: its number, a type and an
/// intensity of 0, the left motor's speed and gain, and the right's,
/// little-endian (`struct steam_ibex_haptic_rumble`,
/// `hid-steam.c:367-375`, filled at 806-813; SDL's `MsgHapticRumble`).
///
/// The strong motor is the left and the weak the right, as the kernel
/// maps a game's rumble (`steam_play_effect`, `hid-steam.c:860-866`)
/// and as SDL maps its low and high frequencies
/// (`SDL_hidapi_steam_triton.c:615-617`).
pub(crate) fn rumble(level: Level) -> [u8; RUMBLE_SIZE] {
    let [strong_low, strong_high] = level.strong.to_le_bytes();
    let [weak_low, weak_high] = level.weak.to_le_bytes();
    [
        RUMBLE,
        0,
        0,
        0,
        strong_low,
        strong_high,
        LEFT_GAIN,
        weak_low,
        weak_high,
        RIGHT_GAIN,
    ]
}

/// A feature report to the pad carrying one command with no arguments:
/// its number, the command, and nothing after, as the kernel sends one
/// (`steam_send_report_byte`, `hid-steam.c:588-591`, through
/// `steam_send_report_id`, 517-571, whose buffer `hid_alloc_report_buf` zeroes).
fn command(id: u8) -> [u8; FEATURE_SIZE] {
    let mut frame = [0; FEATURE_SIZE];
    if let Some(head) = frame.first_chunk_mut::<2>() {
        *head = [FEATURE_PAD, id];
    }
    frame
}

/// A feature report to the pad writing settings: its number, the
/// command, the length of what follows, and each setting's number and
/// little-endian value (`steam_write_settings`, `hid-steam.c:593-613`;
/// SDL's `FeatureReportMsg` filled at
/// `SDL_hidapi_steam_triton.c:135-147`). No more than [`MOST_SETTINGS`]
/// fit, which is checked as this is built.
fn settings<const N: usize>(writes: [(u8, u16); N]) -> [u8; FEATURE_SIZE] {
    const {
        assert!(N <= MOST_SETTINGS, "a feature report holds 20 settings");
    }
    let mut frame = [0; FEATURE_SIZE];
    let (head, body) = frame.split_at_mut(3);
    let length = u8::try_from(N)
        .ok()
        .and_then(|count| count.checked_mul(3))
        .expect("20 settings are 60 bytes, because of the assert above");
    head.copy_from_slice(&[FEATURE_PAD, SET_SETTINGS_VALUES, length]);
    for (place, (setting, value)) in body.as_chunks_mut::<3>().0.iter_mut().zip(writes) {
        let [low, high] = value.to_le_bytes();
        place.copy_from_slice(&[setting, low, high]);
    }
    frame
}

/// Lizard mode off alone, as SDL sends it every three seconds while it
/// doesn't want the motion sensors (`MaintainTritonState`,
/// `SDL_hidapi_steam_triton.c:130-155`, called at 502-505). SDL leaves
/// the pad's watchdog on and so must say it again; this is not sent.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "not sent: the pad is taken over the kernel's way, by `take_over`"
    )
)]
pub(crate) fn lizard_mode_off() -> [u8; FEATURE_SIZE] {
    settings([(SETTING_LIZARD_MODE, LIZARD_MODE_OFF)])
}

/// What a pad is sent, once, as its raw node is taken: its own keys and
/// pointer cleared, then lizard mode off with the pad's watchdog off, as
/// the kernel's driver sends them (`steam_set_lizard_mode`,
/// `hid-steam.c:891-900`), and in the same settings write the motion
/// sensors asked for their raw readings, as the driver asks when its
/// motion device is opened (`steam_sensor_open`, `hid-steam.c:948-960`).
///
/// With the watchdog off the pad stays so until it is told otherwise, and
/// nothing is sent again: there is no interval to keep. SDL does
/// otherwise, leaving the watchdog on and sending lizard mode off every
/// three seconds (`SDL_hidapi_steam_triton.c:502-505`); each such request
/// waits on the pad holding a lock every raw node on the machine needs,
/// which one request as the pad is taken does once and a repeated one
/// would for as long as a pad were slow or silent. What the watchdog
/// would have mended needs no mending here: a daemon that crashes or
/// lets the pad go closes the raw node, and the kernel's driver then
/// sets the pad up again itself, lizard mode and its settings' defaults
/// with it (`steam_work_connect_cb`, `steam_register` and its
/// `steam_set_lizard_mode`, `hid-steam.c:1406`); one that hangs holds
/// the pad grabbed or hidden, so its keys and pointer would reach
/// nothing anyway. What isn't mended is another program's write to the
/// same pad: Steam turns the motion sensors off when it has no use for
/// them, and they stay off until the pad is taken again.
pub(crate) fn take_over() -> [[u8; FEATURE_SIZE]; 2] {
    [
        command(CLEAR_DIGITAL_MAPPINGS),
        settings([
            (SETTING_LIZARD_MODE, LIZARD_MODE_OFF),
            (SETTING_STEAM_WATCHDOG_ENABLE, WATCHDOG_OFF),
            (SETTING_IMU_MODE, IMU_MODE_RAW),
        ]),
    ]
}

/// The motion sensors on, sending their raw readings, or off, as the
/// kernel writes it when its motion device is opened and closed
/// (`steam_sensor_open` and `steam_sensor_close`, `hid-steam.c:948-975`)
/// and SDL when a game asks (`SDL_hidapi_steam_triton.c:676-700`).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the motion sensors are asked for as the pad is taken, and never turned off"
    )
)]
pub(crate) fn motion_sensors(on: bool) -> [u8; FEATURE_SIZE] {
    settings([(
        SETTING_IMU_MODE,
        if on { IMU_MODE_RAW } else { IMU_MODE_OFF },
    )])
}

/// Lizard mode back on, as the kernel puts it back: the pad's keys and
/// pointer to their defaults, then its settings to theirs, in that order
/// (`ID_SET_DEFAULT_DIGITAL_MAPPINGS` then `ID_LOAD_DEFAULT_SETTINGS`,
/// `steam_set_lizard_mode`, `hid-steam.c:885-889`).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "not sent: the kernel's driver sends it itself as the raw node closes"
    )
)]
pub(crate) fn lizard_mode_on() -> [[u8; FEATURE_SIZE]; 2] {
    [
        command(SET_DEFAULT_DIGITAL_MAPPINGS),
        command(LOAD_DEFAULT_SETTINGS),
    ]
}

/// The state report a pad holding `state` sends, [`STATE`] at its full
/// size: what [`Reading::take`] reads back as `state`, each control in
/// the place [`place`] names and each trackpad's touch from whether its
/// contact is down. `time` is the sensor clock, in microseconds. The
/// trackpads' pressure and the pad's own reckoning of its orientation
/// are sent as nothing: the pad state carries neither.
///
/// A lifted trackpad's place is sent as where its finger left, as SDL
/// keeps it; the kernel's driver reads no place without the touch bit.
pub(crate) fn report(state: &PadState, sequence: u8, time: u32) -> [u8; STATE_SIZE] {
    let mut report = [0u8; STATE_SIZE];
    let mut put = |at: usize, bytes: &[u8]| {
        if let Some(place) = report.get_mut(at..at.saturating_add(bytes.len())) {
            place.copy_from_slice(bytes);
        }
    };
    let [right, _] = state.touchpad().contacts();
    let [left, _] = state.second_touchpad().contacts();
    let mut buttons = 0u32;
    for button in Button::ALL {
        let on = match button {
            Button::RightTrackpadTouch => right.down(),
            Button::LeftTrackpadTouch => left.down(),
            other => place(other).is_some_and(|control| state.get(control) >= 0.5),
        };
        if on {
            buttons |= button.mask();
        }
    }
    let full = f32::from(FULL);
    // A float to a count: `as` saturates and takes NaN to 0.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a float cast to an integer saturates"
    )]
    let count = |value: f32| value.round() as i16;
    put(0, &[STATE, sequence]);
    put(BUTTONS, &buttons.to_le_bytes());
    for (at, control) in [
        (LEFT_TRIGGER, gamepad::LEFT_TRIGGER),
        (RIGHT_TRIGGER, gamepad::RIGHT_TRIGGER),
    ] {
        put(
            at,
            &count(state.get(control).clamp(0.0, 1.0) * full).to_le_bytes(),
        );
    }
    for (at, x, y) in [
        (LEFT_STICK, gamepad::LEFT_X, gamepad::LEFT_Y),
        (RIGHT_STICK, gamepad::RIGHT_X, gamepad::RIGHT_Y),
    ] {
        put(
            at,
            &count(state.get(x).clamp(-1.0, 1.0) * full).to_le_bytes(),
        );
        put(
            at.saturating_add(2),
            &count(-state.get(y).clamp(-1.0, 1.0) * full).to_le_bytes(),
        );
    }
    for (at, contact) in [(LEFT_TRACKPAD, left), (RIGHT_TRACKPAD, right)] {
        let x = contact.x().clamp(0.0, 1.0);
        let y = contact.y().clamp(0.0, 1.0);
        put(at, &count(x * 2.0 * full - full).to_le_bytes());
        put(
            at.saturating_add(2),
            &count(full - y * 2.0 * full).to_le_bytes(),
        );
    }
    put(SENSOR_TIME, &time.to_le_bytes());
    // The pad state's (x, y, z) are the report's (x, z, -y): see
    // [`motion`], which this undoes.
    let turned_back = |[x, y, z]: [f32; 3]| [x, -z, y];
    let motion = state.motion();
    for (at, counts) in [
        (
            ACCEL,
            turned_back(motion.accel()).map(|g| count(g * COUNTS_PER_G)),
        ),
        (
            GYRO,
            turned_back(motion.gyro()).map(|rate| count(rate / DEGREES_PER_COUNT)),
        ),
    ] {
        for (axis, value) in counts.into_iter().enumerate() {
            put(
                at.saturating_add(axis.saturating_mul(2)),
                &value.to_le_bytes(),
            );
        }
    }
    report
}

/// The level a rumble output report asks for, as [`rumble`] writes one:
/// none for another report, or one cut short. The gains are not read.
pub(crate) fn rumble_of(report: &[u8]) -> Option<Level> {
    let report = report.first_chunk::<RUMBLE_SIZE>()?;
    let [
        id,
        _,
        _,
        _,
        strong_low,
        strong_high,
        _,
        weak_low,
        weak_high,
        _,
    ] = *report;
    (id == RUMBLE).then(|| Level {
        strong: u16::from_le_bytes([strong_low, strong_high]),
        weak: u16::from_le_bytes([weak_low, weak_high]),
    })
}

/// The first and last of the pad's haptic output reports past rumble:
/// a pulse (`0x81`), a command, a tone, a sweep and a built-in effect
/// (`0x82` to `0x85`; `REPORT_ID_HAPTIC_*`, `hid-steam.c:329-334`, and
/// SDL's `ValveTritonOutReportMessageIDs` with a struct for each), then
/// the reports a real pad declares after them, which carry sound for its
/// actuators and which neither source names (`0x86` to `0x89`). Each
/// names the side it is for itself.
pub(crate) const HAPTIC_FIRST: u8 = 0x81;
pub(crate) const HAPTIC_LAST: u8 = 0x89;
/// The sizes those reports are declared at, each with its number, from
/// `0x81` on: SDL's (`HID_HAPTIC_*_BYTES`) for the first five, and a real
/// pad's descriptor, as published, for the rest.
pub(crate) const HAPTIC_SIZES: [usize; 9] = [8, 4, 10, 9, 4, 4, 64, 64, 64];
/// The most a haptic report is.
pub(crate) const HAPTIC_MOST: usize = 64;

/// One haptic output report, whole, as a reader wrote it to a virtual
/// Steam Controller: passed to a real one behind it as it is, its side
/// and all. What it asks is the reader's and the pad's business; nothing
/// here reads past its number.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Haptic {
    bytes: [u8; HAPTIC_MOST],
    length: u8,
}

impl Haptic {
    /// The haptic report `report` is, its number first: none for a report
    /// of another number, an empty one, or one longer than any is.
    #[must_use]
    pub fn of(report: &[u8]) -> Option<Self> {
        let id = *report.first()?;
        let length = u8::try_from(report.len()).ok()?;
        if !(HAPTIC_FIRST..=HAPTIC_LAST).contains(&id) || report.len() > HAPTIC_MOST {
            return None;
        }
        let mut bytes = [0; HAPTIC_MOST];
        for (place, byte) in bytes.iter_mut().zip(report) {
            *place = *byte;
        }
        Some(Self { bytes, length })
    }

    /// The report, its number first.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes
            .get(..usize::from(self.length))
            .unwrap_or(&self.bytes)
    }
}

impl std::fmt::Debug for Haptic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Haptic").field(&self.as_bytes()).finish()
    }
}

/// A virtual Steam Controller.
pub(crate) mod emulator;

/// The test crate's fake Steam Controller held against this reading.
#[cfg(test)]
mod fake;

#[cfg(test)]
#[expect(
    clippy::arithmetic_side_effects,
    reason = "tests lay reports out at places they know are in them"
)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// A state report's every field, as counts, to be laid out by hand.
    #[derive(Clone, Copy)]
    struct Raw {
        sequence: u8,
        buttons: u32,
        triggers: [i16; 2],
        sticks: [i16; 4],
        /// Each trackpad's x, y and pressure, as the report's bits.
        left: [u16; 3],
        right: [u16; 3],
        sensor_time: u32,
        narrow_time: u16,
        trackpad_time: u16,
        accel: [i16; 3],
        gyro: [i16; 3],
        orientation: [i16; 4],
    }

    /// Every field at a value of its own, so that a field read from
    /// another's place reads wrong.
    const RAW: Raw = Raw {
        sequence: 0xa7,
        buttons: 0x2a5a_c3e1,
        triggers: [32767, 8192],
        sticks: [8192, 16384, -24576, -32768],
        left: [(-32767i16) as u16, 32767, 16384],
        right: [0, (-16384i16) as u16, 40000],
        sensor_time: 0xdead_beef,
        narrow_time: 0xabcd,
        trackpad_time: 0x1234,
        accel: [4096, -8192, 16384],
        gyro: [1024, -2048, 4096],
        orientation: [11, -22, 33, -44],
    };

    fn put(report: &mut [u8], at: usize, bytes: &[u8]) {
        report[at..at + bytes.len()].copy_from_slice(bytes);
    }

    /// `raw` laid out as a report of `id`, by the kernel's table and
    /// SDL's structs, each place written out here rather than taken from
    /// the code under test.
    fn report(id: u8, raw: Raw) -> Vec<u8> {
        let size = match id {
            0x42 => 54,
            _ => 46,
        };
        let mut report = vec![0u8; size];
        report[0] = id;
        report[1] = raw.sequence;
        put(&mut report, 2, &raw.buttons.to_le_bytes());
        put(&mut report, 6, &raw.triggers[0].to_le_bytes());
        put(&mut report, 8, &raw.triggers[1].to_le_bytes());
        for (axis, count) in raw.sticks.iter().enumerate() {
            put(&mut report, 10 + 2 * axis, &count.to_le_bytes());
        }
        let (left, right) = if id == 0x47 { (20, 26) } else { (18, 24) };
        for (value, count) in raw.left.iter().enumerate() {
            put(&mut report, left + 2 * value, &count.to_le_bytes());
        }
        for (value, count) in raw.right.iter().enumerate() {
            put(&mut report, right + 2 * value, &count.to_le_bytes());
        }
        if id == 0x47 {
            put(&mut report, 18, &raw.trackpad_time.to_le_bytes());
            put(&mut report, 32, &raw.narrow_time.to_le_bytes());
        } else {
            put(&mut report, 30, &raw.sensor_time.to_le_bytes());
        }
        for (axis, count) in raw.accel.iter().enumerate() {
            put(&mut report, 34 + 2 * axis, &count.to_le_bytes());
        }
        for (axis, count) in raw.gyro.iter().enumerate() {
            put(&mut report, 40 + 2 * axis, &count.to_le_bytes());
        }
        if id == 0x42 {
            for (part, count) in raw.orientation.iter().enumerate() {
                put(&mut report, 46 + 2 * part, &count.to_le_bytes());
            }
        }
        report
    }

    fn state(report: &[u8]) -> Sample {
        match parse(report) {
            Some(Input::State(sample)) => sample,
            other => panic!("not a state report: {other:?}"),
        }
    }

    #[track_caller]
    fn close(read: f32, wanted: f32) {
        assert!((read - wanted).abs() < 1e-6, "{read} is not {wanted}");
    }

    #[track_caller]
    fn all_close(read: &[f32], wanted: &[f32]) {
        assert_eq!(read.len(), wanted.len());
        for (read, wanted) in read.iter().zip(wanted) {
            close(*read, *wanted);
        }
    }

    #[test]
    fn a_full_state_report_is_read_field_by_field() {
        let sample = state(&report(0x42, RAW));
        assert_eq!(sample.sequence, 0xa7);
        assert_eq!(sample.buttons.bits(), 0x2a5a_c3e1);
        all_close(&sample.triggers, &[1.0, 8192.0 / 32767.0]);
        // Right and down positive: the report's y is up.
        all_close(&sample.left_stick, &[8192.0 / 32767.0, -16384.0 / 32767.0]);
        // -32768 is read as -32767, so its y is exactly 1, down.
        all_close(&sample.right_stick, &[-24576.0 / 32767.0, 1.0]);
        // The left trackpad at its left edge and its top, half pressed.
        let left = sample.left_trackpad;
        all_close(&[left.x, left.y, left.pressure], &[0.0, 0.0, 0.5]);
        // The right in the middle across, a quarter of the way up from
        // the bottom, and pressed past full, which is full.
        let right = sample.right_trackpad;
        all_close(
            &[right.x, right.y, right.pressure],
            &[0.5, (32767.0 + 16384.0) / 65534.0, 1.0],
        );
        // 0x2a5a_c3e1 has the left trackpad's touch bit (25) and not the
        // right's (21).
        assert!(left.touched && !right.touched);
        // Accelerometer x, y, z of 0.25, -0.5 and 1 g are the pad state's
        // x, y, z of 0.25, 1 and 0.5; the gyroscope's 62.5, -125 and 250
        // degrees a second, 62.5, 250 and 125.
        all_close(&sample.accel, &[0.25, 1.0, 0.5]);
        all_close(&sample.gyro, &[62.5, 250.0, 125.0]);
        assert_eq!(sample.time, SensorTime::Wide(0xdead_beef));
        assert_eq!(sample.trackpad_time, None);
        assert_eq!(sample.orientation, Some([11, -22, 33, -44]));
    }

    #[test]
    fn a_plain_state_report_says_all_a_full_one_does_but_the_orientation() {
        let full = state(&report(0x42, RAW));
        let plain = state(&report(0x45, RAW));
        assert_eq!(plain.orientation, None);
        assert_eq!(
            plain,
            Sample {
                orientation: None,
                ..full
            }
        );
    }

    #[test]
    fn a_timestamped_state_report_has_its_trackpads_and_time_elsewhere() {
        let sample = state(&report(0x47, RAW));
        let full = state(&report(0x42, RAW));
        // The trackpads two bytes on, the same values.
        assert_eq!(sample.left_trackpad, full.left_trackpad);
        assert_eq!(sample.right_trackpad, full.right_trackpad);
        assert_eq!(sample.trackpad_time, Some(0x1234));
        assert_eq!(sample.time, SensorTime::Narrow(0xabcd));
        assert_eq!(
            sample,
            Sample {
                time: SensorTime::Narrow(0xabcd),
                trackpad_time: Some(0x1234),
                orientation: None,
                ..full
            }
        );
        // Read as the plain layout, the same bytes put the trackpad time
        // in the left trackpad's x: the layouts differ.
        let mut plain = report(0x47, RAW);
        plain[0] = 0x45;
        assert_ne!(state(&plain).left_trackpad, sample.left_trackpad);
    }

    #[test]
    fn every_button_bit_is_read_alone() {
        // Each named bit, by its place in the kernel's table (byte.bit,
        // byte 2's bit 0 being the word's bit 0) and SDL's value.
        let table: [(Button, usize, u8, u32); 30] = [
            (Button::South, 2, 0, 0x0000_0001),
            (Button::East, 2, 1, 0x0000_0002),
            (Button::West, 2, 2, 0x0000_0004),
            (Button::North, 2, 3, 0x0000_0008),
            (Button::QuickAccess, 2, 4, 0x0000_0010),
            (Button::RightStickClick, 2, 5, 0x0000_0020),
            (Button::Start, 2, 6, 0x0000_0040),
            (Button::R4, 2, 7, 0x0000_0080),
            (Button::R5, 3, 0, 0x0000_0100),
            (Button::RightBumper, 3, 1, 0x0000_0200),
            (Button::DpadDown, 3, 2, 0x0000_0400),
            (Button::DpadRight, 3, 3, 0x0000_0800),
            (Button::DpadLeft, 3, 4, 0x0000_1000),
            (Button::DpadUp, 3, 5, 0x0000_2000),
            (Button::Select, 3, 6, 0x0000_4000),
            (Button::LeftStickClick, 3, 7, 0x0000_8000),
            (Button::Steam, 4, 0, 0x0001_0000),
            (Button::L4, 4, 1, 0x0002_0000),
            (Button::L5, 4, 2, 0x0004_0000),
            (Button::LeftBumper, 4, 3, 0x0008_0000),
            (Button::RightStickTouch, 4, 4, 0x0010_0000),
            (Button::RightTrackpadTouch, 4, 5, 0x0020_0000),
            (Button::RightTrackpadClick, 4, 6, 0x0040_0000),
            (Button::RightTriggerClick, 4, 7, 0x0080_0000),
            (Button::LeftStickTouch, 5, 0, 0x0100_0000),
            (Button::LeftTrackpadTouch, 5, 1, 0x0200_0000),
            (Button::LeftTrackpadClick, 5, 2, 0x0400_0000),
            (Button::LeftTriggerClick, 5, 3, 0x0800_0000),
            (Button::RightGripTouch, 5, 4, 0x1000_0000),
            (Button::LeftGripTouch, 5, 5, 0x2000_0000),
        ];
        assert_eq!(table.map(|(button, ..)| button), Button::ALL);
        let rest = Raw { buttons: 0, ..RAW };
        for (button, byte, bit, value) in table {
            let mut report = report(0x45, rest);
            report[byte] = 1 << bit;
            let sample = state(&report);
            assert_eq!(sample.buttons.bits(), value, "{button:?}");
            for other in Button::ALL {
                assert_eq!(
                    sample.buttons.has(other),
                    other == button,
                    "{button:?} {other:?}"
                );
            }
            assert_eq!(
                sample.left_trackpad.touched,
                button == Button::LeftTrackpadTouch
            );
            assert_eq!(
                sample.right_trackpad.touched,
                button == Button::RightTrackpadTouch
            );
        }
        // The two bits no source names are kept, and are none of these.
        let mut report = report(0x45, rest);
        report[5] = 0xc0;
        let sample = state(&report);
        assert_eq!(sample.buttons.bits(), 0xc000_0000);
        assert!(
            Button::ALL
                .iter()
                .all(|button| !sample.buttons.has(*button))
        );
    }

    /// A plain state report at rest but for these sticks, triggers and
    /// trackpad places.
    fn with(sticks: [i16; 4], triggers: [i16; 2], trackpad: [i16; 2]) -> Sample {
        let raw = Raw {
            sticks,
            triggers,
            left: [trackpad[0] as u16, trackpad[1] as u16, 0],
            ..RAW
        };
        state(&report(0x45, raw))
    }

    #[test]
    fn each_axis_reaches_its_ends_and_rests_in_its_middle() {
        let ends = with([32767, 32767, -32768, -32768], [32767, -1], [32767, -32768]);
        assert_eq!(ends.left_stick, [1.0, -1.0]);
        assert_eq!(ends.right_stick, [-1.0, 1.0]);
        assert_eq!(ends.triggers, [1.0, 0.0], "a trigger below 0 rests");
        assert_eq!((ends.left_trackpad.x, ends.left_trackpad.y), (1.0, 1.0));
        // -32767 reads as -32768 does: the kernel's symmetric range.
        let near = with([-32767, -32767, 32767, 32767], [0, -32768], [-32767, 32767]);
        assert_eq!(near.left_stick, [-1.0, 1.0]);
        assert_eq!(near.right_stick, [1.0, -1.0]);
        assert_eq!(near.triggers, [0.0, 0.0]);
        assert_eq!((near.left_trackpad.x, near.left_trackpad.y), (0.0, 0.0));
        let rest = with([0; 4], [0; 2], [0; 2]);
        assert_eq!(rest.left_stick, [0.0, 0.0]);
        assert_eq!((rest.left_trackpad.x, rest.left_trackpad.y), (0.5, 0.5));
        // Pressure from nothing to full, and past.
        for (count, pressure) in [(0u16, 0.0), (16384, 0.5), (32768, 1.0), (u16::MAX, 1.0)] {
            let raw = Raw {
                left: [0, 0, count],
                ..RAW
            };
            assert_eq!(state(&report(0x45, raw)).left_trackpad.pressure, pressure);
        }
    }

    #[test]
    fn a_lifted_trackpad_is_untouched_and_its_place_passed_through() {
        // No touch bits, both trackpads' fields at places of their own:
        // untouched, and whatever the fields say, for the pad state's
        // builder to keep the last touched place over.
        let raw = Raw {
            buttons: 0,
            left: [(-32767i16) as u16, 32767, 0],
            right: [32767, (-32767i16) as u16, 0],
            ..RAW
        };
        for id in [0x42, 0x45, 0x47] {
            let sample = state(&report(id, raw));
            let (left, right) = (sample.left_trackpad, sample.right_trackpad);
            assert!(!left.touched && !right.touched, "{id:#x}");
            assert_eq!((left.x, left.y), (0.0, 0.0), "{id:#x}");
            assert_eq!((right.x, right.y), (1.0, 1.0), "{id:#x}");
        }
        // Lifted as the kernel reports one, every field 0: the middle.
        let zero = Raw {
            buttons: 0,
            left: [0; 3],
            ..RAW
        };
        let left = state(&report(0x45, zero)).left_trackpad;
        assert_eq!(
            (left.touched, left.x, left.y, left.pressure),
            (false, 0.5, 0.5, 0.0)
        );
    }

    #[test]
    fn motion_reaches_its_full_scale_on_the_pad_state_s_axes() {
        let at = |accel: [i16; 3], gyro: [i16; 3]| {
            let sample = state(&report(0x45, Raw { accel, gyro, ..RAW }));
            (sample.accel, sample.gyro)
        };
        // Flat and still: 1 g on the report's z, the pad state's y.
        assert_eq!(at([0, 0, 16384], [0; 3]), ([0.0, 1.0, 0.0], [0.0; 3]));
        // The report's y away from the player: the pad state's z toward
        // them, so minus.
        assert_eq!(at([0, 16384, 0], [0; 3]).0, [0.0, 0.0, -1.0]);
        // Full scale either way is 2 g and 2000 degrees a second, -32768
        // read as it is.
        assert_eq!(
            at([32767, -32768, -32768], [-32768, 32767, -32768]),
            (
                [32767.0 / 16384.0, -2.0, 2.0],
                [-2000.0, -2000.0, -32767.0 * 2000.0 / 32768.0]
            )
        );
    }

    #[test]
    fn short_empty_and_other_reports_are_none_and_long_ones_are_read() {
        assert_eq!(parse(&[]), None);
        for (id, size) in [(0x42, 54), (0x45, 46), (0x47, 46)] {
            let report = report(id, RAW);
            assert_eq!(report.len(), size);
            for cut in 0..size {
                assert_eq!(parse(&report[..cut]), None, "{id:#x} cut to {cut}");
            }
            // Bytes past its size are passed over.
            let mut long = report.clone();
            long.extend_from_slice(&[0xff; 18]);
            assert_eq!(parse(&long), parse(&report));
        }
        // A full report's number on a plain report's bytes is short.
        let mut plain = report(0x45, RAW);
        plain[0] = 0x42;
        assert_eq!(parse(&plain), None);
        // Numbers this file doesn't read, the 2015 pad's and the output
        // and feature reports among them.
        for id in [0x00, 0x01, 0x02, 0x40, 0x41, 0x44, 0x48, 0x78, 0x80, 0xff] {
            let mut other = report(0x42, RAW);
            other[0] = id;
            assert_eq!(parse(&other), None, "{id:#x}");
        }
    }

    #[test]
    fn a_battery_report_is_read_field_by_field() {
        let report = [
            0x43, 0x02, 87, 0x10, 0x0f, 0x20, 0x0e, 0x30, 0x13, 0x40, 0x01, 0x50, 0x02, 0x60, 0x6d,
        ];
        assert_eq!(
            parse(&report),
            Some(Input::Battery(Battery {
                charge: Charge::Charging,
                percent: 87,
                battery_mv: 0x0f10,
                system_mv: 0x0e20,
                input_mv: 0x1330,
                battery_ma: 0x0140,
                input_ma: 0x0250,
                temperature_millidegrees: 0x6d60,
            }))
        );
        for (state, charge) in [
            (0, Charge::Other(0)),
            (1, Charge::Discharging),
            (2, Charge::Charging),
            (3, Charge::Other(3)),
            (4, Charge::Full),
            (0xff, Charge::Other(0xff)),
        ] {
            let mut report = report;
            report[1] = state;
            let Some(Input::Battery(battery)) = parse(&report) else {
                panic!("{state}");
            };
            assert_eq!(battery.charge, charge);
        }
        assert_eq!(parse(&report[..14]), None);
        assert!(parse(&[&report[..], &[0; 4]].concat()).is_some());
    }

    #[test]
    fn a_wireless_event_says_what_became_of_the_slot() {
        for id in [0x79, 0x46] {
            for (state, event) in [
                (1, Wireless::Disconnected),
                (2, Wireless::Connected),
                (3, Wireless::Pairing),
                (0, Wireless::Other(0)),
                (9, Wireless::Other(9)),
            ] {
                assert_eq!(parse(&[id, state]), Some(Input::Wireless(event)));
                assert_eq!(parse(&[id, state, 0, 0]), Some(Input::Wireless(event)));
            }
            assert_eq!(parse(&[id]), None);
        }
    }

    #[test]
    fn the_sensor_time_is_counted_across_its_wrap_at_either_width() {
        let wide = |us| SensorTime::Wide(us);
        assert_eq!(wide(5000).since(wide(1000)), Some(4000));
        assert_eq!(wide(1000).since(wide(u32::MAX - 2999)), Some(4000));
        assert_eq!(wide(7).since(wide(7)), Some(0));
        let narrow = |ticks| SensorTime::Narrow(ticks);
        // 125 ticks of 32 µs are 4 ms, before and across the wrap.
        assert_eq!(narrow(200).since(narrow(75)), Some(4000));
        assert_eq!(narrow(100).since(narrow(65511)), Some(4000));
        assert_eq!(narrow(0).since(narrow(u16::MAX)), Some(32));
        assert_eq!(narrow(u16::MAX).since(narrow(0)), Some(65535 * 32));
        // One clock says nothing of the other's.
        assert_eq!(wide(4000).since(narrow(0)), None);
        assert_eq!(narrow(125).since(wide(0)), None);
    }

    #[test]
    fn rumble_is_the_report_sdl_sends() {
        // SDL's `HIDAPI_DriverSteamTriton_RumbleJoystick` with the low
        // frequency strong and the high weak, laid out by
        // `MsgHapticRumble`: both gains 0, where the kernel's would have
        // a left gain of 2 at byte 6.
        assert_eq!(
            rumble(Level {
                strong: 0x1234,
                weak: 0xabcd
            }),
            [0x80, 0x00, 0x00, 0x00, 0x34, 0x12, 0x00, 0xcd, 0xab, 0x00]
        );
        assert_eq!(rumble(Level::default()), [0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    /// A 64-byte feature report beginning with `head`, the rest zero.
    fn frame(head: &[u8]) -> [u8; 64] {
        let mut frame = [0; 64];
        frame[..head.len()].copy_from_slice(head);
        frame
    }

    #[test]
    fn the_settings_frames_are_what_sdl_and_the_kernel_send() {
        // SDL's `MaintainTritonState`, without the sensors.
        assert_eq!(
            lizard_mode_off(),
            frame(&[0x01, 0x87, 0x03, 0x09, 0x00, 0x00])
        );
        // The kernel's `steam_set_lizard_mode(steam, false)`: the
        // mappings cleared, then lizard mode and the watchdog off, here
        // with its `steam_sensor_open` in the same write.
        assert_eq!(
            take_over(),
            [
                frame(&[0x01, 0x81]),
                frame(&[
                    0x01, 0x87, 0x09, 0x09, 0x00, 0x00, 0x47, 0x00, 0x00, 0x30, 0x18, 0x00
                ]),
            ]
        );
        // The kernel's `steam_sensor_open` and `steam_sensor_close`, and
        // SDL's `HIDAPI_DriverSteamTriton_SetSensorsEnabled`.
        assert_eq!(
            motion_sensors(true),
            frame(&[0x01, 0x87, 0x03, 0x30, 0x18, 0x00])
        );
        assert_eq!(
            motion_sensors(false),
            frame(&[0x01, 0x87, 0x03, 0x30, 0x00, 0x00])
        );
        // The kernel's `steam_set_lizard_mode(steam, true)`.
        assert_eq!(
            lizard_mode_on(),
            [frame(&[0x01, 0x85]), frame(&[0x01, 0x8e])]
        );
    }

    /// A state report written from a pad state is read back as that
    /// state: every control the report has a place for, both trackpads,
    /// and the motion to within a count.
    #[test]
    fn a_report_written_from_a_state_reads_back_as_it() {
        let mut state = PadState::NEUTRAL;
        for button in Button::ALL {
            if let Some(control) = place(button) {
                state.set(control, 1.0);
            }
        }
        state.set(gamepad::LEFT_X, 0.5);
        state.set(gamepad::LEFT_Y, -0.25);
        state.set(gamepad::RIGHT_X, -1.0);
        state.set(gamepad::RIGHT_Y, 1.0);
        state.set(gamepad::LEFT_TRIGGER, 0.75);
        state.set(gamepad::RIGHT_TRIGGER, 1.0);
        state.set_touchpad(Touchpad::new([Contact::new(true, 0.25, 0.75), Contact::UP]));
        state.set_second_touchpad(Touchpad::new([Contact::new(false, 0.5, 0.5), Contact::UP]));
        state.set_motion(Motion::new([10.0, -20.0, 30.0], [0.0, 1.0, -0.5], 1, 0));
        let report = super::report(&state, 7, 4000);
        assert_eq!(report.len(), STATE_SIZE);
        let Some(Input::State(sample)) = parse(&report) else {
            panic!("a state report: {report:?}");
        };
        assert_eq!(sample.sequence, 7);
        assert_eq!(sample.time, SensorTime::Wide(4000));
        assert_eq!(
            sample.buttons.bits() & Button::RightTrackpadTouch.mask(),
            Button::RightTrackpadTouch.mask()
        );
        assert_eq!(sample.buttons.bits() & Button::LeftTrackpadTouch.mask(), 0);
        let mut reading = Reading::NEW;
        let read = *reading.take(&sample);
        for button in Button::ALL {
            if let Some(control) = place(button) {
                assert!((read.get(control) - 1.0).abs() < f32::EPSILON, "{button:?}");
            }
        }
        let close = |a: f32, b: f32| (a - b).abs() < 0.001;
        for control in [
            gamepad::LEFT_X,
            gamepad::LEFT_Y,
            gamepad::RIGHT_X,
            gamepad::RIGHT_Y,
            gamepad::LEFT_TRIGGER,
            gamepad::RIGHT_TRIGGER,
        ] {
            assert!(close(read.get(control), state.get(control)), "{control:?}");
        }
        let [right, _] = read.touchpad().contacts();
        assert!(
            right.down() && close(right.x(), 0.25) && close(right.y(), 0.75),
            "{right:?}"
        );
        assert!(!read.second_touchpad().contacts()[0].down());
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.1);
        assert!(
            near(read.motion().gyro(), [10.0, -20.0, 30.0]),
            "{:?}",
            read.motion()
        );
        assert!(
            near(read.motion().accel(), [0.0, 1.0, -0.5]),
            "{:?}",
            read.motion()
        );
        assert_eq!(
            rumble_of(&rumble(Level {
                strong: 0x8000,
                weak: 0x4000
            })),
            Some(Level {
                strong: 0x8000,
                weak: 0x4000
            })
        );
        assert_eq!(rumble_of(&[0x81, 0, 0, 0, 0, 0, 0, 0, 0, 0]), None);
    }

    #[test]
    fn a_settings_frame_holds_twenty_settings() {
        let mut writes = [(0u8, 0u16); 20];
        for (setting, write) in (1u8..).zip(writes.iter_mut()) {
            *write = (setting, 0x0100 * u16::from(setting) + 0x42);
        }
        let frame = settings(writes);
        assert_eq!(frame[..3], [0x01, 0x87, 60]);
        for (index, (setting, value)) in writes.iter().enumerate() {
            let at = 3 + 3 * index;
            assert_eq!(frame[at], *setting);
            assert_eq!(u16::from_le_bytes([frame[at + 1], frame[at + 2]]), *value);
        }
        assert_eq!(frame[63], 0);
    }

    #[test]
    fn the_numbers_are_the_sources() {
        assert_eq!(VENDOR, 0x28de);
        assert_eq!([FEATURE_PAD, FEATURE_RECEIVER], [1, 2]);
        assert_eq!(
            [
                PRODUCT_CABLE,
                PRODUCT_BLUETOOTH,
                PRODUCT_PUCK,
                PRODUCT_RECEIVER
            ],
            [0x1302, 0x1303, 0x1304, 0x1305]
        );
        assert_eq!(RUMBLE_AGAIN, Duration::from_millis(40));
    }

    /// The pad state after `raw`, sent as `id`, taken by a reading that
    /// has had nothing before.
    fn first(id: u8, raw: Raw) -> PadState {
        let mut reading = Reading::NEW;
        *reading.take(&state(&report(id, raw)))
    }

    #[test]
    fn every_button_goes_to_its_place_and_nowhere_else() {
        // Written out here, not taken from the code under test.
        let table: [(Button, Option<Control>); 30] = [
            (Button::South, Some(gamepad::SOUTH)),
            (Button::East, Some(gamepad::EAST)),
            (Button::West, Some(gamepad::WEST)),
            (Button::North, Some(gamepad::NORTH)),
            (Button::QuickAccess, Some(Control::Button(15))),
            (Button::RightStickClick, Some(gamepad::RIGHT_THUMB)),
            (Button::Start, Some(gamepad::START)),
            (Button::R4, Some(gamepad::PADDLE_1)),
            (Button::R5, Some(gamepad::PADDLE_3)),
            (Button::RightBumper, Some(gamepad::RIGHT_BUMPER)),
            (Button::DpadDown, Some(gamepad::DPAD_DOWN)),
            (Button::DpadRight, Some(gamepad::DPAD_RIGHT)),
            (Button::DpadLeft, Some(gamepad::DPAD_LEFT)),
            (Button::DpadUp, Some(gamepad::DPAD_UP)),
            (Button::Select, Some(gamepad::SELECT)),
            (Button::LeftStickClick, Some(gamepad::LEFT_THUMB)),
            (Button::Steam, Some(gamepad::MODE)),
            (Button::L4, Some(gamepad::PADDLE_2)),
            (Button::L5, Some(gamepad::PADDLE_4)),
            (Button::LeftBumper, Some(gamepad::LEFT_BUMPER)),
            (Button::RightStickTouch, Some(gamepad::RIGHT_STICK_TOUCH)),
            (Button::RightTrackpadTouch, None),
            (Button::RightTrackpadClick, Some(gamepad::TOUCHPAD)),
            (Button::RightTriggerClick, Some(Control::Button(23))),
            (Button::LeftStickTouch, Some(gamepad::LEFT_STICK_TOUCH)),
            (Button::LeftTrackpadTouch, None),
            (Button::LeftTrackpadClick, Some(gamepad::SECOND_TOUCHPAD)),
            (Button::LeftTriggerClick, Some(Control::Button(22))),
            (Button::RightGripTouch, Some(gamepad::RIGHT_GRIP_TOUCH)),
            (Button::LeftGripTouch, Some(gamepad::LEFT_GRIP_TOUCH)),
        ];
        assert_eq!(table.map(|(button, _)| button), Button::ALL);
        // Every axis at rest, so that only the button can show.
        let rest = Raw {
            buttons: 0,
            triggers: [0; 2],
            sticks: [0; 4],
            ..RAW
        };
        let unpressed = first(0x45, rest);
        for (button, place) in table {
            let held = first(
                0x45,
                Raw {
                    buttons: button.mask(),
                    ..rest
                },
            );
            for index in 0..64 {
                let control = Control::Button(index);
                let wanted = if place == Some(control) { 1.0 } else { 0.0 };
                assert_eq!(held.get(control), wanted, "{button:?} at {control:?}");
            }
            // A trackpad's touch is its contact down, and nothing else.
            let right = button == Button::RightTrackpadTouch;
            let left = button == Button::LeftTrackpadTouch;
            assert_eq!(held.touchpad().contacts()[0].down(), right, "{button:?}");
            assert_eq!(
                held.second_touchpad().contacts()[0].down(),
                left,
                "{button:?}"
            );
            if !right && !left {
                assert_eq!(held.touchpad(), unpressed.touchpad(), "{button:?}");
                assert_eq!(held.second_touchpad(), unpressed.second_touchpad());
            }
        }
        // The two bits no source names go nowhere.
        let unnamed = first(
            0x45,
            Raw {
                buttons: 0xc000_0000,
                ..rest
            },
        );
        assert_eq!(unnamed, unpressed);
    }

    #[test]
    fn sticks_and_triggers_keep_the_signs_the_rest_of_the_crate_uses() {
        // The report's y grows upward: up on the left stick is its y's
        // negative, and full down on the right its positive.
        let state = first(
            0x42,
            Raw {
                sticks: [32767, 32767, -16384, -32768],
                triggers: [32767, 8192],
                ..RAW
            },
        );
        assert_eq!(state.get(gamepad::LEFT_X), 1.0);
        assert_eq!(state.get(gamepad::LEFT_Y), -1.0, "up is negative");
        assert_eq!(state.get(gamepad::RIGHT_X), -16384.0 / 32767.0);
        assert_eq!(state.get(gamepad::RIGHT_Y), 1.0, "down is positive");
        assert_eq!(state.get(gamepad::LEFT_TRIGGER), 1.0);
        assert_eq!(state.get(gamepad::RIGHT_TRIGGER), 8192.0 / 32767.0);
    }

    /// One trackpad touched at a place, or lifted with whatever the report
    /// puts in its fields.
    fn on_trackpads(left: Option<[i16; 2]>, right: Option<[i16; 2]>, lifted: [i16; 2]) -> Raw {
        let mut buttons = 0;
        let pad = |at: Option<[i16; 2]>, touch: Button, buttons: &mut u32| {
            let [x, y] = at.unwrap_or(lifted);
            if at.is_some() {
                *buttons |= touch.mask();
            }
            [x as u16, y as u16, 0]
        };
        Raw {
            left: pad(left, Button::LeftTrackpadTouch, &mut buttons),
            right: pad(right, Button::RightTrackpadTouch, &mut buttons),
            buttons,
            ..RAW
        }
    }

    #[test]
    fn a_lifted_trackpad_keeps_the_place_its_finger_left() {
        let mut reading = Reading::NEW;
        let mut take = |raw: Raw| *reading.take(&state(&report(0x45, raw)));
        // Never touched: no finger, and no place.
        let untouched = take(on_trackpads(None, None, [1234, -4321]));
        assert_eq!(untouched.touchpad(), Touchpad::UNTOUCHED);
        assert_eq!(untouched.second_touchpad(), Touchpad::UNTOUCHED);

        // The right trackpad is the first touchpad, the left the second;
        // each one contact.
        let touched = take(on_trackpads(
            Some([-32767, 32767]),
            Some([32767, 0]),
            [0; 2],
        ));
        let [right, other] = touched.touchpad().contacts();
        assert!(right.down() && !other.down());
        assert_eq!((right.x(), right.y()), (1.0, 0.5));
        let [left, other] = touched.second_touchpad().contacts();
        assert!(left.down() && !other.down());
        assert_eq!((left.x(), left.y()), (0.0, 0.0), "the top left corner");

        // Lifted, whatever the fields say then: where the finger was.
        for lifted in [[0, 0], [-32767, -32767], [5000, 6000]] {
            let state = take(on_trackpads(None, None, lifted));
            let right = state.touchpad().contacts()[0];
            let left = state.second_touchpad().contacts()[0];
            assert!(!right.down() && !left.down());
            assert_eq!((right.x(), right.y()), (1.0, 0.5), "{lifted:?}");
            assert_eq!((left.x(), left.y()), (0.0, 0.0), "{lifted:?}");
        }
        // Touched again, the finger is where it is now.
        let again = take(on_trackpads(None, Some([0, 0]), [0; 2]));
        let right = again.touchpad().contacts()[0];
        assert!(right.down());
        assert_eq!((right.x(), right.y()), (0.5, 0.5));
    }

    #[test]
    fn readings_are_counted_and_spaced_by_the_pad_s_clock_from_the_first() {
        let mut reading = Reading::NEW;
        let mut take = |id: u8, sensor_time: u32, narrow_time: u16| {
            let raw = Raw {
                sensor_time,
                narrow_time,
                ..RAW
            };
            let motion = reading.take(&state(&report(id, raw))).motion();
            (motion.readings(), motion.interval_us())
        };
        // The first has no interval; then the pad's own, across its wrap.
        assert_eq!(take(0x42, u32::MAX - 999, 0), (1, 0));
        assert_eq!(take(0x45, 3000, 0), (2, 4000));
        assert_eq!(take(0x42, 7032, 0), (3, 4032));
        // The other clock says nothing of this one: no interval, and it
        // is counted from there.
        assert_eq!(take(0x47, 0, 65_500), (4, 0));
        assert_eq!(take(0x47, 0, 89), (5, 125 * 32));
        assert_eq!(take(0x45, 50, 0), (6, 0));
        assert_eq!(take(0x45, 4050, 0), (7, 4000));
    }

    /// A report whose sensor clock hasn't moved is no new reading: the
    /// motion stays as the reading before left it, count, interval and
    /// values, while every other control is the report's.
    #[test]
    fn a_report_whose_clock_has_not_moved_is_no_new_reading() {
        let mut reading = Reading::NEW;
        let mut take = |sensor_time: u32, gyro: [i16; 3], buttons: u32| {
            let raw = Raw {
                sensor_time,
                gyro,
                buttons,
                ..RAW
            };
            *reading.take(&state(&report(0x45, raw)))
        };
        take(1000, [0; 3], 0);
        let read = take(5000, [1024, 0, 0], 0);
        assert_eq!(
            (read.motion().readings(), read.motion().interval_us()),
            (2, 4000)
        );
        let again = take(5000, [-4096, 0, 0], Button::South.mask());
        assert_eq!(again.motion(), read.motion(), "the reading before's");
        assert_eq!(again.get(gamepad::SOUTH), 1.0, "the report's button");
        // The next reading's interval is its own, from the last that was.
        let next = take(9000, [0; 3], 0);
        assert_eq!(
            (next.motion().readings(), next.motion().interval_us()),
            (3, 4000)
        );
        // The motion is the sample's.
        let state = first(0x45, RAW);
        assert_eq!(state.motion().gyro(), state_of(0x45).gyro);
        assert_eq!(state.motion().accel(), state_of(0x45).accel);
    }

    fn state_of(id: u8) -> Sample {
        state(&report(id, RAW))
    }

    /// The pad's reports, through a stand-in for its raw node, each
    /// datagram a report: a state report is a step, a battery report and
    /// anything not read are nothing, and a receiver's slot saying its pad
    /// left is the pad gone, with what came after it unread.
    #[test]
    fn a_raw_node_s_reports_are_steps_and_a_slot_left_is_the_pad_gone() {
        use std::os::unix::net::UnixDatagram;
        let (pad, node) = UnixDatagram::pair().unwrap();
        node.set_nonblocking(true).unwrap();
        let mut reader = Reader::with(node.into());
        let south = Raw {
            buttons: Button::South.mask(),
            ..RAW
        };
        let battery = [
            0x43, 0x02, 87, 0x10, 0x0f, 0x20, 0x0e, 0x30, 0x13, 0x40, 0x01, 0x50, 0x02, 0x60, 0x6d,
        ];
        pad.send(&report(0x45, south)).unwrap();
        pad.send(&battery).unwrap();
        pad.send(&[0x44; 46]).unwrap();
        pad.send(&[0x79, 2]).unwrap();
        let later = Raw {
            buttons: 0,
            sensor_time: RAW.sensor_time.wrapping_add(4000),
            ..RAW
        };
        pad.send(&report(0x42, later)).unwrap();
        let mut steps = Vec::new();
        reader.read(|state| steps.push(*state)).unwrap();
        assert_eq!(steps.len(), 2, "{steps:?}");
        assert_eq!(steps[0].get(gamepad::SOUTH), 1.0);
        assert_eq!(steps[1].get(gamepad::SOUTH), 0.0);
        assert_eq!(steps[1].motion().readings(), 2);

        pad.send(&[0x79, 1]).unwrap();
        pad.send(&report(0x45, south)).unwrap();
        let mut after = Vec::new();
        let gone = reader.read(|state| after.push(*state)).unwrap_err();
        assert!(matches!(gone, PadError::Gone), "{gone:?}");
        assert!(after.is_empty());
        // What came after is still there, for whoever reads next.
        let mut last = Vec::new();
        reader.read(|state| last.push(*state)).unwrap();
        assert_eq!(last.len(), 1);
    }

    #[test]
    fn only_a_steam_controller_s_numbers_on_its_buses_are_one() {
        for (bus, product, is) in [
            (3, 0x1302, true),
            (5, 0x1303, true),
            (3, 0x1304, true),
            (3, 0x1305, true),
            (5, 0x1302, false),
            (3, 0x1303, false),
            (3, 0x1102, false),
            (3, 0x1205, false),
        ] {
            assert_eq!(is_pad(bus, 0x28de, product), is, "{bus} {product:#x}");
        }
        assert!(!is_pad(3, 0x054c, 0x1302));
    }

    proptest! {
        /// No bytes can make a report's reading panic, and whatever is
        /// read is in range.
        #[test]
        fn no_report_can_panic_the_reading_or_leave_a_range(
            id in prop::sample::select(vec![0x42u8, 0x43, 0x45, 0x46, 0x47, 0x79, 0x00, 0xff]),
            body in prop::collection::vec(any::<u8>(), 0..80),
        ) {
            let report = [&[id][..], &body].concat();
            if let Some(Input::State(sample)) = parse(&report) {
                let unit = |value: f32| (0.0..=1.0).contains(&value);
                let signed = |value: f32| (-1.0..=1.0).contains(&value);
                prop_assert!(sample.triggers.iter().all(|value| unit(*value)));
                prop_assert!(sample.left_stick.iter().chain(&sample.right_stick).all(|value| signed(*value)));
                for trackpad in [sample.left_trackpad, sample.right_trackpad] {
                    prop_assert!(unit(trackpad.x) && unit(trackpad.y) && unit(trackpad.pressure));
                }
                prop_assert!(sample.gyro.iter().all(|value| value.abs() <= 2000.0));
                prop_assert!(sample.accel.iter().all(|value| value.abs() <= 2.0));
            }
        }

        /// Nor can any bytes at all.
        #[test]
        fn no_bytes_can_panic_the_reading(bytes in prop::collection::vec(any::<u8>(), 0..80)) {
            let _ = parse(&bytes);
        }
    }
}
