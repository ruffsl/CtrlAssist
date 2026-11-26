//! A pad, read from an open descriptor on its event node, or, for a
//! Steam Controller, from its raw node.

use crate::dualsense::Kind;
use crate::output::{Output, Rested, endless};
use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use ctrlassist_core::{Level, Motion, PadState, Touchpad, gamepad};
use evdev::{Device, EventType, FFEffectCode};
use rustix::fs::fstat;

use crate::database::Database;
use crate::layout::{AxisRange, Mapping};
use crate::{descriptor, hidraw, motion, steam};

/// Why a pad couldn't be opened or read.
#[derive(Debug)]
#[non_exhaustive]
pub enum PadError {
    /// The pad has gone: unplugged, out of range, or its driver re-bound.
    Gone,
    /// The descriptor isn't an event device, or the kernel refused a read.
    Io(io::Error),
}

impl std::fmt::Display for PadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Gone => f.write_str("the pad has gone"),
            Self::Io(error) => write!(f, "the pad couldn't be read: {error}"),
        }
    }
}

impl std::error::Error for PadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Gone => None,
            Self::Io(error) => Some(error),
        }
    }
}

impl From<io::Error> for PadError {
    fn from(error: io::Error) -> Self {
        // The kernel answers a read on a removed device with ENODEV.
        if error.raw_os_error() == Some(rustix::io::Errno::NODEV.raw_os_error()) {
            Self::Gone
        } else {
            Self::Io(error)
        }
    }
}

/// What tells a pad from others, as the kernel and udev report it. Every
/// string is the pad's own claim and is not to be trusted as more.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Identifiers {
    /// The pad's name.
    pub name: String,
    /// The bus it is on, as the kernel numbers them: 3 is USB, 5 Bluetooth.
    pub bus: u16,
    /// Its vendor number.
    pub vendor: u16,
    /// Its product number.
    pub product: u16,
    /// Its version number.
    pub version: u16,
    /// Its own address, the kernel's `uniq`: the same over USB and
    /// Bluetooth for a PlayStation pad, absent for an Xbox pad on USB.
    pub address: Option<String>,
    /// Its USB serial number, from udev.
    pub serial: Option<String>,
    /// Where it is plugged in, the kernel's `phys`.
    pub port: Option<String>,
}

/// The one name a pad is known by across reconnects: the most specific
/// thing it reports that stays the same when it is unplugged and
/// plugged in again.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
#[non_exhaustive]
pub enum PadId {
    /// The pad's own address: the same on any port and any transport.
    Address(String),
    /// A model's vendor and product, and this pad's USB serial number.
    Serial(u16, u16, String),
    /// A model's vendor and product, and the port it is plugged into: for
    /// a pad that reports nothing of its own, it is that pad only while
    /// it stays in that port.
    Port(u16, u16, String),
    /// Nothing but a model: any pad of that vendor, product and name.
    Model(u16, u16, String),
}

impl std::fmt::Display for PadId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Address(address) => write!(f, "address:{address}"),
            Self::Serial(vendor, product, serial) => {
                write!(f, "serial:{vendor:04x}:{product:04x}:{serial}")
            }
            Self::Port(vendor, product, port) => {
                write!(f, "port:{vendor:04x}:{product:04x}:{port}")
            }
            Self::Model(vendor, product, name) => {
                write!(f, "model:{vendor:04x}:{product:04x}:{name}")
            }
        }
    }
}

impl Identifiers {
    /// The name this pad is known by across reconnects.
    #[must_use]
    pub fn id(&self) -> PadId {
        if let Some(address) = &self.address {
            PadId::Address(address.clone())
        } else if let Some(serial) = &self.serial {
            PadId::Serial(self.vendor, self.product, serial.clone())
        } else if let Some(port) = self.port.as_ref().filter(|_| !self.is_test_pad()) {
            // A fake pad's `phys` is its marker, not where it is plugged.
            PadId::Port(self.vendor, self.product, port.clone())
        } else {
            PadId::Model(self.vendor, self.product, self.name.clone())
        }
    }

