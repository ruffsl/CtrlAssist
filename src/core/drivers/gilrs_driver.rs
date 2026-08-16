//! GilrsDriver: Converts gilrs events to CtrlEvents and polls for new input.
//!
//! This driver bridges gilrs (the gamepad input library) with the graph architecture.
//! It handles:
//! - Event polling from physical controllers
//! - Conversion from gilrs types to our normalized CtrlEvent types
//! - Device state tracking for connected gamepads

use gilrs::{Axis, Button, Event, EventType, GamepadId, Gilrs};

use crate::core::event::{AxisId, ButtonId, CtrlEvent, DeviceId, InputEvent};

/// Converts a gilrs GamepadId to our DeviceId
pub fn gamepad_id_to_device_id(id: GamepadId) -> DeviceId {
    // GamepadId doesn't implement Into<u32>, so use usize conversion
    DeviceId(usize::from(id) as u32)
}

/// Converts a gilrs Button to our ButtonId
pub fn gilrs_button_to_button_id(button: Button) -> Option<ButtonId> {
    match button {
        Button::South => Some(ButtonId::South),
        Button::East => Some(ButtonId::East),
        Button::North => Some(ButtonId::North),
        Button::West => Some(ButtonId::West),
        Button::LeftTrigger => Some(ButtonId::LeftBumper),
        Button::RightTrigger => Some(ButtonId::RightBumper),
        Button::LeftTrigger2 => Some(ButtonId::LeftTriggerButton),
        Button::RightTrigger2 => Some(ButtonId::RightTriggerButton),
        Button::Select => Some(ButtonId::Select),
        Button::Start => Some(ButtonId::Start),
        Button::Mode => Some(ButtonId::Mode),
        Button::LeftThumb => Some(ButtonId::LeftThumb),
        Button::RightThumb => Some(ButtonId::RightThumb),
        Button::DPadUp => Some(ButtonId::DPadUp),
        Button::DPadDown => Some(ButtonId::DPadDown),
        Button::DPadLeft => Some(ButtonId::DPadLeft),
        Button::DPadRight => Some(ButtonId::DPadRight),
        Button::Unknown | Button::C | Button::Z => None,
    }
}

/// Converts a gilrs Axis to our AxisId
pub fn gilrs_axis_to_axis_id(axis: Axis) -> Option<AxisId> {
    match axis {
        Axis::LeftStickX => Some(AxisId::LeftStickX),
        Axis::LeftStickY => Some(AxisId::LeftStickY),
        Axis::RightStickX => Some(AxisId::RightStickX),
        Axis::RightStickY => Some(AxisId::RightStickY),
        Axis::LeftZ => Some(AxisId::LeftTrigger),
        Axis::RightZ => Some(AxisId::RightTrigger),
        Axis::DPadX => Some(AxisId::DPadX),
        Axis::DPadY => Some(AxisId::DPadY),
        Axis::Unknown => None,
    }
}

