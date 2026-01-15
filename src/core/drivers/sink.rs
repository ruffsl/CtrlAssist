//! SinkNode: Writes CtrlEvents to virtual evdev devices.
//!
//! This driver converts CtrlEvents back to evdev InputEvents
//! and writes them to a virtual gamepad device.

use evdev::uinput::VirtualDevice;
use evdev::{EventType, InputEvent as EvdevInputEvent};

use crate::core::event::{AxisId, ButtonId, CtrlEvent, InputKind};

/// Axis range for evdev (0 to 65535 for absolute axes)
const AXIS_MAX: i32 = 65535;
const AXIS_MID: i32 = AXIS_MAX / 2;

/// Scale a normalized stick value (-1.0 to 1.0) to evdev range.
/// Note: Y axes are typically inverted in evdev (up = 0, down = max)
pub fn scale_stick(value: f32, invert: bool) -> i32 {
    let scaled = if invert {
        ((-value + 1.0) / 2.0 * AXIS_MAX as f32) as i32
    } else {
        ((value + 1.0) / 2.0 * AXIS_MAX as f32) as i32
    };
    scaled.clamp(0, AXIS_MAX)
}

/// Scale a normalized trigger value (0.0 to 1.0) to evdev range
pub fn scale_trigger(value: f32) -> i32 {
    let scaled = (value * AXIS_MAX as f32) as i32;
    scaled.clamp(0, AXIS_MAX)
}

/// Convert a ButtonId to evdev KeyCode
pub fn button_id_to_evdev_key(button: ButtonId) -> Option<evdev::KeyCode> {
    use evdev::KeyCode;
    match button {
        ButtonId::South => Some(KeyCode::BTN_SOUTH),
        ButtonId::East => Some(KeyCode::BTN_EAST),
        ButtonId::North => Some(KeyCode::BTN_NORTH),
        ButtonId::West => Some(KeyCode::BTN_WEST),
        ButtonId::LeftBumper => Some(KeyCode::BTN_TL),
        ButtonId::RightBumper => Some(KeyCode::BTN_TR),
        ButtonId::LeftTriggerButton => Some(KeyCode::BTN_TL2),
        ButtonId::RightTriggerButton => Some(KeyCode::BTN_TR2),
        ButtonId::Select => Some(KeyCode::BTN_SELECT),
        ButtonId::Start => Some(KeyCode::BTN_START),
        ButtonId::Mode => Some(KeyCode::BTN_MODE),
        ButtonId::LeftThumb => Some(KeyCode::BTN_THUMBL),
        ButtonId::RightThumb => Some(KeyCode::BTN_THUMBR),
        // D-pad buttons are handled as axes
        ButtonId::DPadUp | ButtonId::DPadDown | ButtonId::DPadLeft | ButtonId::DPadRight => None,
    }
}

/// Convert an AxisId to evdev AbsoluteAxisCode
pub fn axis_id_to_evdev_axis(axis: AxisId) -> evdev::AbsoluteAxisCode {
    use evdev::AbsoluteAxisCode;
    match axis {
        AxisId::LeftStickX => AbsoluteAxisCode::ABS_X,
        AxisId::LeftStickY => AbsoluteAxisCode::ABS_Y,
        AxisId::RightStickX => AbsoluteAxisCode::ABS_RX,
        AxisId::RightStickY => AbsoluteAxisCode::ABS_RY,
        AxisId::LeftTrigger => AbsoluteAxisCode::ABS_Z,
        AxisId::RightTrigger => AbsoluteAxisCode::ABS_RZ,
        AxisId::DPadX => AbsoluteAxisCode::ABS_HAT0X,
        AxisId::DPadY => AbsoluteAxisCode::ABS_HAT0Y,
    }
}

/// Convert a CtrlEvent to evdev InputEvents
pub fn ctrl_event_to_evdev(event: &CtrlEvent) -> Vec<EvdevInputEvent> {
    match event {
        CtrlEvent::Input(input) => match &input.input {
            InputKind::Button { id, pressed } => {
                if let Some(key) = button_id_to_evdev_key(*id) {
                    vec![EvdevInputEvent::new(
                        EventType::KEY.0,
                        key.0,
                        if *pressed { 1 } else { 0 },
                    )]
                } else {
                    // D-pad buttons - convert to axis events
                    let (axis, value) = match id {
                        ButtonId::DPadUp => (AxisId::DPadY, if *pressed { -1.0 } else { 0.0 }),
                        ButtonId::DPadDown => (AxisId::DPadY, if *pressed { 1.0 } else { 0.0 }),
                        ButtonId::DPadLeft => (AxisId::DPadX, if *pressed { -1.0 } else { 0.0 }),
                        ButtonId::DPadRight => (AxisId::DPadX, if *pressed { 1.0 } else { 0.0 }),
                        _ => return vec![],
                    };
                    let evdev_axis = axis_id_to_evdev_axis(axis);
                    // D-pad uses -1, 0, 1 range for HAT axes
                    let scaled = value as i32;
                    vec![EvdevInputEvent::new(
                        EventType::ABSOLUTE.0,
                        evdev_axis.0,
                        scaled,
                    )]
                }
            }
            InputKind::Axis { id, value } => {
                let evdev_axis = axis_id_to_evdev_axis(*id);
                let scaled = match id {
                    AxisId::LeftStickX | AxisId::RightStickX => scale_stick(*value, false),
                    AxisId::LeftStickY | AxisId::RightStickY => scale_stick(*value, true),
                    AxisId::LeftTrigger | AxisId::RightTrigger => scale_trigger(*value),
                    AxisId::DPadX | AxisId::DPadY => {
                        // D-pad uses -1, 0, 1 raw values
                        if value.abs() > 0.5 {
                            value.signum() as i32
                        } else {
                            0
                        }
                    }
                };
                vec![EvdevInputEvent::new(
                    EventType::ABSOLUTE.0,
                    evdev_axis.0,
                    scaled,
                )]
            }
        },
        // FF and LED events are not handled by the sink node (yet)
        CtrlEvent::ForceFeedback(_) | CtrlEvent::Led(_) => vec![],
    }
}

/// Sink that writes CtrlEvents to a virtual evdev device.
pub struct EvdevSink {
    device: VirtualDevice,
}

impl EvdevSink {
    /// Create a new sink for the given virtual device
    pub fn new(device: VirtualDevice) -> Self {
        Self { device }
    }

    /// Write a CtrlEvent to the virtual device
    pub fn write_event(&mut self, event: &CtrlEvent) -> Result<(), std::io::Error> {
        let evdev_events = ctrl_event_to_evdev(event);
        if !evdev_events.is_empty() {
            self.device.emit(&evdev_events)?;
        }
        Ok(())
    }

    /// Write multiple CtrlEvents with a sync event at the end
    pub fn write_events(&mut self, events: &[CtrlEvent]) -> Result<(), std::io::Error> {
        let mut all_evdev_events = Vec::new();
        for event in events {
            all_evdev_events.extend(ctrl_event_to_evdev(event));
        }
        if !all_evdev_events.is_empty() {
            // Add sync event
            all_evdev_events.push(EvdevInputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0));
            self.device.emit(&all_evdev_events)?;
        }
        Ok(())
    }

    /// Get a reference to the underlying virtual device
    pub fn device(&self) -> &VirtualDevice {
        &self.device
    }

    /// Get a mutable reference to the underlying virtual device
    pub fn device_mut(&mut self) -> &mut VirtualDevice {
        &mut self.device
    }
}