    /// Whether this is one of the daemon's own virtual pads, made by
    /// this process or any other, which must never be read as a source:
    /// that would feed the daemon its own output.
    #[must_use]
    pub fn is_own_output(&self) -> bool {
        let marked = |text: &Option<String>, marker: &str| {
            text.as_deref().is_some_and(|text| text.starts_with(marker))
        };
        // A uinput pad by its `phys`. The kernel's PlayStation driver
        // gives its pads none, so a uhid pad by its address.
        marked(&self.port, crate::OWN_MARKER) || marked(&self.address, crate::OWN_ADDRESS)
    }

    /// Whether this is a fake pad made by the tests.
    #[must_use]
    pub fn is_test_pad(&self) -> bool {
        let marked = |text: &Option<String>, marker: &str| {
            text.as_deref().is_some_and(|text| text.starts_with(marker))
        };
        marked(&self.port, crate::TEST_MARKER) || marked(&self.address, crate::TEST_ADDRESS)
    }

    /// Whether this is a Steam Controller, the 2026 one: on a cable, over
    /// Bluetooth, or in a slot of a Puck or a Steam Machine's receiver.
    /// Such a pad is read from its raw node and nothing else, and its
    /// event node goes while that is open: see [`Pad::set_raw`].
    #[must_use]
    pub fn is_steam_controller(&self) -> bool {
        steam::is_pad(self.bus, self.vendor, self.product)
    }
}

fn nonempty(text: Option<&str>) -> Option<String> {
    text.map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// Kernel drivers that present the gamepad layout themselves: the Xbox
/// and PlayStation families, and the others written to the kernel's
/// gamepad convention. A database line for one of their pads describes
/// some other driver's view of it, and is not used.
const GAMEPAD_DRIVERS: [&str; 9] = [
    "xpad",
    "xpadneo",
    "xone-gip-gamepad",
    "microsoft",
    "playstation",
    "sony",
    "nintendo",
    "hid-steam",
    "steam-deck",
];

/// Why a pad is sent no rumble.
#[derive(Debug)]
enum NoEffect {
    /// It has no motors the kernel drives, or is sent whole output
    /// reports, which carry its rumble.
    NotWanted,
    /// The pad refused the upload, with this error number. A descriptor
    /// open only for reading can upload but never play, and is taken as
    /// refused with `EBADF`.
    Refused(i32),
}

/// The kernel driver behind a device: the nearest one up its parents.
fn driver(device_number: u64) -> Option<String> {
    let device = udev::Device::from_devnum(udev::DeviceType::Character, device_number).ok()?;
    std::iter::successors(Some(device), udev::Device::parent)
        .find_map(|device| Some(device.driver()?.to_str()?.to_string()))
}

/// The USB serial number udev holds for a device.
fn serial(device_number: u64) -> Option<String> {
    let device = udev::Device::from_devnum(udev::DeviceType::Character, device_number).ok()?;
    nonempty(device.property_value("ID_SERIAL_SHORT")?.to_str())
}

/// A pad's raw node, read on the input thread.
enum Raw {
    /// A DualSense's or a DualShock 4's, read for its motion, touchpad
    /// and click beside its event node.
    Beside(motion::Reader),
    /// A Steam Controller's, read for everything: its event node has gone
    /// for as long as this is open.
    Whole(steam::Reader),
}

impl AsFd for Raw {
    fn as_fd(&self) -> BorrowedFd<'_> {
        match self {
            Self::Beside(reader) => reader.as_fd(),
            Self::Whole(reader) => reader.as_fd(),
        }
    }
}

/// A pad being read.
///
/// Reading it is level-based, like the graph it feeds: [`read`](Self::read)
/// reports the pad's whole state after each report frame that changed it,
/// not the events that made the change.
pub struct Pad {
    /// The pad's event node. Taken only as the pad is dropped, to go
    /// with the thread that writes its effect. A Steam Controller's is
    /// kept but never read once the pad is read from its raw node, which
    /// takes it away.
    device: Option<Device>,
    mapping: Mapping,
    identifiers: Identifiers,
    /// The state as of the last complete frame.
    state: PadState,
    /// The state with the events of a frame still arriving applied.
    pending: PadState,
    can_rumble: bool,
    /// What writes to the pad, off the input thread: the one effect kept
    /// on it, uploaded as it is opened, or its raw node, for a pad that
    /// is sent whole output reports or is a Steam Controller; or why
    /// nothing does.
    output: Result<Output, NoEffect>,
    /// The pad's raw node, read for its motion, touchpad and click, or
    /// for everything.
    raw: Option<Raw>,
    /// That node, no longer read: kept to be closed as the pad is
    /// dropped, so that the input thread closes nothing.
    unread: Option<Raw>,
}

