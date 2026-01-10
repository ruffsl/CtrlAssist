use evdev::InputEvent;
use gilrs::{Axis, Button, Gamepad};

use crate::evdev_helpers;

pub const DEADZONE: f32 = 0.1;

/// Calculate net axis value for D-pad from button states (-1.0 to 1.0)
pub fn calculate_dpad_net_value(gamepad: &Gamepad, neg_btn: Button, pos_btn: Button) -> f32 {
    let neg = gamepad.button_data(neg_btn).map_or(0.0, |d| d.value());
    let pos = gamepad.button_data(pos_btn).map_or(0.0, |d| d.value());
    pos - neg
}

/// Check if a stick is active using circular deadzone
pub fn is_stick_active(gamepad: &Gamepad, x_axis: Axis, y_axis: Axis) -> bool {
    let x = gamepad.axis_data(x_axis).map_or(0.0, |d| d.value());
    let y = gamepad.axis_data(y_axis).map_or(0.0, |d| d.value());
    (x * x + y * y).sqrt() > DEADZONE
}

/// Map an axis to its stick pair (X and Y)
pub fn map_to_stick_pair(axis: Axis) -> Option<(Axis, Axis)> {
    match axis {
        Axis::LeftStickX | Axis::LeftStickY => Some((Axis::LeftStickX, Axis::LeftStickY)),
        Axis::RightStickX | Axis::RightStickY => Some((Axis::RightStickX, Axis::RightStickY)),
        _ => None,
    }
}

/// Create an InputEvent for a button key press/release
pub fn create_button_key_event(btn: Button, is_pressed: bool) -> Option<InputEvent> {
    let key = evdev_helpers::gilrs_button_to_evdev_key(btn)?;
    Some(InputEvent::new(
        evdev::EventType::KEY.0,
        key.0,
        is_pressed as i32,
    ))
}

/// Create InputEvent(s) for D-pad axis
pub fn create_dpad_event(
    net_value: f32,
    neg_btn: Button,
    pos_btn: Button,
    abs_axis: evdev::AbsoluteAxisCode,
) -> InputEvent {
    let (active_btn, magnitude) = if net_value > DEADZONE {
        (pos_btn, net_value)
    } else {
        (neg_btn, net_value.abs())
    };

    let invert = matches!(active_btn, Button::DPadUp | Button::DPadLeft);
    let scaled = evdev_helpers::scale_stick(magnitude, invert);

    InputEvent::new(evdev::EventType::ABSOLUTE.0, abs_axis.0, scaled)
}

/// Create an InputEvent for a trigger axis
pub fn create_trigger_event(value: f32, abs_axis: evdev::AbsoluteAxisCode) -> InputEvent {
    let scaled = evdev_helpers::scale_trigger(value);
    InputEvent::new(evdev::EventType::ABSOLUTE.0, abs_axis.0, scaled)
}

/// Create an InputEvent for a stick axis
pub fn create_stick_event(axis: Axis, value: f32) -> Option<InputEvent> {
    let ev_axis = evdev_helpers::gilrs_axis_to_evdev_axis(axis)?;
    let is_y_axis = matches!(axis, Axis::LeftStickY | Axis::RightStickY);
    let scaled = evdev_helpers::scale_stick(value, is_y_axis);

    Some(InputEvent::new(
        evdev::EventType::ABSOLUTE.0,
        ev_axis.0,
        scaled,
    ))
}

/// Process a button that maps to an axis (D-pad or trigger)
pub fn process_button_axis(
    btn: Button,
    gamepad: &Gamepad,
    abs_axis: evdev::AbsoluteAxisCode,
) -> InputEvent {
    if let Some([neg_btn, pos_btn]) = evdev_helpers::dpad_axis_pair(btn) {
        let net_value = calculate_dpad_net_value(gamepad, neg_btn, pos_btn);
        create_dpad_event(net_value, neg_btn, pos_btn, abs_axis)
    } else {
        let value = gamepad.button_data(btn).map_or(0.0, |d| d.value());
        create_trigger_event(value, abs_axis)
    }
}