/// Converts a gilrs Event to a CtrlEvent, if applicable.
///
/// Returns None for events we don't care about (e.g., Unknown buttons,
/// connected/disconnected events, etc.)
pub fn gilrs_event_to_ctrl_event(event: &Event) -> Option<CtrlEvent> {
    let device_id = gamepad_id_to_device_id(event.id);

    match event.event {
        EventType::ButtonPressed(btn, _) => {
            let button_id = gilrs_button_to_button_id(btn)?;
            Some(CtrlEvent::Input(
                InputEvent::button(button_id, true).with_source(device_id),
            ))
        }

        EventType::ButtonReleased(btn, _) => {
            let button_id = gilrs_button_to_button_id(btn)?;
            Some(CtrlEvent::Input(
                InputEvent::button(button_id, false).with_source(device_id),
            ))
        }

        EventType::ButtonChanged(btn, value, _) => {
            // ButtonChanged is used for analog triggers and D-pad
            // We convert these to axis events with the value
            match btn {
                Button::LeftTrigger2 => Some(CtrlEvent::Input(
                    InputEvent::axis(AxisId::LeftTrigger, value).with_source(device_id),
                )),
                Button::RightTrigger2 => Some(CtrlEvent::Input(
                    InputEvent::axis(AxisId::RightTrigger, value).with_source(device_id),
                )),
                Button::DPadUp | Button::DPadDown => {
                    // D-pad Y: Up is positive, Down is negative
                    let dpad_y = if btn == Button::DPadUp { value } else { -value };
                    Some(CtrlEvent::Input(
                        InputEvent::axis(AxisId::DPadY, dpad_y).with_source(device_id),
                    ))
                }
                Button::DPadLeft | Button::DPadRight => {
                    // D-pad X: Right is positive, Left is negative
                    let dpad_x = if btn == Button::DPadRight {
                        value
                    } else {
                        -value
                    };
                    Some(CtrlEvent::Input(
                        InputEvent::axis(AxisId::DPadX, dpad_x).with_source(device_id),
                    ))
                }
                _ => None,
            }
        }

        EventType::AxisChanged(axis, value, _) => {
            let axis_id = gilrs_axis_to_axis_id(axis)?;
            Some(CtrlEvent::Input(
                InputEvent::axis(axis_id, value).with_source(device_id),
            ))
        }

        // We don't handle connected/disconnected/dropped events here
        // Wildcard needed because gilrs::EventType is non-exhaustive
        _ => None,
    }
}

/// Driver that polls gilrs for events and converts them to CtrlEvents.
///
/// This is not a Node itself, but provides the polling mechanism that
/// feeds events into the graph.
pub struct GilrsDriver {
    gilrs: Gilrs,
    /// Filter to only accept events from specific gamepads
    accepted_ids: Vec<GamepadId>,
}

impl GilrsDriver {
    /// Create a new driver for the given gilrs instance
    pub fn new(gilrs: Gilrs, accepted_ids: Vec<GamepadId>) -> Self {
        Self {
            gilrs,
            accepted_ids,
        }
    }

    /// Poll for the next event, blocking with timeout.
    /// Returns (GamepadId, CtrlEvent) if an event is available and from an accepted gamepad.
    pub fn poll_event(&mut self) -> Option<(GamepadId, CtrlEvent)> {
        loop {
            // Poll with a short timeout
            match self.gilrs.next_event() {
                Some(event) => {
                    // Filter by accepted IDs
                    if !self.accepted_ids.contains(&event.id) {
                        continue;
                    }

                    // Convert to CtrlEvent
                    if let Some(ctrl_event) = gilrs_event_to_ctrl_event(&event) {
                        return Some((event.id, ctrl_event));
                    }
                    // If conversion failed (e.g., Unknown button), try next event
                    continue;
                }
                None => return None,
            }
        }
    }

    /// Non-blocking poll - returns immediately if no event
    pub fn try_poll_event(&mut self) -> Option<(GamepadId, CtrlEvent)> {
        self.poll_event()
    }

    /// Blocking poll with optional timeout.
    /// Returns (GamepadId, CtrlEvent) if an event is available from an accepted gamepad.
    /// Returns None if timeout expires (or immediately if timeout is None and no event).
    pub fn poll_event_blocking(
        &mut self,
        timeout: Option<std::time::Duration>,
    ) -> Option<(GamepadId, CtrlEvent)> {
        loop {
            match self.gilrs.next_event_blocking(timeout) {
                Some(event) => {
                    // Filter by accepted IDs
                    if !self.accepted_ids.contains(&event.id) {
                        continue;
                    }

                    // Convert to CtrlEvent
                    if let Some(ctrl_event) = gilrs_event_to_ctrl_event(&event) {
                        return Some((event.id, ctrl_event));
                    }
                    // If conversion failed (e.g., Unknown button), try next event
                    continue;
                }
                None => return None,
            }
        }
    }

    /// Get a reference to the underlying gilrs instance
    pub fn gilrs(&self) -> &Gilrs {
        &self.gilrs
    }

    /// Get a mutable reference to the underlying gilrs instance
    pub fn gilrs_mut(&mut self) -> &mut Gilrs {
        &mut self.gilrs
    }
}
