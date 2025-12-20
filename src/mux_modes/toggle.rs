use super::MuxMode;
use evdev::InputEvent;
use gilrs::{Axis, Button, Event, EventType, GamepadId, Gilrs};

use crate::evdev_helpers;

#[derive(Default)]
pub struct ToggleMode {
    active_id: Option<GamepadId>,
}

const DEADZONE: f32 = 0.1;

impl ToggleMode {
    /// Calculate net axis value for D-pad from button states
    fn calculate_dpad_net_value(
        gamepad: &gilrs::Gamepad,
        neg_btn: Button,
        pos_btn: Button,
    ) -> f32 {
        let neg = gamepad.button_data(neg_btn).map_or(0.0, |d| d.value());
        let pos = gamepad.button_data(pos_btn).map_or(0.0, |d| d.value());
        pos - neg
    }

    /// Process button-to-axis mapping (D-pad or trigger)
    fn process_button_axis(
        btn: Button,
        gamepad: &gilrs::Gamepad,
        abs_axis: evdev::AbsoluteAxisCode,
    ) -> InputEvent {
        if let Some([neg_btn, pos_btn]) = evdev_helpers::dpad_axis_pair(btn) {
            // D-pad logic
            let net_value = Self::calculate_dpad_net_value(gamepad, neg_btn, pos_btn);
            let (active_btn, magnitude) = if net_value > DEADZONE {
                (pos_btn, net_value)
            } else {
                (neg_btn, net_value.abs())
            };

            let invert = matches!(active_btn, Button::DPadUp | Button::DPadLeft);
            let scaled = evdev_helpers::scale_stick(magnitude, invert);

            InputEvent::new(evdev::EventType::ABSOLUTE.0, abs_axis.0, scaled)
        } else {
            // Trigger logic
            let value = gamepad.button_data(btn).map_or(0.0, |d| d.value());
            let scaled = evdev_helpers::scale_trigger(value);

            InputEvent::new(evdev::EventType::ABSOLUTE.0, abs_axis.0, scaled)
        }
    }

    /// Synchronize all input states from the newly active controller
    fn sync_controller_state(
        active: gilrs::Gamepad,
        active_id: GamepadId,
        assist_id: GamepadId,
    ) -> Vec<InputEvent> {
        let state = active.state();
        let mut events = Vec::new();

        // Synchronize button states
        for (code, button_data) in state.buttons() {
            let Some(gilrs::ev::AxisOrBtn::Btn(btn)) = active.axis_or_btn_name(code) else {
                continue;
            };

            // Skip Mode button on assist controller for exclusive binding
            if active_id == assist_id && btn == Button::Mode {
                continue;
            }

            // Handle buttons mapped to keys
            if let Some(key) = evdev_helpers::gilrs_button_to_evdev_key(btn) {
                events.push(InputEvent::new(
                    evdev::EventType::KEY.0,
                    key.0,
                    button_data.is_pressed() as i32,
                ));
            }

            // Handle buttons mapped to axes (triggers, D-pad)
            if let Some(abs_axis) = evdev_helpers::gilrs_button_to_evdev_axis(btn) {
                events.push(Self::process_button_axis(btn, &active, abs_axis));
            }
        }

        // Synchronize axis states
        for (code, axis_data) in state.axes() {
            let Some(gilrs::ev::AxisOrBtn::Axis(axis)) = active.axis_or_btn_name(code) else {
                continue;
            };
            let Some(ev_axis) = evdev_helpers::gilrs_axis_to_evdev_axis(axis) else {
                continue;
            };

            let is_y_axis = matches!(axis, Axis::LeftStickY | Axis::RightStickY);
            let scaled = evdev_helpers::scale_stick(axis_data.value(), is_y_axis);

            events.push(InputEvent::new(
                evdev::EventType::ABSOLUTE.0,
                ev_axis.0,
                scaled,
            ));
        }

        events
    }

    /// Convert a gilrs event to evdev events
    fn convert_event(event: &Event, active: gilrs::Gamepad) -> Option<Vec<InputEvent>> {
        let events = match event.event {
            EventType::ButtonPressed(btn, _) | EventType::ButtonReleased(btn, _) => {
                let key = evdev_helpers::gilrs_button_to_evdev_key(btn)?;
                let is_pressed = matches!(event.event, EventType::ButtonPressed(..));

                vec![InputEvent::new(
                    evdev::EventType::KEY.0,
                    key.0,
                    is_pressed as i32,
                )]
            }

            EventType::ButtonChanged(btn, _, _) => {
                let abs_axis = evdev_helpers::gilrs_button_to_evdev_axis(btn)?;
                vec![Self::process_button_axis(btn, &active, abs_axis)]
            }

            EventType::AxisChanged(axis, raw_val, _) => {
                let ev_axis = evdev_helpers::gilrs_axis_to_evdev_axis(axis)?;
                let is_y_axis = matches!(axis, Axis::LeftStickY | Axis::RightStickY);
                let scaled = evdev_helpers::scale_stick(raw_val, is_y_axis);

                vec![InputEvent::new(
                    evdev::EventType::ABSOLUTE.0,
                    ev_axis.0,
                    scaled,
                )]
            }

            _ => return None,
        };

        Some(events)
    }
}

impl MuxMode for ToggleMode {
    fn handle_event(
        &mut self,
        event: &Event,
        primary_id: GamepadId,
        assist_id: GamepadId,
        gilrs: &Gilrs,
    ) -> Option<Vec<InputEvent>> {
        // Bootstrap active controller
        let active_id = self.active_id.get_or_insert(primary_id);

        // Handle toggle logic
        if matches!(
            (event.id, event.event),
            (id, EventType::ButtonPressed(Button::Mode, _)) if id == assist_id
        ) {
            *active_id = if *active_id == primary_id {
                assist_id
            } else {
                primary_id
            };

            let active = gilrs.gamepad(*active_id);
            return Some(Self::sync_controller_state(active, *active_id, assist_id));
        }

        // Only forward events from the active controller
        if event.id != *active_id {
            return None;
        }

        let active = gilrs.gamepad(*active_id);
        Self::convert_event(event, active)
    }
}
