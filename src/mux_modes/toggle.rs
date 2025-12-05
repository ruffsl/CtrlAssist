use super::MuxMode;
use evdev::InputEvent;
use gilrs::{Axis, Button, Event, GamepadId};

use crate::evdev_helpers;

pub struct ToggleMode {
    active: Option<GamepadId>,
}

impl ToggleMode {
    pub fn new() -> Self {
        Self { active: None }
    }
}

impl MuxMode for ToggleMode {
    fn handle_event(
        &mut self,
        event: &Event,
        primary_id: GamepadId,
        assist_id: GamepadId,
        _gilrs: &gilrs::Gilrs,
    ) -> Option<Vec<InputEvent>> {
        // Bootstrap active controller
        let active = self.active.get_or_insert(primary_id);

        // Toggle logic: if assist presses the toggle button, switch active
        if let (id, gilrs::EventType::ButtonPressed(Button::Mode, _)) = (event.id, event.event)
            && id == assist_id
        {
            *active = if *active == primary_id {
                assist_id
            } else {
                primary_id
            };
            return None;
        }

        // Only forward events from the active controller
        if event.id != *active {
            return None;
        }

        // Convert gilrs event to evdev events
        let mut events = Vec::new();
        match event.event {
            gilrs::EventType::ButtonPressed(button, _)
            | gilrs::EventType::ButtonReleased(button, _) => {
                if let Some(key) = evdev_helpers::gilrs_button_to_evdev_key(button) {
                    let value = matches!(event.event, gilrs::EventType::ButtonPressed(..)) as i32;
                    events.push(InputEvent::new(evdev::EventType::KEY.0, key.0, value));
                }
            }
            gilrs::EventType::ButtonChanged(button, value, _) => {
                if let Some(abs_axis) = evdev_helpers::gilrs_button_to_evdev_axis(button) {
                    let scaled_value = evdev_helpers::scale_trigger(value);
                    events.push(InputEvent::new(
                        evdev::EventType::ABSOLUTE.0,
                        abs_axis.0,
                        scaled_value,
                    ));
                }
            }
            gilrs::EventType::AxisChanged(axis, value, _) => {
                if let Some(abs_axis) = evdev_helpers::gilrs_axis_to_evdev_axis(axis) {
                    let scaled_value = match axis {
                        Axis::LeftStickY | Axis::RightStickY => {
                            evdev_helpers::scale_stick(value, true)
                        }
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
