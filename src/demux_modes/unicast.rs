use super::{DemuxMode, helpers};
use evdev::InputEvent;
use gilrs::{Button, Event, EventType, GamepadId, Gilrs};

/// Build-time flag to control latching behavior on active device switch.
/// When `false` (default), non-active devices are neutralized (axes centered, buttons released).
/// When `true`, non-active devices retain their last state (latching behavior).
const DEMUX_UNICAST_LATCHING: bool = false;

/// Unicast mode: Route primary to currently active virtual gamepad
/// Cycle active with Mode button
#[derive(Default)]
pub struct UnicastMode {
    active_index: usize,
}

impl UnicastMode {
    /// Synchronize controller state to newly active virtual device
    fn sync_controller_state(primary: gilrs::Gamepad) -> Vec<InputEvent> {
        let state = primary.state();
        let mut events = Vec::new();

        // Synchronize button states
        for (code, button_data) in state.buttons() {
            let Some(gilrs::ev::AxisOrBtn::Btn(btn)) = primary.axis_or_btn_name(code) else {
                continue;
            };

            // Continue if button is Mode (exclusive binding)
            if btn == gilrs::Button::Mode {
                continue;
            }

            // Handle buttons mapped to keys
            if let Some(event) = helpers::create_button_key_event(btn, button_data.is_pressed()) {
                events.push(event);
            }

            // Handle buttons mapped to axes (triggers, D-pad)
            if let Some(abs_axis) = crate::evdev_helpers::gilrs_button_to_evdev_axis(btn) {
                events.push(helpers::process_button_axis(btn, &primary, abs_axis));
            }
        }

        // Synchronize axis states
        for (code, axis_data) in state.axes() {
            let Some(gilrs::ev::AxisOrBtn::Axis(axis)) = primary.axis_or_btn_name(code) else {
                continue;
            };

            if let Some(event) = helpers::create_stick_event(axis, axis_data.value()) {
                events.push(event);
            }
        }

        events
    }

    /// Convert a gilrs event to evdev events
    fn convert_event(event: &Event, primary: gilrs::Gamepad) -> Option<Vec<InputEvent>> {
        match event.event {
            EventType::ButtonPressed(btn, _) | EventType::ButtonReleased(btn, _) => {
                let is_pressed = matches!(event.event, EventType::ButtonPressed(..));
                helpers::create_button_key_event(btn, is_pressed).map(|e| vec![e])
            }

            EventType::ButtonChanged(btn, _, _) => {
                let abs_axis = crate::evdev_helpers::gilrs_button_to_evdev_axis(btn)?;
                Some(vec![helpers::process_button_axis(btn, &primary, abs_axis)])
            }

            EventType::AxisChanged(axis, raw_val, _) => {
                helpers::create_stick_event(axis, raw_val).map(|e| vec![e])
            }

            _ => None,
        }
    }
}

impl DemuxMode for UnicastMode {
    fn handle_event(
        &mut self,
        event: &Event,
        primary_id: GamepadId,
        sinks: usize,
        gilrs: &Gilrs,
    ) -> Option<Vec<(usize, Vec<InputEvent>)>> {
        // Only handle primary controller
        if event.id != primary_id {
            return None;
        }

        // Handle mode button to cycle active device
        if matches!(event.event, EventType::ButtonPressed(Button::Mode, _)) {
            let old_active = self.active_index;
            self.active_index = (self.active_index + 1) % sinks;

            let primary = gilrs.gamepad(primary_id);
            let sync_events = Self::sync_controller_state(primary);

            let mut result = Vec::new();

            // If latching is disabled, neutralize the previously active device
            if !DEMUX_UNICAST_LATCHING && old_active != self.active_index {
                let neutral_events = crate::evdev_helpers::generate_neutral_gamepad_events();
                result.push((old_active, neutral_events));
            }

            // Sync the newly active device
            result.push((self.active_index, sync_events));

            return Some(result);
        }

        // Forward event to active device
        let primary = gilrs.gamepad(primary_id);
        Self::convert_event(event, primary).map(|events| vec![(self.active_index, events)])
    }
}
