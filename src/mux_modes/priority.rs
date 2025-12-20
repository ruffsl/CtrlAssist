use super::MuxMode;
use crate::evdev_helpers;
use evdev::InputEvent;
use gilrs::{Axis, Button, Event, EventType, GamepadId, Gilrs};

#[derive(Default)]
pub struct PriorityMode;

const DEADZONE: f32 = 0.1;

impl MuxMode for PriorityMode {
    fn handle_event(
        &mut self,
        event: &Event,
        primary_id: GamepadId,
        assist_id: GamepadId,
        gilrs: &Gilrs,
    ) -> Option<Vec<InputEvent>> {
        // Filter out irrelevant devices
        if event.id != primary_id && event.id != assist_id {
            return None;
        }

        let primary = gilrs.gamepad(primary_id);
        let assist = gilrs.gamepad(assist_id);
        let mut events = Vec::new();

        match event.event {
            // --- Digital Buttons (XOR-like Logic) ---
            // Primary wins conflicts. Forward input only if the other isn't pressing it.
            EventType::ButtonPressed(btn, _) | EventType::ButtonReleased(btn, _) => {
                let key = evdev_helpers::gilrs_button_to_evdev_key(btn)?;
                let is_pressed = matches!(event.event, EventType::ButtonPressed(..));

                // Check if the *other* controller is holding this button
                let other_holding = if event.id == primary_id {
                    assist.is_pressed(btn)
                } else {
                    primary.is_pressed(btn)
                };

                // If other is holding, block this event (unless Primary is overriding Assist)
                if other_holding && event.id == primary_id {
                    return None;
                }

                events.push(InputEvent::new(
                    evdev::EventType::KEY.0,
                    key.0,
                    is_pressed as i32,
                ));
            }

            // --- Analog Triggers & Pressure (Max Value Wins) ---
            // If Assist > Primary, Assist wins. If Assist drops, Primary takes over.
            EventType::ButtonChanged(btn, val, _) => {
                let abs_axis = evdev_helpers::gilrs_button_to_evdev_axis(btn)?;
                
                // Get current values from both controllers
                let p_val = primary.button_data(btn).map_or(0.0, |d| d.value());
                let a_val = assist.button_data(btn).map_or(0.0, |d| d.value());

                // The output is simply the higher of the two. 
                // This automatically handles the "Assist release -> snap to Primary" requirement.
                let max_val = p_val.max(a_val);

                // Identify scaling mode (Triggers vs DPad-as-Axis)
                let is_y_axis = matches!(btn, Button::DPadUp | Button::DPadLeft);
                let scaled = if matches!(btn, Button::DPadUp | Button::DPadDown | Button::DPadLeft | Button::DPadRight) {
                     evdev_helpers::scale_stick(max_val, is_y_axis)
                } else {
                     evdev_helpers::scale_trigger(max_val)
                };

                events.push(InputEvent::new(evdev::EventType::ABSOLUTE.0, abs_axis.0, scaled));
            }

            // --- Joysticks (Snap Logic) ---
            // If Assist is active (out of deadzone), it owns the stick. Otherwise, Primary owns it.
            EventType::AxisChanged(axis, _, _) => {
                // Map axis to specific stick (Left or Right)
                let (x_axis, y_axis) = match axis {
                    Axis::LeftStickX | Axis::LeftStickY => (Axis::LeftStickX, Axis::LeftStickY),
                    Axis::RightStickX | Axis::RightStickY => (Axis::RightStickX, Axis::RightStickY),
                    _ => return None, // Ignore non-stick axes here
                };

                // Check Assist's activity on this specific stick
                let a_x = assist.axis_data(x_axis).map_or(0.0, |d| d.value());
                let a_y = assist.axis_data(y_axis).map_or(0.0, |d| d.value());
                let assist_active = a_x.abs() > DEADZONE || a_y.abs() > DEADZONE;

                // Determine the "Owner" of the stick
                let owner = if assist_active { assist } else { primary };
                
                // Optimization: If Primary moved but Assist is active, ignore completely.
                if event.id == primary_id && assist_active {
                    return None;
                }

                // Push updates for BOTH axes of the stick to ensure sync (Snap effect)
                for ax in [x_axis, y_axis] {
                    if let Some(ev_axis) = evdev_helpers::gilrs_axis_to_evdev_axis(ax) {
                        let raw_val = owner.axis_data(ax).map_or(0.0, |d| d.value());
                        
                        // Handle Y-axis inversion standard
                        let is_y = matches!(ax, Axis::LeftStickY | Axis::RightStickY);
                        let scaled = evdev_helpers::scale_stick(raw_val, is_y);

                        events.push(InputEvent::new(evdev::EventType::ABSOLUTE.0, ev_axis.0, scaled));
                    }
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
