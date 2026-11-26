//! A fake Steam Controller, the 2026 one, made through uhid for the
//! kernel's own `hid-steam` to bind, as a real one is.
//!
//! Its bytes are this crate's own reading of the Steam Controller's reports,
//! from the kernel's driver and SDL's: a second reading, apart from the
//! device crate's, so that each checks the other. Its report descriptor
//! is written here and declares Valve's vendor collection alone: no mouse
//! and no keyboard, so a fake made on a desktop can never type or move
//! the pointer, whatever the driver tells it.

use std::fs::{File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Valve's vendor number.
pub const VALVE: u16 = 0x28de;

// Event types from linux/uhid.h.
const UHID_OUTPUT: u32 = 6;
const UHID_GET_REPORT: u32 = 9;
const UHID_GET_REPORT_REPLY: u32 = 10;
const UHID_CREATE2: u32 = 11;
const UHID_INPUT2: u32 = 12;
const UHID_SET_REPORT: u32 = 13;
const UHID_SET_REPORT_REPLY: u32 = 14;
/// The size of a uhid event's data, and of the largest event.
const UHID_DATA: usize = 4096;
const UHID_EVENT: usize = 4 + 4372;
/// The error a uhid request is answered with when there is nothing to
/// answer: `EIO`.
const UHID_REFUSED: u16 = 5;

// Bus types from linux/input.h.
const BUS_USB: u16 = 3;
const BUS_BLUETOOTH: u16 = 5;

/// The version the fake's HID device reports, as a cabled Steam Controller's.
const VERSION: u32 = 0x0307;

// The Steam Controller's report numbers.
const STATE: u8 = 0x42;
const STATE_WITHOUT_ORIENTATION: u8 = 0x45;
const STATE_WITH_SHORT_CLOCKS: u8 = 0x47;
const BATTERY: u8 = 0x43;
const WIRELESS: u8 = 0x79;
const RUMBLE: u8 = 0x80;
const PULSE: u8 = 0x81;
const CONTROLLER_COMMANDS: u8 = 1;
const RECEIVER_COMMANDS: u8 = 2;

// The commands the fake answers with more than an echo.
const GET_ATTRIBUTES: u8 = 0x83;
const GET_STRING_ATTRIBUTE: u8 = 0xae;
const GET_WIRELESS_STATE: u8 = 0xb4;
/// The length of the field a serial is answered in, as the kernel's
/// driver asks for it.
const SERIAL_FIELD: u8 = 0x15;
/// One tick of report `0x47`'s sensors' clock, in microseconds, as SDL
/// reads it (`SDL_hidapi_steam_triton.c:347-348`).
const NARROW_TICK_US: u32 = 32;

/// How the fake is connected, which decides its bus and product number.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum SteamTransport {
    /// A USB cable: bus USB, product `0x1302`.
    Cable,
    /// Bluetooth: bus Bluetooth, product `0x1303`.
    Bluetooth,
    /// One of a Puck's slots: bus USB, product `0x1304`. It answers the
    /// receiver's wireless state on feature report 2, with a pad
    /// in the slot as it is made if `joined`.
    Puck {
        /// Whether a pad is in the slot as it is made.
        joined: bool,
    },
}

impl SteamTransport {
    /// The product number the fake reports on this transport.
    #[must_use]
    pub const fn product(self) -> u16 {
        match self {
            Self::Cable => 0x1302,
            Self::Bluetooth => 0x1303,
            Self::Puck { .. } => 0x1304,
        }
    }

    const fn bus(self) -> u16 {
        match self {
            Self::Bluetooth => BUS_BLUETOOTH,
            Self::Cable | Self::Puck { .. } => BUS_USB,
        }
    }
}

/// What the fake was sent, in the order it arrived.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Received {
    /// A feature report set, as it arrived: its number, then its bytes.
    SetFeature(Vec<u8>),
    /// A feature report asked for, by its number.
    GetFeature(u8),
    /// An output report, as it arrived: its number, then its bytes.
    Output(Vec<u8>),
}

impl Received {
    /// The command a feature report set carries: its first byte after
    /// the report's number, with what follows it.
    #[must_use]
    pub fn command(&self) -> Option<&[u8]> {
        match self {
            Self::SetFeature(report) => report.get(1..),
            Self::GetFeature(_) | Self::Output(_) => None,
        }
    }
}

/// A button of the pad, or a capacitive touch reported with them,
/// by its place among the 32 bits of buttons in a state report. A
/// trackpad's touch is its [`Trackpad`]'s.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum SteamButton {
    /// A, the south face button.
    South,
    /// B, the east face button.
    East,
    /// X, the west face button.
    West,
    /// Y, the north face button.
    North,
    /// The quick-access button, below the right trackpad.
    QuickAccess,
    /// The right stick, clicked.
    RightStickClick,
    /// The menu button, which the kernel's driver also switches lizard
    /// mode with.
    Menu,
    /// The upper right back button, `paddle-1`.
    UpperRightBack,
    /// The lower right back button, `paddle-3`.
    LowerRightBack,
    /// The right bumper.
    RightBumper,
    /// The D-pad's down.
    DpadDown,
    /// The D-pad's right.
    DpadRight,
    /// The D-pad's left.
    DpadLeft,
    /// The D-pad's up.
    DpadUp,
    /// The view button.
    View,
    /// The left stick, clicked.
    LeftStickClick,
    /// The Steam button.
    Steam,
    /// The upper left back button, `paddle-2`.
    UpperLeftBack,
    /// The lower left back button, `paddle-4`.
    LowerLeftBack,
    /// The left bumper.
    LeftBumper,
    /// A thumb on the right stick.
    RightStickTouch,
    /// The right trackpad, clicked.
    RightPadClick,
    /// The right trigger, pulled home.
    RightTriggerClick,
    /// A thumb on the left stick.
    LeftStickTouch,
    /// The left trackpad, clicked.
    LeftPadClick,
    /// The left trigger, pulled home.
    LeftTriggerClick,
    /// A hand on the right grip.
    RightGripTouch,
    /// A hand on the left grip.
    LeftGripTouch,
}

impl SteamButton {
    /// The button's bit in the report's 32 bits of buttons, which are
    /// little-endian from the report's third byte.
    #[must_use]
    pub const fn bit(self) -> u32 {
        let place = match self {
            Self::South => 0,
            Self::East => 1,
            Self::West => 2,
            Self::North => 3,
            Self::QuickAccess => 4,
            Self::RightStickClick => 5,
            Self::Menu => 6,
            Self::UpperRightBack => 7,
            Self::LowerRightBack => 8,
            Self::RightBumper => 9,
            Self::DpadDown => 10,
            Self::DpadRight => 11,
            Self::DpadLeft => 12,
            Self::DpadUp => 13,
            Self::View => 14,
            Self::LeftStickClick => 15,
            Self::Steam => 16,
            Self::UpperLeftBack => 17,
            Self::LowerLeftBack => 18,
            Self::LeftBumper => 19,
            Self::RightStickTouch => 20,
            Self::RightPadClick => 22,
            Self::RightTriggerClick => 23,
            Self::LeftStickTouch => 24,
            Self::LeftPadClick => 26,
            Self::LeftTriggerClick => 27,
            Self::RightGripTouch => 28,
            Self::LeftGripTouch => 29,
        };
        1 << place
    }
}

/// The right trackpad's touch, among the buttons.
const RIGHT_PAD_TOUCH: u32 = 1 << 21;
/// The left trackpad's touch, among the buttons.
const LEFT_PAD_TOUCH: u32 = 1 << 25;

/// One trackpad as a state report has it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Trackpad {
    /// Whether a thumb is on it: a bit among the buttons.
    pub touched: bool,
    /// Where across, as the pad reports it.
    pub x: i16,
    /// Where up or down, as the pad reports it.
    pub y: i16,
    /// How hard it is pressed.
    pub pressure: u16,
}

/// Which of the pad's state reports a state is sent as.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum StateReport {
    /// `0x42`, 54 bytes: everything, the orientation too.
    Full,
    /// `0x45`, 46 bytes: everything but the orientation. The kernel's
    /// driver calls it `REPORT_ID_INPUT2`; SDL's names it for Bluetooth
    /// LE (`ID_TRITON_CONTROLLER_STATE_BLE`).
    WithoutOrientation,
    /// `0x47`, 46 bytes: a newer report SDL reads and the kernel's driver
    /// doesn't, with the trackpads' clock, and the sensors' clock in 16
    /// bits of 32 µs ticks. A fake declares it only when
    /// [made to](SteamControllerBuilder::declaring_report_47).
    WithShortClocks,
}

