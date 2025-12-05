use super::MuxMode;
use evdev::InputEvent;
use gilrs::{Event, GamepadId};

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
        // Lazily initialize active controller to primary_id
        if self.active.is_none() {
            self.active = Some(primary_id);
        }

        // Define the toggle button (e.g., Start)
        use gilrs::Button;
        let toggle_button = Button::Mode;

        // If event is from assist and toggle button is pressed, toggle active controller
        if event.id == assist_id
            && let gilrs::EventType::ButtonPressed(button, _) = event.event
            && button == toggle_button
        {
            self.active = Some(if self.active == Some(primary_id) {
                assist_id
            } else {
                primary_id
            });
        }

        // Only forward events from the active controller
        if Some(event.id) != self.active {
            return None;
        }

        // Forward event using PriorityMode logic
        let mut events = Vec::new();
        use crate::evdev_helpers;
        match event.event {
            gilrs::EventType::ButtonPressed(button, _)
            | gilrs::EventType::ButtonReleased(button, _) => {
                if let Some(key) = evdev_helpers::gilrs_button_to_evdev_key(button) {
                    let value = if matches!(event.event, gilrs::EventType::ButtonPressed(..)) {
                        1
                    } else {
                        0
                    };
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
                        // Invert Y axes
                        gilrs::Axis::LeftStickY | gilrs::Axis::RightStickY => {
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
        if !events.is_empty() {
            Some(events)
        } else {
            None
        }
    }
}
