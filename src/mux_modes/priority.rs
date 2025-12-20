use super::{helpers, MuxMode};
use crate::evdev_helpers;
use evdev::InputEvent;
use gilrs::{Button, Event, EventType, GamepadId, Gilrs};

#[derive(Default)]
pub struct PriorityMode;

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

        match event.event {
            EventType::ButtonPressed(btn, _) | EventType::ButtonReleased(btn, _) => {
                // Skip unknown buttons - they may be mapped to axes instead
                if btn == Button::Unknown {
                    return None;
                }

                let is_pressed = matches!(event.event, EventType::ButtonPressed(..));

                // Check if assist is holding this button
                let assist_holding = assist.is_pressed(btn);

                // Block primary's event if assist is holding
                if assist_holding && event.id == primary_id {
                    return None;
                }

                helpers::create_button_key_event(btn, is_pressed).map(|e| vec![e])
            }

            EventType::ButtonChanged(btn, _, _) => {
                let abs_axis = evdev_helpers::gilrs_button_to_evdev_axis(btn)?;

                let event = if let Some([neg_btn, pos_btn]) = evdev_helpers::dpad_axis_pair(btn) {
                    // D-pad: Assist priority
                    let assist_net = helpers::calculate_dpad_net_value(&assist, neg_btn, pos_btn);
                    let primary_net = helpers::calculate_dpad_net_value(&primary, neg_btn, pos_btn);
                    
                    let final_value = if assist_net.abs() > helpers::DEADZONE {
                        assist_net
                    } else {
                        primary_net
                    };

                    helpers::create_dpad_event(final_value, neg_btn, pos_btn, abs_axis)
                } else {
                    // Trigger: Highest value wins
                    let primary_val = primary.button_data(btn).map_or(0.0, |d| d.value());
                    let assist_val = assist.button_data(btn).map_or(0.0, |d| d.value());
                    let max_val = primary_val.max(assist_val);

                    helpers::create_trigger_event(max_val, abs_axis)
                };

                Some(vec![event])
            }

            EventType::AxisChanged(axis, _, _) => {
                let (x_axis, y_axis) = helpers::map_to_stick_pair(axis)?;

                // Check if assist is active on this stick
                let assist_active = helpers::is_stick_active(&assist, x_axis, y_axis);

                // If primary moved but assist is active, ignore
                if event.id == primary_id && assist_active {
                    return None;
                }

                // Determine owner and emit events for both axes
                let owner = if assist_active { assist } else { primary };

                let events = [x_axis, y_axis]
                    .into_iter()
                    .filter_map(|ax| {
                        let value = owner.axis_data(ax).map_or(0.0, |d| d.value());
                        helpers::create_stick_event(ax, value)
                    })
                    .collect::<Vec<_>>();

                (!events.is_empty()).then_some(events)
            }

            _ => None,
        }
    }
}
