use super::{MuxMode, helpers};
use evdev::InputEvent;
use gilrs::{Event, EventType, GamepadId, Gilrs};

use crate::evdev_helpers;

#[derive(Default)]
pub struct ToggleMode {
    active_id: Option<GamepadId>,
}

impl ToggleMode {
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
            if active_id == assist_id && btn == gilrs::Button::Mode {
                continue;
            }

            // Handle buttons mapped to keys
            if let Some(event) = helpers::create_button_key_event(btn, button_data.is_pressed()) {
                events.push(event);
            }

            // Handle buttons mapped to axes (triggers, D-pad)
            if let Some(abs_axis) = evdev_helpers::gilrs_button_to_evdev_axis(btn) {
                events.push(helpers::process_button_axis(btn, &active, abs_axis));
            }
        }

        // Synchronize axis states
        for (code, axis_data) in state.axes() {
            let Some(gilrs::ev::AxisOrBtn::Axis(axis)) = active.axis_or_btn_name(code) else {
                continue;
            };

            if let Some(event) = helpers::create_stick_event(axis, axis_data.value()) {
                events.push(event);
            }
        }

        events
    }

    /// Convert a gilrs event to evdev events
    fn convert_event(event: &Event, active: gilrs::Gamepad) -> Option<Vec<InputEvent>> {
        match event.event {
            EventType::ButtonPressed(btn, _) | EventType::ButtonReleased(btn, _) => {
                let is_pressed = matches!(event.event, EventType::ButtonPressed(..));
                helpers::create_button_key_event(btn, is_pressed).map(|e| vec![e])
            }

            EventType::ButtonChanged(btn, _, _) => {
                let abs_axis = evdev_helpers::gilrs_button_to_evdev_axis(btn)?;
                Some(vec![helpers::process_button_axis(btn, &active, abs_axis)])
            }

            EventType::AxisChanged(axis, raw_val, _) => {
                helpers::create_stick_event(axis, raw_val).map(|e| vec![e])
            }

            _ => None,
        }
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
        let active_id = self.active_id.get_or_insert(primary_id);

        // Handle toggle logic
        if matches!(
            (event.id, event.event),
            (id, EventType::ButtonPressed(gilrs::Button::Mode, _)) if id == assist_id
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
