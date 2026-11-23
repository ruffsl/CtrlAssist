//! A pad, read from an open descriptor on its event node.

use crate::dualsense::RawOutput;
use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use ctrlassist_core::{Level, PadState};
use evdev::{
    Device, EventType, FFEffect, FFEffectCode, FFEffectData, FFEffectKind, FFReplay, FFTrigger,
};
use rustix::fs::fstat;

use crate::database::Database;
use crate::layout::{AxisRange, Mapping};

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

/// A pad being read.
///
/// Reading it is level-based, like the graph it feeds: [`read`](Self::read)
/// reports the pad's whole state after each report frame that changed it,
/// not the events that made the change.
pub struct Pad {
    device: Device,
    mapping: Mapping,
    identifiers: Identifiers,
    /// The state as of the last complete frame.
    state: PadState,
    /// The state with the events of a frame still arriving applied.
    pending: PadState,
    can_rumble: bool,
    /// The one effect kept on the pad while it rumbles, and its level.
    rumble: Option<(FFEffect, Level)>,
    /// The pad's raw node, for a pad that is sent whole output reports.
    raw: Option<RawOutput>,
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
        let device = Device::from_fd(fd)?;
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
        Ok(Self {
            device,
            mapping,
            identifiers,
            state,
            pending: state,
            can_rumble,
            rumble: None,
            raw: None,
        })
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
        self.device.grab()
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
    /// # Errors
    ///
    /// [`PadError::Gone`] when the pad has gone; the state then stays as
    /// it last was, and what that should mean is the caller's decision.
    pub fn read(&mut self, mut frame: impl FnMut(&PadState)) -> Result<(), PadError> {
        loop {
            let events = match self.device.fetch_events() {
                Ok(events) => events,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
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
    /// Whether the pad has rumble motors the kernel can drive.
    #[must_use]
    pub fn can_rumble(&self) -> bool {
        self.can_rumble
    }

    /// Gives the pad its raw HID node, where it has one this is able to
    /// write to: a DualSense's or a DualShock 4's. From then on the pad's rumble is
    /// written there, and so is what [`pass`](Self::pass) is given.
    ///
    /// # Errors
    ///
    /// Fails, leaving the pad as it was, if the node isn't such a pad's
    /// or can't be read.
    pub fn set_raw(&mut self, raw: OwnedFd) -> io::Result<()> {
        let raw = RawOutput::open(raw)?;
        // The kernel's own effect, if one runs, ends here.
        self.rumble = None;
        self.raw = Some(raw);
        Ok(())
    }

    /// Whether the pad is one whose raw node this knows how to write to:
    /// a DualSense or a DualShock 4, on USB or Bluetooth.
    #[must_use]
    pub fn could_take_output(&self) -> bool {
        let ids = &self.identifiers;
        RawOutput::takes(ids.bus, ids.vendor, ids.product)
    }

    /// Whether the pad is sent whole output reports.
    #[must_use]
    pub fn takes_output(&self) -> bool {
        self.raw.is_some()
    }

    /// Sends the pad an output report a game sent a virtual pad of its
    /// own kind, keeping the rumble the pad is at. A pad that isn't sent
    /// whole output reports ignores this.
    ///
    /// # Errors
    ///
    /// Fails if the pad's raw node refuses the write.
    pub fn pass(&mut self, report: &[u8]) -> Result<(), PadError> {
        match &mut self.raw {
            Some(raw) => Ok(raw.pass(report)?),
            None => Ok(()),
        }
    }

    /// Makes the pad's motors run at `level` until told otherwise; a
    /// level of zero stops them. A pad that can't rumble ignores this.
    ///
    /// The pad holds one effect however many games are rumbling it: the
    /// new level starts before the old effect is removed, so there is no
    /// gap between them.
    ///
    /// # Errors
    ///
    /// Fails if the pad has gone, the descriptor isn't open for writing,
    /// or the pad refuses the effect; the motors then stay as they were.
    pub fn rumble(&mut self, level: Level) -> Result<(), PadError> {
        if let Some(raw) = &mut self.raw {
            return Ok(raw.rumble(level)?);
        }
        if !self.can_rumble || self.rumble.as_ref().is_some_and(|(_, held)| *held == level) {
            return Ok(());
        }
        if level == Level::default() {
            // Dropping the effect stops and removes it.
            self.rumble = None;
            return Ok(());
        }
        let mut effect = self.device.upload_ff_effect(FFEffectData {
            direction: 0,
            trigger: FFTrigger::default(),
            // No length: it runs until it is replaced or removed. When
            // each rumble ends is the router's business.
            replay: FFReplay {
                length: 0,
                delay: 0,
            },
            kind: FFEffectKind::Rumble {
                strong_magnitude: level.strong,
                weak_magnitude: level.weak,
            },
        })?;
        effect.play(1)?;
        self.rumble = Some((effect, level));
        Ok(())
    }
}

impl AsFd for Pad {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.device.as_fd()
    }
}