impl StateReport {
    /// The report's number.
    #[must_use]
    pub const fn id(self) -> u8 {
        match self {
            Self::Full => STATE,
            Self::WithoutOrientation => STATE_WITHOUT_ORIENTATION,
            Self::WithShortClocks => STATE_WITH_SHORT_CLOCKS,
        }
    }

    /// The report's length, with its number.
    #[must_use]
    pub const fn size(self) -> usize {
        match self {
            Self::Full => 54,
            Self::WithoutOrientation | Self::WithShortClocks => 46,
        }
    }
}

/// What a state report says, each value as the pad reports it:
/// no axis inverted, no unit converted.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SteamState {
    /// The buttons held and the capacitive touches, as their bits; see
    /// [`SteamButton::bit`].
    pub buttons: u32,
    /// The left trigger's pull.
    pub left_trigger: i16,
    /// The right trigger's pull.
    pub right_trigger: i16,
    /// The left stick, across and up or down.
    pub left_stick: [i16; 2],
    /// The right stick, across and up or down.
    pub right_stick: [i16; 2],
    /// The left trackpad.
    pub left_pad: Trackpad,
    /// The right trackpad.
    pub right_pad: Trackpad,
    /// The sensors' clock, in microseconds. A [`StateReport::WithShortClocks`]
    /// carries it in ticks of 32 µs, wrapping at 16 bits, as SDL reads
    /// that report.
    pub sensor_clock: u32,
    /// The trackpads' clock, which only a [`StateReport::WithShortClocks`]
    /// carries.
    pub trackpad_clock: u16,
    /// The accelerometer's three axes, in the report's order.
    pub accel: [i16; 3],
    /// The gyroscope's three axes, in the report's order.
    pub gyro: [i16; 3],
    /// The pad's orientation as a quaternion, W, X, Y and Z, which
    /// only a [`StateReport::Full`] carries.
    pub orientation: [i16; 4],
}

impl SteamState {
    /// This state with `button` held.
    #[must_use]
    pub const fn with(mut self, button: SteamButton) -> Self {
        self.buttons |= button.bit();
        self
    }

    /// Whether `button` is held.
    #[must_use]
    pub const fn holds(&self, button: SteamButton) -> bool {
        self.buttons & button.bit() != 0
    }

    /// A state with something in every control the kernel's driver
    /// presents, each far enough from rest that the driver's fuzz passes
    /// it whole: south and two back buttons (upper right, lower left)
    /// held, the triggers at 1000 and 2000, the left stick at (-20000,
    /// 12000) and the right at (3000, -3000), the left trackpad touched at
    /// (1234, -2345) and the right untouched, the accelerometer at (1000,
    /// -2000, 16384) and the gyroscope at (16, -32, 48).
    #[must_use]
    pub const fn busy() -> Self {
        Self {
            buttons: SteamButton::South.bit()
                | SteamButton::UpperRightBack.bit()
                | SteamButton::LowerLeftBack.bit(),
            left_trigger: 1000,
            right_trigger: 2000,
            left_stick: [-20000, 12000],
            right_stick: [3000, -3000],
            left_pad: Trackpad {
                touched: true,
                x: 1234,
                y: -2345,
                pressure: 500,
            },
            right_pad: Trackpad {
                touched: false,
                x: 3456,
                y: -4567,
                pressure: 0,
            },
            sensor_clock: 4000,
            trackpad_clock: 0,
            accel: [1000, -2000, 16384],
            gyro: [16, -32, 48],
            orientation: [0; 4],
        }
    }

    /// The state as report `kind`, numbered `sequence`.
    #[must_use]
    pub fn report(&self, kind: StateReport, sequence: u8) -> Vec<u8> {
        let mut report = vec![0u8; kind.size()];
        report[0] = kind.id();
        report[1] = sequence;
        let mut buttons = self.buttons;
        if self.left_pad.touched {
            buttons |= LEFT_PAD_TOUCH;
        }
        if self.right_pad.touched {
            buttons |= RIGHT_PAD_TOUCH;
        }
        report[2..6].copy_from_slice(&buttons.to_le_bytes());
        let mut at = 6;
        let mut put = |bytes: &[u8]| {
            report[at..at + bytes.len()].copy_from_slice(bytes);
            at += bytes.len();
        };
        put(&self.left_trigger.to_le_bytes());
        put(&self.right_trigger.to_le_bytes());
        for value in self.left_stick.iter().chain(&self.right_stick) {
            put(&value.to_le_bytes());
        }
        if kind == StateReport::WithShortClocks {
            put(&self.trackpad_clock.to_le_bytes());
        }
        for pad in [self.left_pad, self.right_pad] {
            put(&pad.x.to_le_bytes());
            put(&pad.y.to_le_bytes());
            put(&pad.pressure.to_le_bytes());
        }
        if kind == StateReport::WithShortClocks {
            let [low, high, ..] = (self.sensor_clock / NARROW_TICK_US).to_le_bytes();
            put(&[low, high]);
        } else {
            put(&self.sensor_clock.to_le_bytes());
        }
        for value in self.accel.iter().chain(&self.gyro) {
            put(&value.to_le_bytes());
        }
        if kind == StateReport::Full {
            for value in self.orientation {
                put(&value.to_le_bytes());
            }
        }
        report
    }
}

/// How the battery is being charged, as a battery report says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum ChargeState {
    /// Being discharged: in use with no charger.
    Discharging,
    /// Being charged.
    Charging,
    /// Charged full.
    Charged,
}

impl ChargeState {
    const fn code(self) -> u8 {
        match self {
            Self::Discharging => 1,
            Self::Charging => 2,
            Self::Charged => 4,
        }
    }
}

/// What a battery report says.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SteamBattery {
    /// How it is being charged.
    pub charge: ChargeState,
    /// How full it is, in percent.
    pub level: u8,
    /// The battery's voltage, in millivolts.
    pub battery_millivolts: u16,
    /// The pad's own voltage, in millivolts.
    pub system_millivolts: u16,
    /// The charger's voltage, in millivolts.
    pub input_millivolts: u16,
    /// The battery's current, in milliamperes.
    pub battery_milliamps: u16,
    /// The charger's current, in milliamperes.
    pub input_milliamps: u16,
    /// The temperature, as the pad reports it.
    pub temperature: u16,
}

impl SteamBattery {
    /// The battery report, `0x43`, 15 bytes.
    #[must_use]
    pub fn report(&self) -> Vec<u8> {
        let mut report = vec![BATTERY, self.charge.code(), self.level];
        for value in [
            self.battery_millivolts,
            self.system_millivolts,
            self.input_millivolts,
            self.battery_milliamps,
            self.input_milliamps,
            self.temperature,
        ] {
            report.extend_from_slice(&value.to_le_bytes());
        }
        report
    }
}

/// A short item of a report descriptor with one byte of data.
const fn item1(prefix: u8, data: u8) -> [u8; 2] {
    [prefix | 1, data]
}

/// A short item of a report descriptor with two bytes of data,
/// little-endian. The fake has no item of four bytes, whose size code
/// would be 3, not 4.
const fn item2(prefix: u8, data: u16) -> [u8; 3] {
    let [low, high] = data.to_le_bytes();
    [prefix | 2, low, high]
}

/// One report of Valve's collection: its number, its length after the
/// number in bytes, and the main item it is (input, output or feature).
fn declared(id: u8, length: u8, main: u8) -> Vec<u8> {
    [
        item1(0x84, id),     // Report ID.
        item1(0x94, length), // Report count.
        item1(0x08, id),     // Usage.
        item1(main, 0x02),   // Data, variable, absolute.
    ]
    .concat()
}

const INPUT: u8 = 0x80;
const OUTPUT: u8 = 0x90;
const FEATURE: u8 = 0xb0;

