//! Edge state for caching gamepad state on graph edges.
//!
//! Inspired by gilrs's `GamepadState`, this provides a snapshot
//! of button and axis states that nodes can query for modes like
//! Priority (needs to know "is Assist stick active?").

use crate::core::event::{AxisId, ButtonId, CtrlEvent, InputKind};
use std::collections::HashMap;

/// Cached state on a graph edge, similar to gilrs GamepadState.
///
/// As events flow through an edge, the executor automatically
/// updates this state. Downstream nodes can query it to make
/// decisions (e.g., "is the other controller's stick active?").
#[derive(Debug, Clone, Default)]
pub struct EdgeState {
    buttons: HashMap<ButtonId, ButtonData>,
    axes: HashMap<AxisId, AxisData>,
}

/// Button state data
#[derive(Debug, Clone, Copy, Default)]
pub struct ButtonData {
    pub pressed: bool,
    pub value: f32, // For analog buttons (0.0 to 1.0)
}

/// Axis state data
#[derive(Debug, Clone, Copy, Default)]
pub struct AxisData {
    pub value: f32, // Normalized: -1.0 to 1.0 or 0.0 to 1.0
}

impl EdgeState {
    /// Create a new empty state
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply an event to update the state
    pub fn apply(&mut self, event: &CtrlEvent) {
        if let CtrlEvent::Input(input) = event {
            match &input.input {
                InputKind::Button { id, pressed } => {
                    self.buttons.insert(
                        *id,
                        ButtonData {
                            pressed: *pressed,
                            value: if *pressed { 1.0 } else { 0.0 },
                        },
                    );
                }
                InputKind::Axis { id, value } => {
                    self.axes.insert(*id, AxisData { value: *value });
                }
            }
        }
    }

    /// Check if a button is pressed
    pub fn is_pressed(&self, button: ButtonId) -> bool {
        self.buttons.get(&button).is_some_and(|b| b.pressed)
    }

    /// Get button data
    pub fn button_data(&self, button: ButtonId) -> Option<ButtonData> {
        self.buttons.get(&button).copied()
    }

    /// Get axis value (returns 0.0 if not set)
    pub fn axis_value(&self, axis: AxisId) -> f32 {
        self.axes.get(&axis).map(|a| a.value).unwrap_or(0.0)
    }

    /// Get axis data
    pub fn axis_data(&self, axis: AxisId) -> Option<AxisData> {
        self.axes.get(&axis).copied()
    }

    /// Check if a stick is active (beyond deadzone) using circular deadzone
    pub fn is_stick_active(&self, x_axis: AxisId, y_axis: AxisId, deadzone: f32) -> bool {
        let x = self.axis_value(x_axis);
        let y = self.axis_value(y_axis);
        (x * x + y * y).sqrt() > deadzone
    }

    /// Reset all state to default
    pub fn reset(&mut self) {
        self.buttons.clear();
        self.axes.clear();
    }

    /// Iterator over all buttons
    pub fn buttons(&self) -> impl Iterator<Item = (ButtonId, ButtonData)> + '_ {
        self.buttons.iter().map(|(k, v)| (*k, *v))
    }

    /// Iterator over all axes
    pub fn axes(&self) -> impl Iterator<Item = (AxisId, AxisData)> + '_ {
        self.axes.iter().map(|(k, v)| (*k, *v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::event::InputEvent;

    #[test]
    fn test_button_state() {
        let mut state = EdgeState::new();

        // Initially not pressed
        assert!(!state.is_pressed(ButtonId::South));

        // Press button
        let event = CtrlEvent::Input(InputEvent::button(ButtonId::South, true));
        state.apply(&event);
        assert!(state.is_pressed(ButtonId::South));

        // Release button
        let event = CtrlEvent::Input(InputEvent::button(ButtonId::South, false));
        state.apply(&event);
        assert!(!state.is_pressed(ButtonId::South));
    }

    #[test]
    fn test_axis_state() {
        let mut state = EdgeState::new();

        // Initially zero
        assert_eq!(state.axis_value(AxisId::LeftStickX), 0.0);

        // Move stick
        let event = CtrlEvent::Input(InputEvent::axis(AxisId::LeftStickX, 0.75));
        state.apply(&event);
        assert_eq!(state.axis_value(AxisId::LeftStickX), 0.75);
    }

    #[test]
    fn test_stick_deadzone() {
        let mut state = EdgeState::new();
        let deadzone = 0.1;

        // Within deadzone
        state.apply(&CtrlEvent::Input(InputEvent::axis(
            AxisId::LeftStickX,
            0.05,
        )));
        state.apply(&CtrlEvent::Input(InputEvent::axis(
            AxisId::LeftStickY,
            0.05,
        )));
        assert!(!state.is_stick_active(AxisId::LeftStickX, AxisId::LeftStickY, deadzone));

        // Outside deadzone
        state.apply(&CtrlEvent::Input(InputEvent::axis(AxisId::LeftStickX, 0.5)));
        assert!(state.is_stick_active(AxisId::LeftStickX, AxisId::LeftStickY, deadzone));
    }
}
