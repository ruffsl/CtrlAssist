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
            let state = active.state();
            let mut sync_events = Vec::new();

            // Synchronize all button states to their current values
            for (code, button_data) in state.buttons() {
                if let Some(gilrs::ev::AxisOrBtn::Btn(btn)) = active.axis_or_btn_name(code) {
                    // if active id is assist and button is Mode, skip to avoid retriggering toggle
                    if *active_id == assist_id && btn == Button::Mode {
                        continue;
                    }
                    // For buttons that map to keys
                    if let Some(key) = evdev_helpers::gilrs_button_to_evdev_key(btn) {
                        sync_events.push(InputEvent::new(
                            evdev::EventType::KEY.0,
                            key.0,
                            button_data.is_pressed() as i32,
                        ));
                    }
                    
                    // For buttons that map to axes (like triggers)
                    if let Some(abs_axis) = evdev_helpers::gilrs_button_to_evdev_axis(btn) {
                        let scaled = evdev_helpers::scale_trigger(button_data.value());
                        sync_events.push(InputEvent::new(
                            evdev::EventType::ABSOLUTE.0,
                            abs_axis.0,
                            scaled,
                        ));
                    }
                }
            }

            // Synchronize all axis states to their current values
            for (code, axis_data) in state.axes() {
                if let Some(gilrs::ev::AxisOrBtn::Axis(axis)) = active.axis_or_btn_name(code) {
                    if let Some(ev_axis) = evdev_helpers::gilrs_axis_to_evdev_axis(axis) {
                        // Handle Y-axis inversion standard
                        let is_y = matches!(axis, Axis::LeftStickY | Axis::RightStickY);
                        let scaled = evdev_helpers::scale_stick(axis_data.value(), is_y);
                        
                        sync_events.push(InputEvent::new(
                            evdev::EventType::ABSOLUTE.0,
                            ev_axis.0,
                            scaled,
                        ));
                    }
                }
            }
            return Some(sync_events);
        }

        // Only forward events from the active controller
        if event.id != *active_id {
            return None;
        }
        
        let active = gilrs.gamepad(*active_id);

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
            // --- Analog Triggers & D-Pads ---
            EventType::ButtonChanged(btn, _, _) => {
                let abs_axis = evdev_helpers::gilrs_button_to_evdev_axis(btn)?;

                // 1. D-PAD LOGIC
                if let Some([neg_btn, pos_btn]) = evdev_helpers::dpad_axis_pair(btn) {
                    // Helper to calculate "Net Axis Value" (-1.0 to 1.0) for a controller
                    let get_net_axis = |pad: &gilrs::Gamepad| -> f32 {
                        let neg = pad.button_data(neg_btn).map_or(0.0, |d| d.value());
                        let pos = pad.button_data(pos_btn).map_or(0.0, |d| d.value());
                        pos - neg
                    };
                    let final_val = get_net_axis(&active);

                    // If the calculated `final_val` is effectively "Up", treat it as DPadUp press.
                    let (active_btn, mag) = if final_val > DEADZONE {
                        (pos_btn, final_val)
                    } else {
                        (neg_btn, final_val.abs())
                    };

                    // Note: DPadUp/Left usually map to -1. Check your `scale_stick` impl.
                    // Assuming `scale_stick` handles the typical 0..1 -> axis conversion:
                    let invert = matches!(active_btn, Button::DPadUp | Button::DPadLeft);
                    let scaled = evdev_helpers::scale_stick(mag, invert);

                    events.push(InputEvent::new(
                        evdev::EventType::ABSOLUTE.0,
                        abs_axis.0,
                        scaled,
                    ));
                }
                // 2. TRIGGER LOGIC
                else {
                    let a_val = active.button_data(btn).map_or(0.0, |d| d.value());

                    events.push(InputEvent::new(
                        evdev::EventType::ABSOLUTE.0,
                        abs_axis.0,
                        evdev_helpers::scale_trigger(a_val),
                    ));
                }
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