/// Valve's vendor collection, as the fake declares it: the input reports
/// it can send, each at the length it sends it, the output reports the
/// kernel's driver writes, and the two feature reports commands go
/// through, each byte a value of 0 to 255. With `report_47`, the newer
/// state report too.
fn valve_collection(report_47: bool) -> Vec<u8> {
    let mut collection: Vec<u8> = [
        &item2(0x04, 0xff00)[..], // Usage page: vendor 0xff00.
        &item1(0x08, 0x01),       // Usage 1.
        &item1(0xa0, 0x01),       // Collection: application.
        &item1(0x14, 0x00),       // Logical minimum 0.
        &item2(0x24, 0x00ff),     // Logical maximum 255.
        &item1(0x74, 0x08),       // Report size: 8 bits.
    ]
    .concat();
    let mut inputs = vec![
        (STATE, StateReport::Full.size()),
        (
            STATE_WITHOUT_ORIENTATION,
            StateReport::WithoutOrientation.size(),
        ),
        (BATTERY, 15),
        (WIRELESS, 2),
    ];
    if report_47 {
        inputs.push((STATE_WITH_SHORT_CLOCKS, StateReport::WithShortClocks.size()));
    }
    for (id, length) in inputs {
        let after = u8::try_from(length - 1).expect("every report is shorter than 256 bytes");
        collection.extend(declared(id, after, INPUT));
    }
    collection.extend(declared(RUMBLE, 9, OUTPUT));
    collection.extend(declared(PULSE, 7, OUTPUT));
    collection.extend(declared(CONTROLLER_COMMANDS, 63, FEATURE));
    collection.extend(declared(RECEIVER_COMMANDS, 63, FEATURE));
    collection.push(0xc0); // End of the collection.
    collection
}

/// A mouse, as lizard mode's pointer: two buttons and a relative X and Y,
/// report `0x40`. Written here.
const LIZARD_MOUSE: &[u8] = &[
    0x05, 0x01, // Usage page: generic desktop.
    0x09, 0x02, // Usage: mouse.
    0xa1, 0x01, // Collection: application.
    0x85, 0x40, //   Report ID 0x40.
    0x09, 0x01, //   Usage: pointer.
    0xa1, 0x00, //   Collection: physical.
    0x05, 0x09, //     Usage page: buttons.
    0x19, 0x01, //     Usage minimum 1.
    0x29, 0x02, //     Usage maximum 2.
    0x15, 0x00, //     Logical minimum 0.
    0x25, 0x01, //     Logical maximum 1.
    0x75, 0x01, //     Report size: 1 bit.
    0x95, 0x02, //     Report count 2.
    0x81, 0x02, //     Input: data, variable, absolute.
    0x95, 0x06, //     Report count 6.
    0x81, 0x01, //     Input: padding.
    0x05, 0x01, //     Usage page: generic desktop.
    0x09, 0x30, //     Usage: X.
    0x09, 0x31, //     Usage: Y.
    0x15, 0x81, //     Logical minimum -127.
    0x25, 0x7f, //     Logical maximum 127.
    0x75, 0x08, //     Report size: 8 bits.
    0x95, 0x02, //     Report count 2.
    0x81, 0x06, //     Input: data, variable, relative.
    0xc0, //   End of the collection.
    0xc0, // End of the collection.
];

/// A keyboard, as lizard mode's keys: modifiers and six keys, report
/// `0x41`. Written here.
const LIZARD_KEYBOARD: &[u8] = &[
    0x05, 0x01, // Usage page: generic desktop.
    0x09, 0x06, // Usage: keyboard.
    0xa1, 0x01, // Collection: application.
    0x85, 0x41, //   Report ID 0x41.
    0x05, 0x07, //   Usage page: keys.
    0x19, 0xe0, //   Usage minimum: left control.
    0x29, 0xe7, //   Usage maximum: right GUI.
    0x15, 0x00, //   Logical minimum 0.
    0x25, 0x01, //   Logical maximum 1.
    0x75, 0x01, //   Report size: 1 bit.
    0x95, 0x08, //   Report count 8.
    0x81, 0x02, //   Input: data, variable, absolute.
    0x19, 0x00, //   Usage minimum 0.
    0x29, 0x65, //   Usage maximum: application.
    0x25, 0x65, //   Logical maximum 0x65.
    0x75, 0x08, //   Report size: 8 bits.
    0x95, 0x06, //   Report count 6.
    0x81, 0x00, //   Input: data, array.
    0xc0, // End of the collection.
];

/// Proof that this process runs in one of the virtual machine checks,
/// where a fake that can type and point troubles nobody: the machine is a
/// virtual one, and its configuration says it is a check's. Nothing else
/// makes one.
#[derive(Debug)]
pub struct InVirtualMachineCheck(());

/// The file a virtual machine check's configuration puts in place to say
/// it is one.
pub const VM_CHECK_MARKER: &str = "/etc/ctrlassist-vm-check";

impl InVirtualMachineCheck {
    /// The proof, if this is a virtual machine check: the machine says it
    /// is a virtual one, and [`VM_CHECK_MARKER`] is there and root's, so
    /// that no user could have put it there for a fake on their desktop.
    ///
    /// A machine says it is virtual by any of: an x86 processor's
    /// `hypervisor` flag, a hypervisor the kernel names in
    /// `/sys/hypervisor/type`, firmware that names QEMU or KVM as the
    /// machine's maker or product, or, on ARM, the device tree of QEMU's
    /// `virt` machine, which the checks run on there.
    #[must_use]
    pub fn confirm() -> Option<Self> {
        use std::os::unix::fs::MetadataExt as _;
        let read = |path: &str| std::fs::read_to_string(path).unwrap_or_default();
        let flagged = read("/proc/cpuinfo")
            .lines()
            .filter(|line| line.starts_with("flags"))
            .any(|line| line.split_whitespace().any(|flag| flag == "hypervisor"));
        let named = !read("/sys/hypervisor/type").trim().is_empty();
        let firmware = [
            "/sys/class/dmi/id/sys_vendor",
            "/sys/class/dmi/id/product_name",
        ]
        .iter()
        .any(|path| {
            let text = read(path);
            text.contains("QEMU") || text.contains("KVM")
        });
        let device_tree = read("/proc/device-tree/compatible")
            .split('\0')
            .any(|compatible| compatible == "linux,dummy-virt");
        let virtual_machine = flagged || named || firmware || device_tree;
        let marked = std::fs::metadata(VM_CHECK_MARKER).is_ok_and(|marker| marker.uid() == 0);
        (virtual_machine && marked).then_some(Self(()))
    }
}

/// How a fake Steam Controller is to be made; see
/// [`FakeSteamController::builder`].
#[derive(Debug)]
#[must_use = "a builder makes nothing until it is built"]
pub struct SteamControllerBuilder {
    transport: SteamTransport,
    report_47: bool,
    lizard: bool,
}

impl SteamControllerBuilder {
    /// Connected by `transport`, not a cable.
    pub const fn transport(mut self, transport: SteamTransport) -> Self {
        self.transport = transport;
        self
    }

    /// Declaring the newer state report, `0x47`, too, as a pad's
    /// firmware may.
    pub const fn declaring_report_47(mut self) -> Self {
        self.report_47 = true;
        self
    }

    /// With lizard mode's mouse and keyboard in its HID device beside
    /// Valve's collection, as a real Steam Controller's descriptor has them.
    /// Only in a virtual machine check: such a fake can type and point
    /// wherever the driver lets it.
    pub const fn with_lizard_mouse_and_keyboard(mut self, _check: InVirtualMachineCheck) -> Self {
        self.lizard = true;
        self
    }

    /// Makes the fake.
    ///
    /// # Errors
    ///
    /// Fails if uhid can't be opened or the kernel refuses the device.
    pub fn build(self) -> io::Result<FakeSteamController> {
        FakeSteamController::make(&self)
    }
}

/// A stand-in for a Steam Controller (2026), made through uhid with
/// Valve's vendor number and the transport's product number, for the
/// kernel's `hid-steam` to bind. It answers what the driver asks as it
/// binds (the serial, the attributes, a Puck's wireless state) and
/// acknowledges every other command, so binding waits on no timeout; it
/// keeps everything it is sent, in order; and it sends the reports it is
/// told to.
///
/// It is marked as the tests' by where it says it is plugged, its
/// `phys`, which the driver gives its input devices too. Its serial,
/// which they carry as `uniq`, is its own and starts `CATEST`. Dropping
/// it removes it.
pub struct FakeSteamController {
    uhid: Arc<File>,
    received: Arc<Log>,
    joined: Arc<AtomicBool>,
    stop: Arc<OwnedFd>,
    serving: Option<std::thread::JoinHandle<()>>,
    sequence: AtomicU8,
    report_47: bool,
    phys: String,
    serial: String,
}

impl std::fmt::Debug for FakeSteamController {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeSteamController")
            .field("phys", &self.phys)
            .field("serial", &self.serial)
            .finish_non_exhaustive()
    }
}

