use evdev::{
    AbsInfo, AbsoluteAxisCode, AttributeSet, KeyCode, UinputAbsSetup, uinput::VirtualDevice,
};
use gilrs::{Axis, Button};
use std::error::Error;

// --- Scaling Helper Functions ---

pub const AXIS_MAX: f32 = u16::MAX as f32;
pub const AXIS_HALF: f32 = AXIS_MAX / 2.0;

/// Scales a value from -1.0..1.0 range to 0..AXIS_MAX
pub fn scale_stick(val: f32, invert: bool) -> i32 {
    let val = if invert { -val } else { val };
    ((val + 1.0) * AXIS_HALF).round() as i32
}

/// Scales a trigger value from 0.0..1.0 to 0..AXIS_MAX
pub fn scale_trigger(val: f32) -> i32 {
    (val * AXIS_MAX).round() as i32
}

/// Struct to represent a virtual gamepad's identity (real or spoofed)
pub struct VirtualGamepadInfo<'a> {
    pub name: &'a str,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
}

impl<'a> From<&'a gilrs::Gamepad<'a>> for VirtualGamepadInfo<'a> {
    fn from(gp: &'a gilrs::Gamepad<'a>) -> Self {
        Self {
            name: gp.os_name(),
            vendor_id: gp.vendor_id(),
            product_id: gp.product_id(),
        }
    }
}

// --- evdev Device Creation ---

/// Helper to create the virtual gamepad device
pub fn create_virtual_gamepad<'a>(
    info: &VirtualGamepadInfo<'a>,
) -> Result<VirtualDevice, Box<dyn Error>> {
    let max = AXIS_MAX as i32;
    let mid = AXIS_HALF as i32;
    let abs_stick_setup = AbsInfo::new(mid, 0, max, 0, 0, 0);
    let abs_trigger_setup = AbsInfo::new(0, 0, max, 0, 0, 0);

    let keys = AttributeSet::from_iter([
        KeyCode::BTN_NORTH,
        KeyCode::BTN_SOUTH,
        KeyCode::BTN_EAST,
        KeyCode::BTN_WEST,
        KeyCode::BTN_TL,  // L1
        KeyCode::BTN_TR,  // R1
        KeyCode::BTN_TL2, // L2 (as button)
        KeyCode::BTN_TR2, // R2 (as button)
        KeyCode::BTN_THUMBL,
        KeyCode::BTN_THUMBR,
        KeyCode::BTN_SELECT,
        KeyCode::BTN_START,
        KeyCode::BTN_MODE,
        KeyCode::BTN_DPAD_UP,
        KeyCode::BTN_DPAD_DOWN,
        KeyCode::BTN_DPAD_LEFT,
        KeyCode::BTN_DPAD_RIGHT,
    ]);

    let abs_axes = [
        (AbsoluteAxisCode::ABS_X, abs_stick_setup), // Left Stick X
        (AbsoluteAxisCode::ABS_Y, abs_stick_setup), // Left Stick Y
        (AbsoluteAxisCode::ABS_Z, abs_trigger_setup), // Left Trigger (L2)
        (AbsoluteAxisCode::ABS_RX, abs_stick_setup), // Right Stick X
        (AbsoluteAxisCode::ABS_RY, abs_stick_setup), // Right Stick Y
        (AbsoluteAxisCode::ABS_RZ, abs_trigger_setup), // Right Trigger (R2)
        (AbsoluteAxisCode::ABS_HAT0X, abs_stick_setup), // D-Pad X
        (AbsoluteAxisCode::ABS_HAT0Y, abs_stick_setup), // D-Pad Y
    ];

    let mut builder = VirtualDevice::builder()?;
    builder = builder.name(info.name);
    if let (Some(vendor), Some(product)) = (info.vendor_id, info.product_id) {
        builder = builder.input_id(evdev::InputId::new(
            evdev::BusType::BUS_USB,
            vendor,
            product,
            0x4242,
        ));
    }
    builder = builder.with_keys(&keys)?;

    for (code, info) in abs_axes {
        let setup = UinputAbsSetup::new(code, info);
        builder = builder.with_absolute_axis(&setup)?;
    }

    Ok(builder.build()?)
}

// --- gilrs to evdev Mappings ---

pub fn gilrs_button_to_evdev_key(button: Button) -> Option<KeyCode> {
    match button {
        Button::North => Some(KeyCode::BTN_NORTH),
        Button::South => Some(KeyCode::BTN_SOUTH),
        Button::East => Some(KeyCode::BTN_EAST),
        Button::West => Some(KeyCode::BTN_WEST),
        Button::LeftTrigger => Some(KeyCode::BTN_TL), // L1
        Button::RightTrigger => Some(KeyCode::BTN_TR), // R1
        Button::LeftTrigger2 => Some(KeyCode::BTN_TL2), // L2
        Button::RightTrigger2 => Some(KeyCode::BTN_TR2), // R2
        Button::LeftThumb => Some(KeyCode::BTN_THUMBL),
        Button::RightThumb => Some(KeyCode::BTN_THUMBR),
        Button::Select => Some(KeyCode::BTN_SELECT),
        Button::Start => Some(KeyCode::BTN_START),
        Button::Mode => Some(KeyCode::BTN_MODE),
        Button::DPadUp => Some(KeyCode::BTN_DPAD_UP),
        Button::DPadDown => Some(KeyCode::BTN_DPAD_DOWN),
        Button::DPadLeft => Some(KeyCode::BTN_DPAD_LEFT),
        Button::DPadRight => Some(KeyCode::BTN_DPAD_RIGHT),
        _ => None,
    }
}

pub fn gilrs_button_to_evdev_axis(button: Button) -> Option<AbsoluteAxisCode> {
    match button {
        Button::LeftTrigger2 => Some(AbsoluteAxisCode::ABS_Z),
        Button::RightTrigger2 => Some(AbsoluteAxisCode::ABS_RZ),
        // This logic handles D-Pad being mapped to HAT axes
        Button::DPadUp => Some(AbsoluteAxisCode::ABS_HAT0Y),
        Button::DPadDown => Some(AbsoluteAxisCode::ABS_HAT0Y),
        Button::DPadLeft => Some(AbsoluteAxisCode::ABS_HAT0X),
        Button::DPadRight => Some(AbsoluteAxisCode::ABS_HAT0X),
        _ => None,
    }
}

pub fn gilrs_axis_to_evdev_axis(axis: Axis) -> Option<AbsoluteAxisCode> {
    match axis {
        Axis::LeftStickX => Some(AbsoluteAxisCode::ABS_X),
        Axis::LeftStickY => Some(AbsoluteAxisCode::ABS_Y),
        Axis::LeftZ => Some(AbsoluteAxisCode::ABS_Z), // Some controllers map LT/RT to axes
        Axis::RightStickX => Some(AbsoluteAxisCode::ABS_RX),
        Axis::RightStickY => Some(AbsoluteAxisCode::ABS_RY),
        Axis::RightZ => Some(AbsoluteAxisCode::ABS_RZ), // Some controllers map LT/RT to axes
        _ => None,
    }
}

/// Returns the DPad axis pair for a given button, if applicable
pub fn dpad_axis_pair(button: Button) -> Option<[Button; 2]> {
    match button {
        Button::DPadUp | Button::DPadDown => Some([Button::DPadUp, Button::DPadDown]),
        Button::DPadLeft | Button::DPadRight => Some([Button::DPadLeft, Button::DPadRight]),
        _ => None,
    }
}
