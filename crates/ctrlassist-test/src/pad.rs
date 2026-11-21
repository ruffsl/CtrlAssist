//! A fake pad at the evdev level.

use std::io;
use std::path::{Path, PathBuf};

use evdev::uinput::VirtualDevice;
use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode,
    UinputAbsSetup,
};

/// A gamepad made through uinput: the standard buttons and two sticks.
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
        let mut device = builder.build()?;
        let event_node = device
            .enumerate_dev_nodes_blocking()?
            .filter_map(Result::ok)
            .find(|node| node.to_string_lossy().contains("/event"))
            .ok_or_else(|| io::Error::other("the fake pad has no event node"))?;
        Ok(Self { device, event_node })
    }

    /// The pad's event node.
    #[must_use]
    pub fn event_node(&self) -> &Path {
        &self.event_node
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