/// Appends `text` as a fixed-width, zero-terminated field.
fn field(event: &mut Vec<u8>, text: &str, width: usize) {
    let mut bytes = text.as_bytes().to_vec();
    bytes.truncate(width - 1);
    bytes.resize(width, 0);
    event.extend_from_slice(&bytes);
}

/// How long a wait for the driver or the fake gives up after: only so
/// that a test whose wait fails ends, never so that one passes.
const GIVE_UP: Duration = Duration::from_secs(10);

/// A socket the kernel sends its uevents to, unread ones kept; none
/// where netlink can't be used.
fn kernel_uevents() -> Option<OwnedFd> {
    use rustix::net::{AddressFamily, SocketFlags, SocketType, bind, netlink, socket_with};
    let events = socket_with(
        AddressFamily::NETLINK,
        SocketType::DGRAM,
        SocketFlags::CLOEXEC | SocketFlags::NONBLOCK,
        Some(netlink::KOBJECT_UEVENT),
    )
    .ok()?;
    // The kernel's group of uevents, 1.
    bind(&events, &netlink::SocketAddrNetlink::new(0, 1)).ok()?;
    Some(events)
}

/// Whether `message`, a kernel uevent, says `hid-steam` has bound the
/// HID device that says it is plugged at `phys`, and not the client
/// device the driver makes beside it, in its own group, `0103`.
fn says_bound(message: &[u8], phys: &str) -> bool {
    let mut fields = message.split(|byte| *byte == 0);
    let bind = fields
        .next()
        .is_some_and(|header| header.starts_with(b"bind@"));
    let (mut driver, mut plugged, mut client) = (false, false, false);
    for field in fields {
        driver |= field == b"DRIVER=hid-steam";
        plugged |= field.strip_prefix(b"HID_PHYS=") == Some(phys.as_bytes());
        client |= field.starts_with(b"MODALIAS=") && field.windows(5).any(|part| part == b"g0103");
    }
    bind && driver && plugged && !client
}

/// Waits for `found` to find something, looking again each time the
/// kernel says a device came, went or changed, until [`GIVE_UP`]. The
/// kernel's own uevents are listened to, not udev's, so that this needs
/// no udev and wakes as soon as sysfs has changed. Where they can't be
/// listened to, it looks once more at the end.
fn eventually<T>(mut found: impl FnMut() -> Option<T>) -> Option<T> {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    use rustix::net::{RecvFlags, recv};
    // Listening first, so that nothing that happens between the look and
    // the wait is missed.
    let events = kernel_uevents();
    let deadline = Instant::now() + GIVE_UP;
    let mut buffer = [0u8; 8192];
    loop {
        if let Some(found) = found() {
            return Some(found);
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        let wait = Timespec::try_from(left).expect("ten seconds fit in a timespec");
        match &events {
            Some(events) => {
                let mut fds = [PollFd::new(events, PollFlags::IN)];
                let _ = poll(&mut fds, Some(&wait));
                // What the events say isn't read: the look after them is.
                while recv(events, &mut buffer, RecvFlags::DONTWAIT).is_ok() {}
            }
            None => {
                let _ = poll(&mut [], Some(&wait));
            }
        }
    }
}

/// An input device the kernel's driver made for a fake.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SteamInput {
    /// Its name, which the driver gives it.
    pub name: String,
    /// Its `uniq`, which the driver gives the pad's serial.
    pub uniq: String,
    /// Its event node.
    pub event: PathBuf,
}

/// The name the driver gives the gamepad of a pad on a cable or
/// Bluetooth.
pub const GAMEPAD: &str = "Steam Controller";
/// The name the driver gives the gamepad of a pad behind a Puck.
pub const WIRELESS_GAMEPAD: &str = "Wireless Steam Controller";
/// The name the driver gives the motion sensors' input device.
pub const MOTION: &str = "Steam Controller Motion Sensors";

