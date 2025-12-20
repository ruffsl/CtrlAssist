use super::MuxMode;
use evdev::InputEvent;
use gilrs::{Axis, Button, Event, EventType, GamepadId, Gilrs};

use crate::evdev_helpers;

#[derive(Default)]
pub struct ToggleMode {
    active_id: Option<GamepadId>,
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

        // Toggle logic: if assist presses the toggle button, switch active
        if let (id, EventType::ButtonPressed(Button::Mode, _)) = (event.id, event.event)
            && id == assist_id
        {
            *active_id = if *active_id == primary_id {
                assist_id
            } else {
                primary_id
            };
            let active = gilrs.gamepad(*active_id);
            // TODO: Return events that reset all buttons/axes on the newly active controller
            // not implemented yet
            unimplemented!()
        }

        // Only forward events from the active controller
        if event.id != *active_id {
            return None;
        }

        // Convert gilrs event to evdev events
        let mut events = Vec::new();
        match event.event {
            EventType::ButtonPressed(btn, _) | EventType::ButtonReleased(btn, _) => {
                let key = evdev_helpers::gilrs_button_to_evdev_key(btn)?;
                let is_pressed = matches!(event.event, EventType::ButtonPressed(..));

                events.push(InputEvent::new(
                    evdev::EventType::KEY.0,
                    key.0,
                    is_pressed as i32,
                ));
            }
            EventType::ButtonChanged(btn, raw_val, _) => {
                let abs_axis = evdev_helpers::gilrs_button_to_evdev_axis(btn)?;
                let scaled = evdev_helpers::scale_trigger(raw_val);

                events.push(InputEvent::new(
                    evdev::EventType::ABSOLUTE.0,
                    abs_axis.0,
                    scaled,
                ));
            }
            EventType::AxisChanged(axis, raw_val, _) => {
                if let Some(ev_axis) = evdev_helpers::gilrs_axis_to_evdev_axis(axis) {

                    // Handle Y-axis inversion standard
                    let is_y = matches!(axis, Axis::LeftStickY | Axis::RightStickY);
                    let scaled = evdev_helpers::scale_stick(raw_val, is_y);

                    events.push(InputEvent::new(
                        evdev::EventType::ABSOLUTE.0,
                        ev_axis.0,
                        scaled,
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