impl std::fmt::Debug for Pad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pad")
            .field("identifiers", &self.identifiers)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl Pad {
    /// Takes over a descriptor open on a pad's event node, and reads what
    /// the pad holds now. The mapping is the kernel's gamepad layout.
    ///
    /// # Errors
    ///
    /// Fails if the descriptor isn't an event device.
    pub fn from_fd(fd: OwnedFd) -> Result<Self, PadError> {
        Self::open(fd, None)
    }

    /// As [`from_fd`](Self::from_fd), with layouts from a controller
    /// database for pads whose drivers don't present the gamepad layout.
    ///
    /// # Errors
    ///
    /// Fails if the descriptor isn't an event device.
    pub fn with_database(fd: OwnedFd, database: &Database) -> Result<Self, PadError> {
        Self::open(fd, Some(database))
    }

    fn open(fd: OwnedFd, database: Option<&Database>) -> Result<Self, PadError> {
        let device_number = fstat(&fd).map_err(io::Error::from)?.st_rdev;
        let serial = serial(device_number);
        // Open for more than reading: the access mode isn't read-only.
        let writable = rustix::fs::fcntl_getfl(&fd)
            .is_ok_and(|flags| flags & rustix::fs::OFlags::RWMODE != rustix::fs::OFlags::RDONLY);
        let mut device = Device::from_fd(fd)?;
        // A read with nothing waiting must return, not block the thread
        // that every other pad is read on.
        device.set_nonblocking(true)?;

        let id = device.input_id();
        let identifiers = Identifiers {
            name: device.name().unwrap_or_default().to_string(),
            bus: id.bus_type().0,
            vendor: id.vendor(),
            product: id.product(),
            version: id.version(),
            address: nonempty(device.unique_name()),
            serial,
            port: nonempty(device.physical_path()),
        };

        let keys: Vec<u16> = device
            .supported_keys()
            .map(|keys| keys.iter().map(evdev::KeyCode::code).collect())
            .unwrap_or_default();
        let now = device.get_abs_state()?;
        let absolutes: Vec<(u16, AxisRange)> = device
            .supported_absolute_axes()
            .map(|axes| {
                axes.iter()
                    .filter_map(|axis| {
                        let info = now.get(usize::from(axis.0))?;
                        let range = AxisRange {
                            min: info.minimum,
                            max: info.maximum,
                        };
                        Some((axis.0, range))
                    })
                    .collect()
            })
            .unwrap_or_default();
        // A pad whose driver presents the gamepad layout is read as the
        // driver presents it. Any other pad takes its layout from the
        // database if it is there, and is read as a gamepad if it isn't.
        let mapping = database
            .filter(|_| {
                !driver(device_number)
                    .is_some_and(|driver| GAMEPAD_DRIVERS.contains(&driver.as_str()))
            })
            .and_then(|database| database.mapping(&identifiers, &keys, &absolutes))
            .unwrap_or_else(|| Mapping::kernel(&keys, &absolutes));

        // What is held at this moment: a pad can be opened mid-press.
        let mut state = PadState::NEUTRAL;
        for (code, _) in &absolutes {
            if let Some(info) = now.get(usize::from(*code)) {
                mapping.apply(&mut state, EventType::ABSOLUTE.0, *code, info.value);
            }
        }
        for key in device.get_key_state()?.iter() {
            mapping.apply(&mut state, EventType::KEY.0, key.code(), 1);
        }

        let can_rumble = device
            .supported_ff()
            .is_some_and(|ff| ff.contains(FFEffectCode::FF_RUMBLE));
        // The one effect the pad will hold, uploaded here and not on the
        // input thread: an upload duplicates the descriptor, and a pad
        // another program makes waits for that program to answer it. A
        // thread of the pad's own updates it from then on, for the same
        // reason.
        let output = if !can_rumble {
            Err(NoEffect::NotWanted)
        } else if !writable {
            Err(NoEffect::Refused(rustix::io::Errno::BADF.raw_os_error()))
        } else {
            device
                .upload_ff_effect(endless(Level::default()))
                .and_then(Output::effect)
                .map_err(|error| {
                    NoEffect::Refused(
                        error
                            .raw_os_error()
                            .unwrap_or(rustix::io::Errno::IO.raw_os_error()),
                    )
                })
        };
        Ok(Self {
            device: Some(device),
            mapping,
            identifiers,
            state,
            pending: state,
            can_rumble,
            output,
            raw: None,
            unread: None,
        })
    }

    /// Whether the pad is read from its raw node and nothing else: a
    /// Steam Controller given its raw node.
    fn is_wholly_raw(&self) -> bool {
        matches!(self.raw, Some(Raw::Whole(_))) || matches!(self.unread, Some(Raw::Whole(_)))
    }

    /// What tells this pad from others.
    #[must_use]
    pub fn identifiers(&self) -> &Identifiers {
        &self.identifiers
    }

    /// Takes the pad's events for this descriptor alone: every other
    /// reader of its event node, and of the joystick node beside it,
    /// hears nothing until this pad is dropped. The kernel lifts the grab
    /// if the process dies. Programs that read the pad some other way, as
    /// Steam reads a PlayStation pad through hidraw, still see it.
    ///
    /// # Errors
    ///
    /// Fails if the pad has gone, or another descriptor holds the grab.
    pub fn grab(&mut self) -> io::Result<()> {
        match &mut self.device {
            Some(device) => device.grab(),
            None => Err(io::Error::from(rustix::io::Errno::NODEV)),
        }
    }

    /// Whether this is one of the daemon's own virtual pads; see
    /// [`Identifiers::is_own_output`].
    #[must_use]
    pub fn is_own_output(&self) -> bool {
        self.identifiers.is_own_output()
    }

    /// What the pad held as of its last complete report frame.
    #[must_use]
    pub fn state(&self) -> &PadState {
        &self.state
    }

    /// Reads everything waiting, and calls `frame` with the pad's state
    /// after each report frame that changed it. Returns when nothing is
    /// left to read.
    ///
    /// When the kernel has dropped events because they weren't read in
    /// time, the pad's true state is fetched again, so a release that
    /// was dropped can't leave a button held.
    ///
    /// A pad read from its raw node alone, a Steam Controller given its
    /// raw node, has nothing to read here: its event node has gone, and
    /// [`set_raw`](Self::set_raw) says how it is read.
    ///
    /// # Errors
    ///
    /// [`PadError::Gone`] when the pad has gone; the state then stays as
    /// it last was, and what that should mean is the caller's decision.
    pub fn read(&mut self, mut frame: impl FnMut(&PadState)) -> Result<(), PadError> {
        if self.is_wholly_raw() {
            return Ok(());
        }
        let Some(device) = &mut self.device else {
            return Err(PadError::Gone);
        };
        loop {
            let events = match device.fetch_events() {
                Ok(events) => events,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                // A signal cut the read short, or a resync's fetch, which
                // waits on the node's lock while a writer of the pad's
                // holds it: the pad is still there, the resync still owed,
                // and nothing of a half fetch taken in. Read again, for as
                // long as the signal keeps coming and no longer.
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            };
            for event in events {
                let kind = event.event_type();
                if kind == EventType::SYNCHRONIZATION {
                    // The end of a frame, or of the made-up events that
                    // stand in for dropped ones.
                    if self.pending != self.state {
                        self.state = self.pending;
                        frame(&self.state);
                    }
                } else {
                    self.mapping
                        .apply(&mut self.pending, kind.0, event.code(), event.value());
                }
            }
        }
    }
}

