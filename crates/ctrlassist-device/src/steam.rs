//! A Steam Controller's reports, read and written as plain code that
//! opens nothing: bytes in, values out, and values in, bytes out.
//!
//! The pad is Valve's 2026 Steam Controller, which SDL calls Triton and
//! the kernel Ibex. It is read from its raw HID node, as SDL reads it,
//! and not from the input devices the kernel's driver makes of it, which
//! leave out much of what it reports. Its reports are the same on every
//! transport: a cable, the Puck, and Bluetooth.
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
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "read and written by the setup that holds a Steam Controller, which is built on this"
    )
)]

use std::time::Duration;

use ctrlassist_core::Level;

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
/// How often lizard mode is turned off again: the pad turns it back on
/// by a watchdog of its own (SDL's comment at
/// `SDL_hidapi_steam_triton.c:704`), and SDL turns it off every 3 s
/// (`SDL_hidapi_steam_triton.c:502-505`), sending [`lizard_mode_off`] or
/// [`lizard_mode_off_motion_on`]. This is the pad's protocol, not a
/// choice of CtrlAssist's; how long the watchdog waits is published
/// nowhere.
///
/// The kernel does otherwise: it turns the watchdog off as it turns
/// lizard mode off, once, and sends nothing again
/// (`steam_set_lizard_mode`, `hid-steam.c:894-900`). That is not done
/// here, and no frame for it is offered: with the watchdog off, a daemon
/// that crashed would leave the pad out of lizard mode, neither typing
/// nor pointing for the player, until it is turned off. With the
/// watchdog on, the pad falls back to lizard mode by itself once nothing
/// sends this.
pub(crate) const LIZARD_MODE_AGAIN: Duration = Duration::from_secs(3);

/// The commands a feature report carries, by their first byte
/// (`hid-steam.c:91-144`; `controller_constants.h`).
const CLEAR_DIGITAL_MAPPINGS: u8 = 0x81;
const SET_DEFAULT_DIGITAL_MAPPINGS: u8 = 0x85;
const SET_SETTINGS_VALUES: u8 = 0x87;
const LOAD_DEFAULT_SETTINGS: u8 = 0x8e;

/// The settings this file writes, by number: the kernel's and SDL's
/// enums agree, each counting from 0 (`hid-steam.c:147-247`, these at
/// 158 and 205; `controller_constants.h`). The watchdog's, 71, is left
/// out on purpose: see [`LIZARD_MODE_AGAIN`].
const SETTING_LIZARD_MODE: u8 = 9;
const SETTING_IMU_MODE: u8 = 48;
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

/// Lizard mode off, as SDL keeps it off every [`LIZARD_MODE_AGAIN`] while
/// it doesn't want the motion sensors (`MaintainTritonState`,
/// `SDL_hidapi_steam_triton.c:130-155`).
pub(crate) fn lizard_mode_off() -> [u8; FEATURE_SIZE] {
    settings([(SETTING_LIZARD_MODE, LIZARD_MODE_OFF)])
}

/// Lizard mode off and the motion sensors on, in one write, as SDL keeps
/// them every [`LIZARD_MODE_AGAIN`] while it wants the sensors
/// (`MaintainTritonState`, `SDL_hidapi_steam_triton.c:143-147`): Steam
/// turns the sensors off when it has no use for them.
pub(crate) fn lizard_mode_off_motion_on() -> [u8; FEATURE_SIZE] {
    settings([
        (SETTING_LIZARD_MODE, LIZARD_MODE_OFF),
        (SETTING_IMU_MODE, IMU_MODE_RAW),
    ])
}

/// The motion sensors on, sending their raw readings, or off, as the
/// kernel writes it when its motion device is opened and closed
/// (`steam_sensor_open` and `steam_sensor_close`, `hid-steam.c:948-975`)
/// and SDL when a game asks (`SDL_hidapi_steam_triton.c:676-700`).
pub(crate) fn motion_sensors(on: bool) -> [u8; FEATURE_SIZE] {
    settings([(
        SETTING_IMU_MODE,
        if on { IMU_MODE_RAW } else { IMU_MODE_OFF },
    )])
}

/// The pad's own keys and pointer turned off: the kernel sends it before
/// it writes lizard mode off (`ID_CLEAR_DIGITAL_MAPPINGS`,
/// `steam_set_lizard_mode`, `hid-steam.c:891-892`), and SDL never sends
/// it, relying on lizard mode off alone. Whether it is sent is for
/// whatever opens the pad to choose; what it adds over lizard mode off
/// only a real pad can show.
pub(crate) fn clear_digital_mappings() -> [u8; FEATURE_SIZE] {
    command(CLEAR_DIGITAL_MAPPINGS)
}

/// Lizard mode back on, as the kernel puts it back: the pad's keys and
/// pointer to their defaults, then its settings to theirs, in that order
/// (`ID_SET_DEFAULT_DIGITAL_MAPPINGS` then `ID_LOAD_DEFAULT_SETTINGS`,
/// `steam_set_lizard_mode`, `hid-steam.c:885-889`).
pub(crate) fn lizard_mode_on() -> [[u8; FEATURE_SIZE]; 2] {
    [
        command(SET_DEFAULT_DIGITAL_MAPPINGS),
        command(LOAD_DEFAULT_SETTINGS),
    ]
}

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
        // SDL's `MaintainTritonState`, without the sensors and with.
        assert_eq!(
            lizard_mode_off(),
            frame(&[0x01, 0x87, 0x03, 0x09, 0x00, 0x00])
        );
        assert_eq!(
            lizard_mode_off_motion_on(),
            frame(&[0x01, 0x87, 0x06, 0x09, 0x00, 0x00, 0x30, 0x18, 0x00])
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
        // What the kernel's `steam_set_lizard_mode(steam, false)` sends
        // first: the mappings cleared.
        assert_eq!(clear_digital_mappings(), frame(&[0x01, 0x81]));
        // And `steam_set_lizard_mode(steam, true)`.
        assert_eq!(
            lizard_mode_on(),
            [frame(&[0x01, 0x85]), frame(&[0x01, 0x8e])]
        );
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
        assert_eq!(LIZARD_MODE_AGAIN, Duration::from_secs(3));
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