fn trimmed(path: &Path) -> String {
    std::fs::read_to_string(path)
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

impl FakeSteamController {
    /// A fake on a USB cable.
    ///
    /// # Errors
    ///
    /// Fails if uhid can't be opened or the kernel refuses the device.
    pub fn new() -> io::Result<Self> {
        Self::builder().build()
    }

    /// A fake made some other way: another transport, the newer state
    /// report declared.
    pub const fn builder() -> SteamControllerBuilder {
        SteamControllerBuilder {
            transport: SteamTransport::Cable,
            report_47: false,
            lizard: false,
        }
    }

    /// The report descriptor a fake made by `builder` has.
    #[must_use]
    pub fn descriptor(builder: &SteamControllerBuilder) -> Vec<u8> {
        let valve = valve_collection(builder.report_47);
        if builder.lizard {
            [LIZARD_MOUSE, LIZARD_KEYBOARD, &valve].concat()
        } else {
            valve
        }
    }

    fn make(builder: &SteamControllerBuilder) -> io::Result<Self> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let phys = format!("{}steam/{pid}/{number}", ctrlassist_device::TEST_MARKER);
        // At most 13 characters, so that the battery the driver names
        // `steam-<serial>` has a thermal zone, whose name is at most 19.
        let serial = format!("CATEST{}{}", base36(pid, 5), base36(number, 2));
        let joined = match builder.transport {
            SteamTransport::Puck { joined } => joined,
            SteamTransport::Cable | SteamTransport::Bluetooth => true,
        };
        let descriptor = Self::descriptor(builder);
        // Listening before the device is made, so that the driver's
        // binding it is heard.
        let uevents = kernel_uevents();

        let mut uhid = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32)
            .open("/dev/uhid")?;
        // UHID_CREATE2, packed: type u32, name[128], phys[64], uniq[64],
        // rd_size u16, bus u16, vendor u32, product u32, version u32,
        // country u32, rd_data[4096].
        let mut event = Vec::with_capacity(UHID_EVENT);
        event.extend_from_slice(&UHID_CREATE2.to_ne_bytes());
        field(&mut event, "CtrlAssist Test Steam Controller", 128);
        field(&mut event, &phys, 64);
        field(&mut event, &serial, 64);
        let size = u16::try_from(descriptor.len())
            .expect("the descriptor is a few hundred bytes, so its length fits in u16");
        event.extend_from_slice(&size.to_ne_bytes());
        event.extend_from_slice(&builder.transport.bus().to_ne_bytes());
        event.extend_from_slice(&u32::from(VALVE).to_ne_bytes());
        event.extend_from_slice(&u32::from(builder.transport.product()).to_ne_bytes());
        event.extend_from_slice(&VERSION.to_ne_bytes());
        event.extend_from_slice(&0u32.to_ne_bytes());
        let mut padded = descriptor;
        padded.resize(UHID_DATA, 0);
        event.extend_from_slice(&padded);
        uhid.write_all(&event)?;

        let uhid = Arc::new(uhid);
        let received = Arc::new(Log::default());
        received.heard.store(uevents.is_some(), Ordering::Release);
        let joined = Arc::new(AtomicBool::new(joined));
        let stop = Arc::new(rustix::event::eventfd(
            0,
            rustix::event::EventfdFlags::CLOEXEC,
        )?);
        let answers = Answers {
            serial: serial.clone(),
            product: builder.transport.product(),
            joined: Arc::clone(&joined),
        };
        let serving = {
            let (uhid, received, stop) =
                (Arc::clone(&uhid), Arc::clone(&received), Arc::clone(&stop));
            let phys = phys.clone();
            std::thread::Builder::new()
                .name("fake-steam".into())
                .spawn(move || {
                    let heard = uevents.as_ref().map(|events| (events, phys.as_str()));
                    serve(&uhid, &received, &stop, heard, &answers);
                })?
        };
        Ok(Self {
            uhid,
            received,
            joined,
            stop,
            serving: Some(serving),
            sequence: AtomicU8::new(0),
            report_47: builder.report_47,
            phys,
            serial,
        })
    }

    /// Where it says it is plugged, which marks it as the tests'.
    #[must_use]
    pub fn phys(&self) -> &str {
        &self.phys
    }

    /// The serial it answers the driver with.
    #[must_use]
    pub fn serial(&self) -> &str {
        &self.serial
    }

    /// Everything it was sent, in the order it arrived.
    ///
    /// # Panics
    ///
    /// Panics if the serving thread panicked while noting something.
    #[must_use]
    pub fn received(&self) -> Vec<Received> {
        self.received
            .entries
            .lock()
            .expect("nothing panics while the log is held")
            .clone()
    }

    /// Waits until what it was sent satisfies `wanted`, woken as each
    /// thing arrives, for ten seconds at most; whether it did.
    ///
    /// # Panics
    ///
    /// Panics if the serving thread panicked while noting something.
    pub fn wait_for(&self, wanted: impl Fn(&[Received]) -> bool) -> bool {
        let deadline = Instant::now() + GIVE_UP;
        let mut entries = self
            .received
            .entries
            .lock()
            .expect("nothing panics while the log is held");
        loop {
            if wanted(&entries) {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            entries = self
                .received
                .arrived
                .wait_timeout(entries, left)
                .expect("nothing panics while the log is held")
                .0;
        }
    }

    /// Sends `report` as it is: its number, then its bytes.
    ///
    /// # Errors
    ///
    /// Fails if the kernel refuses the write.
    pub fn send(&self, report: &[u8]) -> io::Result<()> {
        // UHID_INPUT2: type u32, size u16, then the report.
        let mut event = UHID_INPUT2.to_ne_bytes().to_vec();
        let size = u16::try_from(report.len())
            .ok()
            .filter(|size| usize::from(*size) <= UHID_DATA)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "uhid takes a report of at most 4096 bytes",
                )
            })?;
        event.extend_from_slice(&size.to_ne_bytes());
        event.extend_from_slice(report);
        (&*self.uhid).write_all(&event)
    }

    /// Sends `state` as report `kind`, numbered one on from the last.
    /// The kernel has handled it by the time this returns.
    ///
    /// # Errors
    ///
    /// Fails if the kernel refuses the write, or `kind` is the newer state
    /// report and the fake doesn't declare it.
    pub fn send_state(&self, state: &SteamState, kind: StateReport) -> io::Result<()> {
        if kind == StateReport::WithShortClocks && !self.report_47 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the fake doesn't declare report 0x47, so the kernel would drop it",
            ));
        }
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        self.send(&state.report(kind, sequence))
    }

    /// Sends a battery report.
    ///
    /// # Errors
    ///
    /// Fails if the kernel refuses the write.
    pub fn send_battery(&self, battery: &SteamBattery) -> io::Result<()> {
        self.send(&battery.report())
    }

    /// Says a pad joined the slot, or left it, as a Puck's slot
    /// does with report `0x79`; the slot's wireless state is answered so
    /// from then on.
    ///
    /// # Errors
    ///
    /// Fails if the kernel refuses the write.
    pub fn send_wireless(&self, joined: bool) -> io::Result<()> {
        self.joined.store(joined, Ordering::Release);
        self.send(&wireless_report(joined))
    }

    /// The input devices the kernel's driver has made for it now.
    #[must_use]
    pub fn inputs(&self) -> Vec<SteamInput> {
        let Ok(entries) = std::fs::read_dir("/sys/class/input") else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let device = entry.path();
            if !entry.file_name().to_string_lossy().starts_with("input")
                || trimmed(&device.join("phys")) != self.phys
            {
                continue;
            }
            let event = std::fs::read_dir(&device).ok().and_then(|children| {
                children.flatten().find_map(|child| {
                    let name = child.file_name().to_string_lossy().into_owned();
                    name.starts_with("event")
                        .then(|| Path::new("/dev/input").join(name))
                })
            });
            if let Some(event) = event {
                found.push(SteamInput {
                    name: trimmed(&device.join("name")),
                    uniq: trimmed(&device.join("uniq")),
                    event,
                });
            }
        }
        found.sort_by(|a, b| a.name.cmp(&b.name));
        found
    }

    /// The driver's gamepad, once it has made it and has bound the fake;
    /// nothing if it doesn't within ten seconds.
    ///
    /// The driver makes its devices while it binds the fake, and the HID
    /// core drops every report the fake sends until binding is done; so
    /// this, [`motion`](Self::motion), [`raw_node`](Self::raw_node) and
    /// [`battery`](Self::battery) each wait for that too, and a report
    /// sent after any of them returns is taken.
    #[must_use]
    pub fn gamepad(&self) -> Option<SteamInput> {
        self.wait_bound()?;
        eventually(|| {
            self.inputs()
                .into_iter()
                .find(|input| input.name == GAMEPAD || input.name == WIRELESS_GAMEPAD)
        })
    }

    /// The driver's motion sensors, once it has made them and has bound
    /// the fake; nothing if it doesn't within ten seconds.
    #[must_use]
    pub fn motion(&self) -> Option<SteamInput> {
        self.wait_bound()?;
        eventually(|| self.inputs().into_iter().find(|input| input.name == MOTION))
    }

    /// Waits until the driver has taken its gamepad and motion sensors
    /// away, as it does while its raw node is open; whether it did within
    /// ten seconds.
    #[must_use]
    pub fn inputs_gone(&self) -> bool {
        eventually(|| {
            self.inputs()
                .iter()
                .all(|input| {
                    input.name != MOTION && input.name != GAMEPAD && input.name != WIRELESS_GAMEPAD
                })
                .then_some(())
        })
        .is_some()
    }

    /// Its raw node, once the driver has made it and has bound the fake;
    /// nothing if it doesn't within ten seconds. It is the node of the
    /// client HID device the driver makes beside the fake's own, which has
    /// none: the one raw node whose HID device says it is plugged where
    /// the fake does.
    #[must_use]
    pub fn raw_node(&self) -> Option<PathBuf> {
        self.wait_bound()?;
        eventually(|| {
            std::fs::read_dir("/sys/class/hidraw")
                .ok()?
                .flatten()
                .find(|entry| {
                    trimmed(&entry.path().join("device/uevent"))
                        .lines()
                        .any(|line| line.strip_prefix("HID_PHYS=") == Some(self.phys.as_str()))
                })
                .map(|entry| Path::new("/dev").join(entry.file_name()))
        })
    }

    /// The driver bound to the fake's own HID device, if any: `hid-steam`,
    /// or `hid-generic` where `hid-steam` is blacklisted. It is the HID
    /// device that says it is plugged where the fake does and isn't the
    /// client device `hid-steam` makes beside it, which is in the
    /// driver's own group, `0103`. The device's link to its driver is
    /// made as the driver's probe begins, not once it is done.
    fn driver_linked(&self) -> Option<String> {
        let device = std::fs::read_dir("/sys/bus/hid/devices")
            .ok()?
            .flatten()
            .map(|entry| entry.path())
            .find(|device| {
                let uevent = trimmed(&device.join("uevent"));
                let says = |key: &str| {
                    uevent
                        .lines()
                        .find_map(|line| line.strip_prefix(key))
                        .unwrap_or_default()
                        .to_string()
                };
                says("HID_PHYS=") == self.phys && !says("MODALIAS=").contains("g0103")
            })?;
        let driver = std::fs::read_link(device.join("driver")).ok()?;
        Some(driver.file_name()?.to_string_lossy().into_owned())
    }

    /// Waits until `hid-steam` has bound the fake, for ten seconds at
    /// most; something if it did. The kernel says so with a `bind` uevent,
    /// sent once the driver's probe has returned, which the serving thread
    /// listens for. Where uevents can't be listened to, the driver's link
    /// is taken instead, which comes as the probe begins: a report sent
    /// at once may then still be dropped.
    fn wait_bound(&self) -> Option<()> {
        if !self.received.heard.load(Ordering::Acquire) {
            return eventually(|| (self.driver_linked()? == "hid-steam").then_some(()));
        }
        let deadline = Instant::now() + GIVE_UP;
        let mut entries = self
            .received
            .entries
            .lock()
            .expect("nothing panics while the log is held");
        loop {
            if self.received.bound.load(Ordering::Acquire) {
                return Some(());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            entries = self
                .received
                .arrived
                .wait_timeout(entries, left)
                .expect("nothing panics while the log is held")
                .0;
        }
    }

    /// Whether the kernel's `hid-steam` binds the fake, within ten
    /// seconds. Another driver may hold it for a while first: where the
    /// module loads only for the fake, `hid-generic` takes it until
    /// `hid-steam` is registered and claims it.
    #[must_use]
    pub fn bound_by_hid_steam(&self) -> bool {
        self.wait_bound().is_some()
    }

    /// The battery the driver makes for the fake, a power supply named
    /// `steam-<serial>`, once it has made it and has bound the fake;
    /// nothing if it doesn't within ten seconds.
    #[must_use]
    pub fn battery(&self) -> Option<PathBuf> {
        self.wait_bound()?;
        let battery = Path::new("/sys/class/power_supply").join(format!("steam-{}", self.serial));
        eventually(|| battery.exists().then(|| battery.clone()))
    }
}