impl Pad {
    /// Whether the pad has rumble motors the kernel can drive, or is a
    /// Steam Controller given its raw node, which this drives itself.
    #[must_use]
    pub fn can_rumble(&self) -> bool {
        self.can_rumble
    }

    /// Gives the pad its raw HID node, where it has one this is able to
    /// write to: a DualSense's or a DualShock 4's, or a Steam
    /// Controller's. From then on the pad's rumble is written there, by a
    /// thread of the pad's own: a write to a pad on USB waits for the pad.
    ///
    /// For a DualSense or a DualShock 4, so is what [`pass`](Self::pass)
    /// is given, and dropping the pad has that thread write what is left,
    /// leave the pad at rest with its driver's lights, and end. The raw
    /// node is read as well, on a duplicate of the descriptor, for its
    /// motion sensors, its touchpad and its click, which its driver
    /// reports on nodes of their own: the input thread listens to it
    /// beside the pad's event node. Its calibration is read here, the
    /// request waiting on the pad.
    ///
    /// A Steam Controller is read from its raw node and nothing else,
    /// every control, both trackpads and its motion (see `steam.rs`):
    /// opening the raw node has the kernel's driver take the pad's
    /// gamepad and motion devices away for as long as it is open. So the
    /// pad is opened on its event node first, which says what it is, and
    /// given this after; the event node it holds is never read or
    /// listened to again, its `ENODEV` being no sign of the pad going, and
    /// is closed as the pad is dropped. The node must be the pad's own,
    /// whose report descriptor declares its state report: the driver
    /// starts the HID device's other interfaces too, with the same numbers
    /// and raw nodes of their own on which the pad says nothing. The pad's
    /// state is at rest until its first report: the reports the node kept
    /// before now are read away here, and again as the input thread first
    /// listens to it, so that none from before the setup runs is played.
    /// Its thread sets lizard mode off, with the motion sensors
    /// on, once as it starts, and sends its rumble again while it runs; dropping the pad has it stop the
    /// motors, and the raw node closed then gives the pad back to its
    /// driver, which puts it back in lizard mode. It is sent nothing of
    /// what [`pass`](Self::pass) is given: a virtual DualSense's lights
    /// and trigger effects are nothing this pad has. Its haptics beyond
    /// plain rumble are not sent at all (see `output.rs`'s `SteamNode`).
    /// The pad's going is its raw node failing or hanging up, or, in a
    /// receiver's slot, the slot saying its pad left.
    ///
    /// # Errors
    ///
    /// Fails, leaving the pad as it was, if the node isn't such a pad's,
    /// can't be read or duplicated, or the thread can't be started.
    ///
    /// For a Steam Controller "as it was" is a pad whose event node may
    /// be gone: the caller opened the raw node, which had the driver take
    /// the event node away, or will. So a failure means the pad must be
    /// dropped and found again, its event node coming back once the raw
    /// node is closed; with one exception, a node that isn't the pad's own,
    /// refused with [`io::ErrorKind::InvalidInput`], whose opening took
    /// nothing away. A pad that failed so and is run all the same is safe:
    /// its event node, listened to, reads `ENODEV` and the pad is reported
    /// gone; if the driver hadn't yet taken it away, it reads as the
    /// driver's gamepad presents it, silent in lizard mode.
    pub fn set_raw(&mut self, raw: OwnedFd) -> io::Result<()> {
        let (bus, vendor, product) = hidraw::info(raw.as_fd())?;
        if steam::is_pad(bus, vendor, product) {
            if !descriptor::is_steam_controller(&hidraw::descriptor(raw.as_fd())?) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "a raw node of the Steam Controller's HID device that isn't the pad's own: its descriptor declares none of the pad's state reports",
                ));
            }
            let reader = steam::Reader::new(raw.as_fd())?;
            self.output = Ok(Output::steam(raw)?);
            self.raw = Some(Raw::Whole(reader));
            self.can_rumble = true;
            self.state = PadState::NEUTRAL;
            self.pending = PadState::NEUTRAL;
            return Ok(());
        }
        let kind = Kind::of(raw.as_fd())?;
        let make = match kind {
            Kind::DualSense { .. } => motion::Make::DualSense,
            Kind::DualShock4 { bluetooth } => motion::Make::DualShock4 { bluetooth },
        };
        let reader = motion::Reader::new(raw.as_fd(), make)?;
        // The kernel's own effect, if one runs, ends here, stopped and
        // erased by the thread that wrote it.
        self.output = Ok(Output::raw(raw, kind)?);
        self.raw = Some(Raw::Beside(reader));
        Ok(())
    }

    /// The pad's event node, while it is read: none for a pad read from
    /// its raw node alone, whose event node has gone.
    pub(crate) fn event_node(&self) -> Option<BorrowedFd<'_>> {
        if self.is_wholly_raw() {
            return None;
        }
        self.device.as_ref().map(AsFd::as_fd)
    }

    /// The pad's raw node, while it is read.
    pub(crate) fn raw_node(&self) -> Option<BorrowedFd<'_>> {
        self.raw.as_ref().map(AsFd::as_fd)
    }

    /// Readies the pad to be listened to on the input thread: a Steam
    /// Controller's raw node has what it kept since the pad was given it
    /// read away, the first time. Waits for nothing and allocates
    /// nothing.
    pub(crate) fn begin(&mut self) {
        if let Some(Raw::Whole(reader)) = &mut self.raw {
            reader.begin();
        }
    }

    /// Reads every report waiting on the raw node, and calls `frame` with
    /// the pad's state after each input report. A DualSense's or a
    /// DualShock 4's is its motion, touchpad and click as the report says,
    /// and its other controls as of the event node's last frame; a Steam
    /// Controller's is all the report's. Returns when nothing is left to
    /// read; a pad whose raw node isn't read has nothing to.
    ///
    /// # Errors
    ///
    /// [`PadError::Gone`] when the node has gone from under the pad, or,
    /// for a Steam Controller, whose raw node is all there is of it, when
    /// it can't be read or its receiver's slot says it left;
    /// [`PadError::Io`] when a node read beside the event node can't be
    /// read, which [`rest_motion`](Self::rest_motion) is for.
    pub(crate) fn read_raw(&mut self, mut frame: impl FnMut(&PadState)) -> Result<(), PadError> {
        let Self {
            raw,
            state,
            pending,
            ..
        } = self;
        match raw {
            None => Ok(()),
            Some(Raw::Beside(reader)) => reader.read(|(motion, touchpad, click)| {
                // Into the frame still arriving on the event node too,
                // which would otherwise put back what it held.
                for state in [&mut *pending, &mut *state] {
                    state.set_motion(motion);
                    state.set_touchpad(touchpad);
                    state.set(gamepad::TOUCHPAD, f32::from(u8::from(click)));
                }
                frame(state);
            }),
            Some(Raw::Whole(reader)) => reader.read(|read| {
                *state = *read;
                *pending = *read;
                frame(state);
            }),
        }
    }

    /// Stops reading the raw node, and leaves what it was read for at rest
    /// for as long as the pad is held: for a raw node that couldn't be
    /// read. For a DualSense or a DualShock 4 that is its motion, touchpad
    /// and click; for a Steam Controller, everything. The node is kept,
    /// and closed as the pad is dropped, off the input thread.
    pub(crate) fn rest_motion(&mut self) {
        let whole = self.is_wholly_raw();
        if let Some(reader) = self.raw.take() {
            self.unread = Some(reader);
        }
        for state in [&mut self.pending, &mut self.state] {
            if whole {
                *state = PadState::NEUTRAL;
            } else {
                state.set_motion(Motion::STILL);
                state.set_touchpad(Touchpad::UNTOUCHED);
                state.set(gamepad::TOUCHPAD, 0.0);
            }
        }
    }

    /// Whether the pad is one whose raw node this knows how to write to:
    /// a DualSense or a DualShock 4, on USB or Bluetooth.
    #[must_use]
    pub fn could_take_output(&self) -> bool {
        let ids = &self.identifiers;
        Kind::takes(ids.bus, ids.vendor, ids.product)
    }

    /// Whether the pad is sent whole output reports, which carry what the
    /// games set on a virtual DualSense: a DualSense or a DualShock 4
    /// given its raw node, and not a Steam Controller.
    #[must_use]
    pub fn takes_output(&self) -> bool {
        self.output.as_ref().is_ok_and(Output::takes_reports)
    }

    /// Sends the pad an output report a game sent a virtual pad of its
    /// own kind, keeping the rumble the pad is at. A pad that isn't sent
    /// whole output reports ignores this. It is written without waiting
    /// for the pad, and takes the place of a report not yet written.
    ///
    /// # Errors
    ///
    /// Fails if the pad's raw node refused a write since that was last
    /// said.
    pub fn pass(&mut self, report: &[u8]) -> Result<(), PadError> {
        match &mut self.output {
            Ok(output) => Ok(output.pass(report)?),
            Err(_) => Ok(()),
        }
    }

    /// Keeps back what the pad is told, until
    /// [`release_output`](Self::release_output): a level and a report
    /// told together are then written once.
    pub(crate) fn hold_output(&mut self) {
        if let Ok(output) = &mut self.output {
            output.hold();
        }
    }

    /// Has what was kept back from the pad written.
    pub(crate) fn release_output(&mut self) {
        if let Ok(output) = &mut self.output {
            output.release();
        }
    }

    /// Makes the pad's motors run at `level` until told otherwise; a
    /// level of zero stops them. A pad that can't rumble ignores this.
    ///
    /// The pad holds one effect however many games are rumbling it,
    /// uploaded as the pad is opened and kept until it is dropped: a new
    /// level updates it in place, with no gap and no second slot, and a
    /// level of zero stops it. A pad sent whole output reports is sent
    /// the level in them instead, and a Steam Controller given its raw
    /// node in its own rumble report. Either way the level is sent by a
    /// thread of the pad's own, and this never waits for it: a newer
    /// level takes the place of one not yet sent.
    ///
    /// # Errors
    ///
    /// Fails if the descriptor isn't open for writing or the pad refused
    /// the effect as it was opened, the motors then staying as they were;
    /// or if the pad refused a write since that was last said, as one
    /// that has gone does.
    pub fn rumble(&mut self, level: Level) -> Result<(), PadError> {
        let stop = level == Level::default();
        match &mut self.output {
            Ok(output) => output.rumble(level)?,
            Err(NoEffect::Refused(errno)) if !stop => {
                return Err(io::Error::from_raw_os_error(*errno).into());
            }
            Err(_) => {}
        }
        Ok(())
    }

    /// Says when the thread that writes to this pad has left it at rest,
    /// once the pad is dropped: for whoever is about to have the pad's
    /// driver re-bound, which would cut off a last report not yet
    /// written. Got before the drop, from the writer the pad has then:
    /// one replaced since by [`set_raw`](Self::set_raw) is said for as
    /// it stops. A pad that is sent nothing is done already. See
    /// [`Rested`] for what waiting covers; waiting is optional.
    #[must_use]
    pub fn rested(&self) -> Rested {
        match &self.output {
            Ok(output) => output.rested(),
            Err(_) => Rested::already(),
        }
    }

    /// Drops the pad, and says when its writer has left it at rest, as
    /// [`rested`](Self::rested) does.
    pub fn let_go(self) -> Rested {
        let rested = self.rested();
        drop(self);
        rested
    }
}

impl Drop for Pad {
    fn drop(&mut self) {
        // The raw node read is closed first, so that the writer's closing
        // its own, after the pad's last report, is the node's last: what
        // has a Steam Controller's driver take the pad back, and what
        // anyone waiting for the writer to be done waits for.
        drop(self.raw.take());
        drop(self.unread.take());
        // A pad whose effect is written goes to the thread that writes
        // it, and is closed there after the effect is stopped and erased:
        // closing the pad, and taking back a grab, wait on the pad's node
        // while that thread's update or erase holds it, which for a pad
        // another program makes lasts until that program answers. Whoever
        // drops the pad waits for none of that.
        if let (Ok(output), Some(device)) = (&mut self.output, self.device.take()) {
            if output.is_raw() {
                self.device = Some(device);
            } else {
                output.leave_after(Box::new(device));
            }
        }
    }
}

/// The pad's event node: for a Steam Controller given its raw node, one
/// that has gone.
impl AsFd for Pad {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.device
            .as_ref()
            .expect("the pad's device is taken only as the pad is dropped")
            .as_fd()
    }
}
