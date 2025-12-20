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
            // Assist wins conflicts. Forward input only if Assist isn't pressing it.
            EventType::ButtonPressed(btn, _) | EventType::ButtonReleased(btn, _) => {
                let key = evdev_helpers::gilrs_button_to_evdev_key(btn)?;
                let is_pressed = matches!(event.event, EventType::ButtonPressed(..));

                // Check if the *other* controller is holding this button
                let other_holding = if event.id == primary_id {
                    assist.is_pressed(btn)
                } else {
                    primary.is_pressed(btn)
                };

                // If Assist is still holding, block this event.
                // Still allow release from Assist override Primary.
                if other_holding && event.id == primary_id {
                    return None;
                }

                events.push(InputEvent::new(
                    evdev::EventType::KEY.0,
                    key.0,
                    is_pressed as i32,
                ));
            }

            // --- Analog Triggers & D-Pads ---
            EventType::ButtonChanged(btn, _, _) => {
                let abs_axis = evdev_helpers::gilrs_button_to_evdev_axis(btn)?;

                // 1. D-PAD LOGIC (Strict Primary Priority on Axis Pairs)
                if let Some([neg_btn, pos_btn]) = evdev_helpers::dpad_axis_pair(btn) {
                    // Helper to calculate "Net Axis Value" (-1.0 to 1.0) for a controller
                    let get_net_axis = |pad: &gilrs::Gamepad| -> f32 {
                        let neg = pad.button_data(neg_btn).map_or(0.0, |d| d.value());
                        let pos = pad.button_data(pos_btn).map_or(0.0, |d| d.value());
                        pos - neg
                    };

                    let a_net = get_net_axis(&assist);
                    let p_net = get_net_axis(&primary);

                    // If Assist is active on this axis, it rules. Otherwise, Primary.
                    // This handles both "Override" and "Return to Primary" automatically.
                    let final_val = if a_net.abs() > DEADZONE { a_net } else { p_net };
                    
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
                    
                    events.push(InputEvent::new(evdev::EventType::ABSOLUTE.0, abs_axis.0, scaled));
                } 
                // 2. TRIGGER LOGIC (Highest Value Wins)
                else {
                    let p_val = primary.button_data(btn).map_or(0.0, |d| d.value());
                    let a_val = assist.button_data(btn).map_or(0.0, |d| d.value());
                    let max_val = p_val.max(a_val);

                    events.push(InputEvent::new(
                        evdev::EventType::ABSOLUTE.0,
                        abs_axis.0,
                        evdev_helpers::scale_trigger(max_val),
                    ));
                }
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