/// The wireless event a Puck's slot sends, report `0x79`: a pad joined
/// the slot (2), or left it (1).
#[must_use]
pub const fn wireless_report(joined: bool) -> [u8; 2] {
    [WIRELESS, if joined { 2 } else { 1 }]
}

/// `number` in base 36, upper case, in exactly `width` digits, wrapping.
fn base36(mut number: u32, width: usize) -> String {
    const DIGITS: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut digits = vec![b'0'; width];
    for digit in digits.iter_mut().rev() {
        *digit = DIGITS[(number % 36) as usize];
        number /= 36;
    }
    String::from_utf8(digits).expect("base-36 digits are ASCII")
}

impl Drop for FakeSteamController {
    fn drop(&mut self) {
        // Wakes the serving thread to stop.
        let _ = rustix::io::write(&*self.stop, &1u64.to_ne_bytes());
        if let Some(serving) = self.serving.take() {
            let _ = serving.join();
        }
        // Closing uhid, with the serving thread's handle gone, removes
        // the device.
    }
}

/// What the fake was sent, whether `hid-steam` has bound it, and a way to
/// wait for either.
#[derive(Default)]
struct Log {
    entries: Mutex<Vec<Received>>,
    /// Set when the kernel says `hid-steam` has bound the fake: its probe
    /// has returned, and the HID core passes it the fake's reports.
    bound: AtomicBool,
    /// Whether the kernel's uevents could be listened to, without which
    /// `bound` is never set.
    heard: AtomicBool,
    arrived: Condvar,
}

/// What the fake answers the driver's questions with.
struct Answers {
    serial: String,
    product: u16,
    joined: Arc<AtomicBool>,
}

impl Answers {
    /// The reply to a GET of feature report `number`, from the command
    /// last SET on it; nothing, for an error, if none was.
    fn reply(&self, number: u8, last: Option<&[u8]>) -> Option<Vec<u8>> {
        let last = last?;
        let command = *last.get(1)?;
        let mut reply = vec![0u8; 64];
        reply[0] = number;
        reply[1] = command;
        match (number, command) {
            (CONTROLLER_COMMANDS, GET_STRING_ATTRIBUTE) => {
                // The field's length, which attribute, then the string:
                // the serial for the unit's and the board's alike.
                reply[2] = SERIAL_FIELD;
                reply[3] = last.get(3).copied().unwrap_or(1);
                let serial = self.serial.as_bytes();
                let length = serial.len().min(usize::from(SERIAL_FIELD) - 1);
                reply[4..4 + length].copy_from_slice(&serial[..length]);
            }
            (CONTROLLER_COMMANDS, GET_ATTRIBUTES) => {
                // Each attribute a tag and a 32-bit value: the product,
                // capabilities, the firmware's build time and the board,
                // none of them a real pad's, and the connection's
                // interval, 4000 µs, which the driver reads.
                let attributes: [(u8, u32); 5] = [
                    (1, u32::from(self.product)),
                    (2, 0),
                    (4, 0),
                    (9, 0),
                    (11, 4000),
                ];
                let mut at = 3;
                for (tag, value) in attributes {
                    reply[at] = tag;
                    reply[at + 1..at + 5].copy_from_slice(&value.to_le_bytes());
                    at += 5;
                }
                reply[2] = u8::try_from(at - 3).expect("five attributes are 25 bytes");
            }
            (RECEIVER_COMMANDS, GET_WIRELESS_STATE) => {
                reply[2] = 1;
                reply[3] = if self.joined.load(Ordering::Acquire) {
                    2
                } else {
                    1
                };
            }
            _ => {
                // Anything else: the command echoed, an acknowledgment.
                let length = last.len().min(reply.len());
                reply[1..length].copy_from_slice(&last[1..length]);
            }
        }
        Some(reply)
    }
}

/// Answers the kernel through uhid, `handle`, until `stop` is written,
/// noting what it was sent in `log`, and, by the kernel's uevents on
/// `heard`, when `hid-steam` has bound the fake plugged at its `phys`.
fn serve(
    mut handle: &File,
    log: &Log,
    stop: &OwnedFd,
    heard: Option<(&OwnedFd, &str)>,
    answers: &Answers,
) {
    use rustix::event::{PollFd, PollFlags, poll};
    use rustix::net::{RecvFlags, recv};

    let note = |what: Received| {
        log.entries
            .lock()
            .expect("nothing panics while the log is held")
            .push(what);
        log.arrived.notify_all();
    };
    let word = |event: &[u8], at: usize| {
        u32::from_ne_bytes([event[at], event[at + 1], event[at + 2], event[at + 3]])
    };
    let half = |event: &[u8], at: usize| u16::from_ne_bytes([event[at], event[at + 1]]);
    // The last command SET on each feature report, by its number.
    let mut last: Vec<Option<Vec<u8>>> = vec![None; 256];
    let mut event = vec![0u8; UHID_EVENT];
    let mut message = vec![0u8; 8192];
    loop {
        let mut fds = vec![
            PollFd::new(&handle, PollFlags::IN),
            PollFd::new(stop, PollFlags::IN),
        ];
        if let Some((events, _)) = heard {
            fds.push(PollFd::new(events, PollFlags::IN));
        }
        match poll(&mut fds, None) {
            Ok(_) => {}
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => return,
        }
        if !fds[1].revents().is_empty() {
            return;
        }
        if let Some((events, phys)) = heard {
            while let Ok((size, _)) = recv(events, &mut message, RecvFlags::DONTWAIT) {
                if says_bound(&message[..size.min(message.len())], phys) {
                    // Set under the log's lock, so that a waiter between
                    // its look and its wait isn't missed.
                    let _held = log
                        .entries
                        .lock()
                        .expect("nothing panics while the log is held");
                    log.bound.store(true, Ordering::Release);
                    log.arrived.notify_all();
                }
            }
        }
        loop {
            match handle.read(&mut event) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            match word(&event, 0) {
                // data[4096], then its size.
                UHID_OUTPUT => {
                    let size = usize::from(half(&event, 4 + UHID_DATA)).min(UHID_DATA);
                    note(Received::Output(event[4..4 + size].to_vec()));
                }
                // The request's id, the report's number and type.
                UHID_GET_REPORT => {
                    let number = event[8];
                    note(Received::GetFeature(number));
                    let reply = answers.reply(number, last[usize::from(number)].as_deref());
                    // The request's id, an error, a size, the report.
                    let mut answer = UHID_GET_REPORT_REPLY.to_ne_bytes().to_vec();
                    answer.extend_from_slice(&event[4..8]);
                    match reply {
                        Some(reply) => {
                            answer.extend_from_slice(&0u16.to_ne_bytes());
                            answer.extend_from_slice(&(reply.len() as u16).to_ne_bytes());
                            answer.extend_from_slice(&reply);
                        }
                        None => {
                            answer.extend_from_slice(&UHID_REFUSED.to_ne_bytes());
                            answer.extend_from_slice(&0u16.to_ne_bytes());
                        }
                    }
                    let _ = handle.write_all(&answer);
                }
                // The request's id, the report's number and type, a size,
                // the report.
                UHID_SET_REPORT => {
                    let number = event[8];
                    let size = usize::from(half(&event, 10)).min(UHID_DATA);
                    let report = event[12..12 + size].to_vec();
                    note(Received::SetFeature(report.clone()));
                    last[usize::from(number)] = Some(report);
                    let mut answer = UHID_SET_REPORT_REPLY.to_ne_bytes().to_vec();
                    answer.extend_from_slice(&event[4..8]);
                    answer.extend_from_slice(&0u16.to_ne_bytes());
                    let _ = handle.write_all(&answer);
                }
                _ => {}
            }
        }
    }
}

/// The line the running kernel's module aliases have for `hid-steam`
/// taking the 2026 Steam Controller on a cable.
const CABLE_ALIAS: &str = "alias hid:b0003g*v000028DEp00001302 hid_steam";

