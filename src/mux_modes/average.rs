use super::{helpers, MuxMode};
use crate::evdev_helpers;
use evdev::InputEvent;
use gilrs::{Button, Event, EventType, GamepadId, Gilrs};

#[derive(Default)]
pub struct AverageMode;

impl MuxMode for AverageMode {
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

                // Check if the other controller is holding this button
                let other_holding = if event.id == primary_id {
                    assist.is_pressed(btn)
                } else {
                    primary.is_pressed(btn)
                };

                // If either is still holding, block this event (OR logic)
                if other_holding {
                    return None;
                }

                helpers::create_button_key_event(btn, is_pressed).map(|e| vec![e])
            }

            EventType::ButtonChanged(btn, _, _) => {
                let abs_axis = evdev_helpers::gilrs_button_to_evdev_axis(btn)?;

                let event = if let Some([neg_btn, pos_btn]) = evdev_helpers::dpad_axis_pair(btn) {
                    // D-pad: Average active values
                    let assist_net = helpers::calculate_dpad_net_value(&assist, neg_btn, pos_btn);
                    let primary_net = helpers::calculate_dpad_net_value(&primary, neg_btn, pos_btn);

                    let final_value = match (
                        assist_net.abs() > helpers::DEADZONE,
                        primary_net.abs() > helpers::DEADZONE,
                    ) {
                        (true, true) => primary_net + assist_net,
                        (true, false) => assist_net,
                        (false, _) => primary_net,
                    };

                    helpers::create_dpad_event(final_value, neg_btn, pos_btn, abs_axis)
                } else {
                    // Trigger: Average active values
                    let primary_val = primary.button_data(btn).map_or(0.0, |d| d.value());
                    let assist_val = assist.button_data(btn).map_or(0.0, |d| d.value());

                    let final_value = match (
                        assist_val > helpers::DEADZONE,
                        primary_val > helpers::DEADZONE,
                    ) {
                        (true, true) => (primary_val + assist_val) / 2.0,
                        (true, false) => assist_val,
                        (false, _) => primary_val,
                    };

                    helpers::create_trigger_event(final_value, abs_axis)
                };

                Some(vec![event])
            }

            EventType::AxisChanged(axis, _, _) => {
                let (x_axis, y_axis) = helpers::map_to_stick_pair(axis)?;

                // Check activity on both sticks
                let assist_active = helpers::is_stick_active(&assist, x_axis, y_axis);
                let primary_active = helpers::is_stick_active(&primary, x_axis, y_axis);

                // Calculate final values
                let (final_x, final_y) = {
                    let assist_x = assist.axis_data(x_axis).map_or(0.0, |d| d.value());
                    let assist_y = assist.axis_data(y_axis).map_or(0.0, |d| d.value());
                    let primary_x = primary.axis_data(x_axis).map_or(0.0, |d| d.value());
                    let primary_y = primary.axis_data(y_axis).map_or(0.0, |d| d.value());

                    match (assist_active, primary_active) {
                        (true, true) => ((primary_x + assist_x) / 2.0, (primary_y + assist_y) / 2.0),
                        (true, false) => (assist_x, assist_y),
                        (false, _) => (primary_x, primary_y),
                    }
                };

                // Emit events for both axes
                let events = [(x_axis, final_x), (y_axis, final_y)]
                    .into_iter()
                    .filter_map(|(ax, val)| helpers::create_stick_event(ax, val))
                    .collect::<Vec<_>>();

                (!events.is_empty()).then_some(events)
            }

            _ => None,
        }
    }
}
