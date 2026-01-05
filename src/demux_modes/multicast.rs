use super::{DemuxMode, helpers};
use evdev::InputEvent;
use gilrs::{Event, EventType, GamepadId, Gilrs};

/// Multicast mode: Route primary to all virtual gamepads
pub struct MulticastMode;

impl MulticastMode {
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

impl DemuxMode for MulticastMode {
    fn handle_event(
        &mut self,
        event: &Event,
        primary_id: GamepadId,
        sinks: usize,
        _gilrs: &Gilrs,
    ) -> Option<Vec<(usize, Vec<InputEvent>)>> {
        // Only handle primary controller
        if event.id != primary_id {
            return None;
        }

        let primary = _gilrs.gamepad(primary_id);

        // Broadcast to all virtual devices
        Self::convert_event(event, primary).map(|events| {
            (0..sinks)
                .map(|idx| (idx, events.clone()))
                .collect()
        })
    }
}