/// Whether the running kernel's `hid-steam` knows the 2026 Steam Controller,
/// and if not, why not.
///
/// The module's own table isn't in sysfs, loaded or not, so this reads
/// the module aliases `depmod` wrote for the running kernel, where any
/// distribution keeps them (`/lib/modules`, `/usr/lib/modules`, and on
/// NixOS the booted system's), the built-in modules' aliases too. Where
/// none can be read, it goes by the kernel's release: the Steam Controller came
/// with Linux 7.3.
///
/// # Errors
///
/// Says why the kernel doesn't know the Steam Controller.
pub fn kernel_knows_steam_controller() -> Result<(), String> {
    let release = trimmed(Path::new("/proc/sys/kernel/osrelease"));
    let mut read_any = false;
    for root in [
        "/lib/modules",
        "/usr/lib/modules",
        "/run/booted-system/kernel-modules/lib/modules",
    ] {
        for file in ["modules.alias", "modules.builtin.alias"] {
            let Ok(aliases) = std::fs::read_to_string(Path::new(root).join(&release).join(file))
            else {
                continue;
            };
            read_any = true;
            if aliases.lines().any(|line| line.trim() == CABLE_ALIAS) {
                return Ok(());
            }
        }
    }
    if read_any {
        return Err(format!(
            "Linux {release}'s hid-steam doesn't know the 2026 Steam Controller, which came with 7.3"
        ));
    }
    let mut numbers = release
        .split(|c: char| !c.is_ascii_digit())
        .map(|part| part.parse::<u32>().unwrap_or(0));
    let (major, minor) = (numbers.next().unwrap_or(0), numbers.next().unwrap_or(0));
    if (major, minor) >= (7, 3) {
        Ok(())
    } else {
        Err(format!(
            "Linux {release} is older than 7.3, whose hid-steam is the first to know the 2026 Steam Controller"
        ))
    }
}

