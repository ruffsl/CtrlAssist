use super::MuxMode;
use evdev::InputEvent;
use gilrs::{Axis, Button, Event, GamepadId};

use crate::evdev_helpers;

#[derive(Default)]
pub struct AverageMode;

const DEFAULT_DEADZONE: f32 = 0.1;

fn deadzone() -> f32 {
    DEFAULT_DEADZONE
}

impl MuxMode for AverageMode {
    fn handle_event(
        &mut self,
        event: &Event,
        primary_id: GamepadId,
        assist_id: GamepadId,
        gilrs: &gilrs::Gilrs,
    ) -> Option<Vec<InputEvent>> {
        // Ignore events from devices other than primary and assist
        let other_id = match event.id {
            id if id == primary_id => assist_id,
            id if id == assist_id => primary_id,
            _ => return None,
        };

        // Always get up-to-date gamepad handles from active gilrs instance
        let other_gamepad = gilrs.gamepad(other_id);

        // Convert gilrs event to evdev events
        let mut events = Vec::new();
        match event.event {
            // --- Digital Buttons ---
            gilrs::EventType::ButtonPressed(button, _)
            | gilrs::EventType::ButtonReleased(button, _) => {
                if let Some(key) = evdev_helpers::gilrs_button_to_evdev_key(button) {
                    let value = matches!(event.event, gilrs::EventType::ButtonPressed(..)) as i32;
                    // Only relay if the other gamepad does not have the button pressed
                    let other_pressed = other_gamepad
                        .button_data(button)
                        .is_some_and(|d| d.value() != 0.0);
                    if other_pressed {
                        return None;
                    }
                    events.push(InputEvent::new(evdev::EventType::KEY.0, key.0, value));
                }
            }

            // --- Analog Triggers / Pressure Buttons ---
            gilrs::EventType::ButtonChanged(button, value, _) => {
                if let Some(abs_axis) = evdev_helpers::gilrs_button_to_evdev_axis(button) {
                    let mut value = value;
                    let mut button = button;

                    // 1. Identify the Axis Pair
                    let axis_pair = match button {
                        Button::DPadUp | Button::DPadDown => {
                            Some([Button::DPadUp, Button::DPadDown])
                        }
                        Button::DPadLeft | Button::DPadRight => {
                            Some([Button::DPadLeft, Button::DPadRight])
                        }
                        _ => None,
                    };

                    if let Some(pair) = axis_pair {
                        // Closure to check if the OTHER controller is pressing a button
                        let is_other_pressing = |b| {
                            other_gamepad
                                .button_data(b)
                                .is_some_and(|d| d.value() > 0.0)
                        };

                        if other_id == assist_id && pair.iter().copied().any(is_other_pressing) {
                            return None; // Primary is blocked because Assist is active on this axis
                        } else if other_id == primary_id && value == 0.0 {
                            // Assist released; if Primary is holding a button, adopt it
                            if let Some(active_btn) =
                                pair.iter().copied().find(|&b| is_other_pressing(b))
                            {
                                button = active_btn;
                                value = 1.0;
                            }
                        }
                    }
                    let other_active = match button {
                        Button::DPadUp
                        | Button::DPadDown
                        | Button::DPadLeft
                        | Button::DPadRight => false,
                        _ => other_gamepad
                            .button_data(button)
                            .is_some_and(|d| d.value() >= deadzone()),
                    };
                    if other_active {
                        value = (value + other_gamepad.button_data(button).unwrap().value()) / 2.0;
                    }
                    let scaled_value = match button {
                        // D-pad-as-axis (uncommon, but matches original logic)
                        Button::DPadUp | Button::DPadLeft => {
                            evdev_helpers::scale_stick(value, true)
                        }
                        Button::DPadDown | Button::DPadRight => {
                            evdev_helpers::scale_stick(value, false)
                        }
                        // Analog triggers (LT2/RT2)
                        _ => evdev_helpers::scale_trigger(value),
                    };
                    events.push(InputEvent::new(
                        evdev::EventType::ABSOLUTE.0,
                        abs_axis.0,
                        scaled_value,
                    ));
                }
            }

            // --- Analog Sticks ---
            gilrs::EventType::AxisChanged(axis, value, _) => {
                if let Some(abs_axis) = evdev_helpers::gilrs_axis_to_evdev_axis(axis) {
                    let mut value = value;
                    // Only relay if not conflicting with assist joysticks
                    let other_pushed = match axis {
                        Axis::LeftStickX | Axis::LeftStickY => {
                            other_gamepad
                                .axis_data(Axis::LeftStickX)
                                .is_some_and(|d| d.value().abs() >= deadzone())
                                || other_gamepad
                                    .axis_data(Axis::LeftStickY)
                                    .is_some_and(|d| d.value().abs() >= deadzone())
                        }
                        Axis::RightStickX | Axis::RightStickY => {
                            other_gamepad
                                .axis_data(Axis::RightStickX)
                                .is_some_and(|d| d.value().abs() >= deadzone())
                                || other_gamepad
                                    .axis_data(Axis::RightStickY)
                                    .is_some_and(|d| d.value().abs() >= deadzone())
                        }
                        _ => false,
                    };
                    if other_pushed {
                        value = (value + other_gamepad.axis_data(axis).unwrap().value()) / 2.0;
                    }
                    let scaled_value = match axis {
                        // Invert Y axes
                        Axis::LeftStickY | Axis::RightStickY => {
                            evdev_helpers::scale_stick(value, true)
                        }
                        // X axes
                        _ => evdev_helpers::scale_stick(value, false),
                    };
                    events.push(InputEvent::new(
                        evdev::EventType::ABSOLUTE.0,
                        abs_axis.0,
                        scaled_value,
                    ));
                }
            }
            _ => {}
        }
        if events.is_empty() {
            None
        } else {
            Some(events)
        }
    }
}
