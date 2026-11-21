//! A fake pad at the evdev level.

use std::io;
use std::path::{Path, PathBuf};

use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode,
    UinputAbsSetup,
};

/// A gamepad made through uinput: the standard buttons, two sticks, two
/// triggers and a hat.
///
/// It has no HID device behind it, so no hidraw node and no driver to
/// re-bind. Dropping it removes it, as unplugging a pad would.
pub struct FakePad {
    device: VirtualDevice,
    event_node: PathBuf,
}

impl std::fmt::Debug for FakePad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakePad")
            .field("event_node", &self.event_node)
            .finish_non_exhaustive()
    }
}

impl FakePad {
    /// Creates the pad and waits for its event node.
    ///
    /// # Errors
    ///
    /// Fails if uinput can't be opened or the kernel refuses the pad.
    pub fn new(name: &str, vendor: u16, product: u16) -> io::Result<Self> {
        let keys = AttributeSet::from_iter([
            KeyCode::BTN_SOUTH,
            KeyCode::BTN_EAST,
            KeyCode::BTN_NORTH,
            KeyCode::BTN_WEST,
            KeyCode::BTN_TL,
            KeyCode::BTN_TR,
            KeyCode::BTN_SELECT,
            KeyCode::BTN_START,
            KeyCode::BTN_MODE,
            KeyCode::BTN_THUMBL,
            KeyCode::BTN_THUMBR,
        ]);
        let stick = AbsInfo::new(0, -32768, 32767, 0, 0, 0);
        let mut builder = VirtualDevice::builder()?
            .name(name)
            .input_id(InputId::new(BusType::BUS_USB, vendor, product, 1))
            .with_keys(&keys)?;
        for axis in [
            AbsoluteAxisCode::ABS_X,
            AbsoluteAxisCode::ABS_Y,
            AbsoluteAxisCode::ABS_RX,
            AbsoluteAxisCode::ABS_RY,
        ] {
            builder = builder.with_absolute_axis(&UinputAbsSetup::new(axis, stick))?;
        }
        let trigger = AbsInfo::new(0, 0, 255, 0, 0, 0);
        for axis in [AbsoluteAxisCode::ABS_Z, AbsoluteAxisCode::ABS_RZ] {
            builder = builder.with_absolute_axis(&UinputAbsSetup::new(axis, trigger))?;
        }
        let hat = AbsInfo::new(0, -1, 1, 0, 0, 0);
        for axis in [AbsoluteAxisCode::ABS_HAT0X, AbsoluteAxisCode::ABS_HAT0Y] {
            builder = builder.with_absolute_axis(&UinputAbsSetup::new(axis, hat))?;
        }
        Self::built(builder.build()?)
    }

    /// Creates a pad with exactly these key codes and absolute axes
    /// (each a code, its lowest and its highest value), for standing in
    /// for a pad that doesn't follow the gamepad layout.
    ///
    /// # Errors
    ///
    /// Fails if uinput can't be opened or the kernel refuses the pad.
    pub fn with_controls(
        name: &str,
        (vendor, product, version): (u16, u16, u16),
        keys: &[u16],
        axes: &[(u16, i32, i32)],
    ) -> io::Result<Self> {
        let keys = AttributeSet::from_iter(keys.iter().map(|code| KeyCode::new(*code)));
        let mut builder = VirtualDevice::builder()?
            .name(name)
            .input_id(InputId::new(BusType::BUS_USB, vendor, product, version))
            .with_keys(&keys)?;
        for (code, min, max) in axes {
            let info = AbsInfo::new((min + max + 1).div_euclid(2), *min, *max, 0, 0, 0);
            builder =
                builder.with_absolute_axis(&UinputAbsSetup::new(AbsoluteAxisCode(*code), info))?;
        }
        Self::built(builder.build()?)
    }

    fn built(mut device: VirtualDevice) -> io::Result<Self> {
        let event_node = device
            .enumerate_dev_nodes_blocking()?
            .filter_map(Result::ok)
            .find(|node| node.to_string_lossy().contains("/event"))
            .ok_or_else(|| io::Error::other("the fake pad has no event node"))?;
        // The node exists before udev has set who may open it. Wait for
        // udev to finish with it, so the caller can open it at once.
        let _ = std::process::Command::new("udevadm").arg("settle").status();
        Ok(Self { device, event_node })
    }

    /// The pad's event node.
    #[must_use]
    pub fn event_node(&self) -> &Path {
        &self.event_node
    }

    /// Sends events as one report frame, each an event type, a code and
    /// a value as the kernel numbers them.
    ///
    /// # Errors
    ///
    /// Fails if the kernel refuses the write.
    pub fn frame(&mut self, events: &[(u16, u16, i32)]) -> io::Result<()> {
        let events: Vec<InputEvent> = events
            .iter()
            .map(|(kind, code, value)| InputEvent::new(*kind, *code, *value))
            .collect();
        self.device.emit(&events)
    }

    /// Presses or releases the south face button, as one report frame.
    ///
    /// # Errors
    ///
    /// Fails if the kernel refuses the write.
    pub fn south(&mut self, pressed: bool) -> io::Result<()> {
        self.device.emit(&[InputEvent::new(
            EventType::KEY.0,
            KeyCode::BTN_SOUTH.0,
            i32::from(pressed),
        )])
    }

    /// Moves the left stick's X axis, as one report frame.
    ///
    /// # Errors
    ///
    /// Fails if the kernel refuses the write.
    pub fn left_x(&mut self, value: i16) -> io::Result<()> {
        self.device.emit(&[InputEvent::new(
            EventType::ABSOLUTE.0,
            AbsoluteAxisCode::ABS_X.0,
            i32::from(value),
        )])
    }
}