/// Whether Steam is running here: it takes a fake Steam Controller for
/// one of its own, opening its raw node and taking its input devices
/// away, and may offer it a firmware update.
#[must_use]
pub fn steam_is_running() -> bool {
    let Ok(processes) = std::fs::read_dir("/proc") else {
        return false;
    };
    processes
        .flatten()
        .any(|process| trimmed(&process.path().join("comm")) == "steam")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A report descriptor's items as (tag and type, data), short items
    /// only, which is all the fake's are.
    fn items(descriptor: &[u8]) -> Vec<(u8, u32)> {
        let mut items = Vec::new();
        let mut at = 0;
        while at < descriptor.len() {
            let prefix = descriptor[at];
            let size = match prefix & 3 {
                3 => 4,
                size => usize::from(size),
            };
            let mut data = [0u8; 4];
            data[..size].copy_from_slice(&descriptor[at + 1..at + 1 + size]);
            items.push((prefix & 0xfc, u32::from_le_bytes(data)));
            at += 1 + size;
        }
        items
    }

    /// Each report a descriptor declares: its main item's kind, its
    /// number, and its length with its number, every field being a byte.
    fn reports(descriptor: &[u8]) -> Vec<(u8, u32, u32)> {
        let (mut id, mut count) = (0, 0);
        let mut found = Vec::new();
        for (tag, data) in items(descriptor) {
            match tag {
                0x84 => id = data,
                0x94 => count = data,
                0x80 | 0x90 | 0xb0 => found.push((tag, id, count + 1)),
                _ => {}
            }
        }
        found
    }

    #[test]
    fn the_descriptor_declares_each_report_at_the_length_it_is_sent() {
        let builder = FakeSteamController::builder();
        let descriptor = FakeSteamController::descriptor(&builder);
        assert_eq!(
            reports(&descriptor),
            [
                (INPUT, 0x42, 54),
                (INPUT, 0x45, 46),
                (INPUT, 0x43, 15),
                (INPUT, 0x79, 2),
                (OUTPUT, 0x80, 10),
                (OUTPUT, 0x81, 8),
                (FEATURE, 1, 64),
                (FEATURE, 2, 64),
            ]
        );
        let with_47 = FakeSteamController::descriptor(&builder.declaring_report_47());
        assert!(reports(&with_47).contains(&(INPUT, 0x47, 46)));
        // Each field is a byte of 0 to 255.
        let fields: Vec<(u8, u32)> = items(&descriptor)
            .into_iter()
            .filter(|(tag, _)| matches!(tag, 0x14 | 0x24 | 0x74))
            .collect();
        assert_eq!(fields, [(0x14, 0), (0x24, 255), (0x74, 8)]);
    }

    /// The fake can never type or point: its descriptor's one collection
    /// is Valve's, on the vendor's usage page, and no other page is named.
    #[test]
    fn the_descriptor_has_no_mouse_and_no_keyboard() {
        for builder in [
            FakeSteamController::builder(),
            FakeSteamController::builder().declaring_report_47(),
            FakeSteamController::builder().transport(SteamTransport::Puck { joined: false }),
        ] {
            let items = items(&FakeSteamController::descriptor(&builder));
            let pages: Vec<u32> = items
                .iter()
                .filter(|(tag, _)| *tag == 0x04)
                .map(|(_, page)| *page)
                .collect();
            assert_eq!(pages, [0xff00]);
            let collections = items.iter().filter(|(tag, _)| *tag == 0xa0).count();
            assert_eq!(collections, 1);
        }
    }

    #[test]
    fn a_state_report_is_laid_out_as_the_driver_reads_it() {
        let state = SteamState {
            buttons: 0,
            left_trigger: 0x0102,
            right_trigger: 0x0304,
            left_stick: [0x0506, -2],
            right_stick: [0x0708, 0x090a],
            left_pad: Trackpad {
                touched: true,
                x: 0x0b0c,
                y: 0x0d0e,
                pressure: 0x0f10,
            },
            right_pad: Trackpad {
                touched: false,
                x: 0x1112,
                y: 0x1314,
                pressure: 0x1516,
            },
            sensor_clock: 0x1718_191a,
            trackpad_clock: 0x1b1c,
            accel: [0x1d1e, 0x1f20, 0x2122],
            gyro: [0x2324, 0x2526, 0x2728],
            orientation: [0x292a, 0x2b2c, 0x2d2e, 0x2f30],
        }
        .with(SteamButton::South)
        .with(SteamButton::Menu)
        .with(SteamButton::LeftGripTouch);
        let full = state.report(StateReport::Full, 9);
        assert_eq!(full.len(), 54);
        assert_eq!(full[..2], [0x42, 9]);
        // South is bit 0 of the third byte, menu bit 6; the left
        // trackpad's touch is bit 1 of the sixth byte, the left grip's
        // touch bit 5.
        assert_eq!(full[2..6], [0x41, 0x00, 0x00, 0x22]);
        assert_eq!(full[6..10], [0x02, 0x01, 0x04, 0x03]);
        assert_eq!(full[10..14], [0x06, 0x05, 0xfe, 0xff]);
        assert_eq!(full[18..24], [0x0c, 0x0b, 0x0e, 0x0d, 0x10, 0x0f]);
        assert_eq!(full[24..30], [0x12, 0x11, 0x14, 0x13, 0x16, 0x15]);
        assert_eq!(full[30..34], [0x1a, 0x19, 0x18, 0x17]);
        assert_eq!(full[34..36], [0x1e, 0x1d]);
        assert_eq!(full[44..46], [0x28, 0x27]);
        assert_eq!(full[46..48], [0x2a, 0x29]);
        assert_eq!(full[52..54], [0x30, 0x2f]);

        let short = state.report(StateReport::WithoutOrientation, 9);
        assert_eq!(short[0], 0x45);
        assert_eq!(short[1..], full[1..46]);

        // The newer report: the trackpads' clock before them, and the
        // sensors' clock in ticks of 32 µs, wrapping at 16 bits:
        // 0x1718191a µs is 0x00b8c0c8 ticks.
        let newer = state.report(StateReport::WithShortClocks, 9);
        assert_eq!(newer.len(), 46);
        assert_eq!(newer[0], 0x47);
        assert_eq!(newer[1..18], full[1..18]);
        assert_eq!(newer[18..20], [0x1c, 0x1b]);
        assert_eq!(newer[20..32], full[18..30]);
        assert_eq!(newer[32..34], [0xc8, 0xc0]);
        assert_eq!(newer[34..46], full[34..46]);
    }

    /// The back buttons and touches are where SDL and the driver read
    /// them, by bit.
    #[test]
    fn each_button_has_its_own_bit() {
        assert_eq!(SteamButton::UpperRightBack.bit(), 1 << 7);
        assert_eq!(SteamButton::LowerRightBack.bit(), 1 << 8);
        assert_eq!(SteamButton::UpperLeftBack.bit(), 1 << 17);
        assert_eq!(SteamButton::LowerLeftBack.bit(), 1 << 18);
        assert_eq!(SteamButton::RightStickTouch.bit(), 0x0010_0000);
        assert_eq!(SteamButton::LeftStickTouch.bit(), 0x0100_0000);
        assert_eq!(SteamButton::RightGripTouch.bit(), 0x1000_0000);
        assert_eq!(SteamButton::LeftGripTouch.bit(), 0x2000_0000);
        assert_eq!(RIGHT_PAD_TOUCH, 0x0020_0000);
        assert_eq!(LEFT_PAD_TOUCH, 0x0200_0000);
        let all = [
            SteamButton::South,
            SteamButton::East,
            SteamButton::West,
            SteamButton::North,
            SteamButton::QuickAccess,
            SteamButton::RightStickClick,
            SteamButton::Menu,
            SteamButton::UpperRightBack,
            SteamButton::LowerRightBack,
            SteamButton::RightBumper,
            SteamButton::DpadDown,
            SteamButton::DpadRight,
            SteamButton::DpadLeft,
            SteamButton::DpadUp,
            SteamButton::View,
            SteamButton::LeftStickClick,
            SteamButton::Steam,
            SteamButton::UpperLeftBack,
            SteamButton::LowerLeftBack,
            SteamButton::LeftBumper,
            SteamButton::RightStickTouch,
            SteamButton::RightPadClick,
            SteamButton::RightTriggerClick,
            SteamButton::LeftStickTouch,
            SteamButton::LeftPadClick,
            SteamButton::LeftTriggerClick,
            SteamButton::RightGripTouch,
            SteamButton::LeftGripTouch,
        ];
        let bits = all.iter().fold(0u32, |bits, button| {
            assert_eq!(bits & button.bit(), 0, "{button:?}");
            bits | button.bit()
        });
        assert_eq!(bits | RIGHT_PAD_TOUCH | LEFT_PAD_TOUCH, 0x3fff_ffff);
        assert!(
            SteamState::default()
                .with(SteamButton::Steam)
                .holds(SteamButton::Steam)
        );
    }

    #[test]
    fn a_battery_report_is_fifteen_bytes() {
        let battery = SteamBattery {
            charge: ChargeState::Discharging,
            level: 77,
            battery_millivolts: 3600,
            system_millivolts: 3300,
            input_millivolts: 0,
            battery_milliamps: 120,
            input_milliamps: 0,
            temperature: 30,
        };
        let report = battery.report();
        assert_eq!(report.len(), 15);
        assert_eq!(report[..5], [0x43, 1, 77, 0x10, 0x0e]);
    }

    fn answers(product: u16, joined: bool) -> Answers {
        Answers {
            serial: "CATEST0000000A0001".into(),
            product,
            joined: Arc::new(AtomicBool::new(joined)),
        }
    }

    /// The serial is answered as the driver asks it: the command, the
    /// field's length, which string, then the string, its number first.
    #[test]
    fn the_serial_is_answered_as_the_driver_asks() {
        let asked = [1, 0xae, 0x16, 0x01];
        let reply = answers(0x1302, true).reply(1, Some(&asked)).unwrap();
        assert_eq!(reply.len(), 64);
        assert_eq!(reply[..4], [1, 0xae, 0x15, 0x01]);
        assert_eq!(&reply[4..22], b"CATEST0000000A0001");
        assert_eq!(reply[22], 0);
    }

    #[test]
    fn the_attributes_carry_the_connection_s_interval() {
        let reply = answers(0x1303, true).reply(1, Some(&[1, 0x83])).unwrap();
        assert_eq!(reply[..3], [1, 0x83, 25]);
        assert_eq!(reply[3..8], [1, 0x03, 0x13, 0, 0]);
        assert_eq!(reply[23..28], [11, 0xa0, 0x0f, 0, 0]);
    }

    #[test]
    fn a_puck_s_slot_answers_whether_a_pad_is_in_it() {
        let asked = [2, 0xb4];
        assert_eq!(
            answers(0x1304, true).reply(2, Some(&asked)).unwrap()[..4],
            [2, 0xb4, 1, 2]
        );
        assert_eq!(
            answers(0x1304, false).reply(2, Some(&asked)).unwrap()[..4],
            [2, 0xb4, 1, 1]
        );
    }

    /// Any other command is acknowledged by its echo; a GET with no
    /// command before it is refused.
    #[test]
    fn other_commands_are_echoed_and_a_bare_get_is_refused() {
        let settings = [1, 0x87, 0x06, 0x09, 0x00, 0x00, 0x47, 0x00, 0x00];
        let reply = answers(0x1302, true).reply(1, Some(&settings)).unwrap();
        assert_eq!(reply[..9], settings);
        assert_eq!(answers(0x1302, true).reply(1, None), None);
    }

    /// Lizard mode's mouse and keyboard, which only a virtual machine
    /// check's fake has, are reports 0x40 and 0x41 on the generic
    /// desktop's page.
    #[test]
    fn lizard_mode_s_mouse_and_keyboard_are_their_own_reports() {
        let mouse = items(LIZARD_MOUSE);
        let keyboard = items(LIZARD_KEYBOARD);
        assert_eq!(mouse[..2], [(0x04, 0x01), (0x08, 0x02)]);
        assert_eq!(keyboard[..2], [(0x04, 0x01), (0x08, 0x06)]);
        let ids = |items: &[(u8, u32)]| -> Vec<u32> {
            items
                .iter()
                .filter(|(tag, _)| *tag == 0x84)
                .map(|(_, id)| *id)
                .collect()
        };
        assert_eq!(ids(&mouse), [0x40]);
        assert_eq!(ids(&keyboard), [0x41]);
        // Each opens as many collections as it ends.
        for items in [mouse, keyboard] {
            let opened = items.iter().filter(|(tag, _)| *tag == 0xa0).count();
            let ended = items.iter().filter(|(tag, _)| *tag == 0xc0).count();
            assert_eq!(opened, ended);
        }
    }

    /// The serial is 13 characters for any process and fake, so that the
    /// battery's name, `steam-<serial>`, leaves its thermal zone a name
    /// of at most 19; and a number reads back from its digits.
    #[test]
    fn a_serial_is_short_enough_for_the_battery_s_name() {
        assert_eq!(base36(0, 2), "00");
        assert_eq!(base36(35, 2), "0Z");
        assert_eq!(base36(36, 2), "10");
        // The most a process number can be, 2^22.
        let pid = base36(1 << 22, 5);
        assert_eq!(u32::from_str_radix(&pid, 36), Ok(1 << 22));
        let serial = format!("CATEST{pid}{}", base36(1295, 2));
        assert_eq!(serial.len(), 13);
        assert!(format!("steam-{serial}").len() <= 19);
    }

    /// Only the kernel's word that `hid-steam` bound the fake's own HID
    /// device is taken for its binding: not another device's, not the
    /// driver's client device, not another driver's, not another action.
    #[test]
    fn binding_is_heard_only_from_the_fake_s_own_device() {
        let phys = "ctrlassist/test/steam/1/0";
        let message = |action: &str, driver: &str, phys: &str, group: &str| {
            format!(
                "{action}@/devices/virtual/misc/uhid/0003:28DE:1302.0001\0ACTION={action}\0DRIVER={driver}\0HID_PHYS={phys}\0MODALIAS=hid:b0003g{group}v000028DEp00001302\0"
            )
            .into_bytes()
        };
        assert!(says_bound(
            &message("bind", "hid-steam", phys, "0001"),
            phys
        ));
        assert!(!says_bound(
            &message("add", "hid-steam", phys, "0001"),
            phys
        ));
        assert!(!says_bound(
            &message("bind", "hid-generic", phys, "0001"),
            phys
        ));
        assert!(!says_bound(
            &message("bind", "hid-steam", "elsewhere", "0001"),
            phys
        ));
        assert!(!says_bound(
            &message("bind", "hid-steam", phys, "0103"),
            phys
        ));
        // A prefix of the fake's place isn't its place.
        assert!(!says_bound(
            &message("bind", "hid-steam", "ctrlassist/test/steam/1/01", "0001"),
            phys
        ));
    }

    /// A descriptor item's size code is its data's length, for the one
    /// and two bytes the fake's items have.
    #[test]
    fn an_item_s_size_code_is_its_length() {
        assert_eq!(item1(0x84, 0x42), [0x85, 0x42]);
        assert_eq!(item2(0x04, 0xff00), [0x06, 0x00, 0xff]);
    }
}
