//! MuxNode: Multiplexes two input streams into one output.
//!
//! This node receives events from two input ports (Primary and Assist)
//! and merges them according to the configured mode.

#[allow(unused_imports)]
use std::collections::HashMap;

use crate::core::event::{AxisId, ButtonId, CtrlEvent, InputEvent, InputKind};
use crate::core::node::{Node, NodePorts, PortDef, PortId, ProcessContext, ports};
use crate::core::state::EdgeState;

/// Mode for multiplexing inputs
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    clap::ValueEnum,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum MuxModeType {
    /// Assist input takes priority when active
    #[default]
    Priority,
    /// Average inputs from both controllers
    Average,
    /// Toggle between controllers with Mode button
    Toggle,
}

/// Mux node that combines two controller inputs into one.
pub struct MuxNode {
    mode: MuxModeType,
    /// State for Toggle mode: which input is currently active
    toggle_active_port: PortId,
}

impl MuxNode {
    /// Deadzone for stick activity detection
    const DEADZONE: f32 = 0.1;

    /// Create a new MuxNode with the specified mode
    pub fn new(mode: MuxModeType) -> Self {
        Self {
            mode,
            toggle_active_port: ports::MUX_PRIMARY_IN,
        }
    }

    /// Get the current mode
    pub fn mode(&self) -> MuxModeType {
        self.mode
    }

    /// Process with Priority mode: Assist overrides when active
    fn process_priority(
        &self,
        input_port: PortId,
        event: CtrlEvent,
        ctx: &ProcessContext,
    ) -> Vec<(PortId, CtrlEvent)> {
        let CtrlEvent::Input(ref input) = event else {
            return vec![];
        };

        let primary_state = ctx.state(ports::MUX_PRIMARY_IN);
        let assist_state = ctx.state(ports::MUX_ASSIST_IN);

        match &input.input {
            InputKind::Button { id, pressed: _ } => {
                // Check if assist is holding this button
                let assist_holding = assist_state.is_some_and(|s| s.is_pressed(*id));

                // Block primary's event if assist is holding
                if assist_holding && input_port == ports::MUX_PRIMARY_IN {
                    return vec![];
                }

                // Forward the button event
                vec![(ports::MUX_OUTPUT, event)]
            }

            InputKind::Axis { id, value: _ } => {
                match id {
                    // D-pad axes: Assist priority when active
                    AxisId::DPadX | AxisId::DPadY => {
                        let assist_val = assist_state.map(|s| s.axis_value(*id)).unwrap_or(0.0);
                        let primary_val = primary_state.map(|s| s.axis_value(*id)).unwrap_or(0.0);

                        let final_value = if assist_val.abs() > Self::DEADZONE {
                            assist_val
                        } else {
                            primary_val
                        };

                        vec![(
                            ports::MUX_OUTPUT,
                            CtrlEvent::Input(InputEvent::axis(*id, final_value)),
                        )]
                    }

                    // Triggers: Maximum value wins
                    AxisId::LeftTrigger | AxisId::RightTrigger => {
                        let assist_val = assist_state.map(|s| s.axis_value(*id)).unwrap_or(0.0);
                        let primary_val = primary_state.map(|s| s.axis_value(*id)).unwrap_or(0.0);
                        let max_val = primary_val.max(assist_val);

                        vec![(
                            ports::MUX_OUTPUT,
                            CtrlEvent::Input(InputEvent::axis(*id, max_val)),
                        )]
                    }

                    // Sticks: Assist priority when active (circular deadzone)
                    AxisId::LeftStickX | AxisId::LeftStickY => {
                        let assist_active = assist_state.is_some_and(|s| {
                            s.is_stick_active(
                                AxisId::LeftStickX,
                                AxisId::LeftStickY,
                                Self::DEADZONE,
                            )
                        });

                        // Block primary if assist is active
                        if input_port == ports::MUX_PRIMARY_IN && assist_active {
                            return vec![];
                        }

                        // Get values from the active controller
                        let (x_val, y_val) = if assist_active {
                            (
                                assist_state
                                    .map(|s| s.axis_value(AxisId::LeftStickX))
                                    .unwrap_or(0.0),
                                assist_state
                                    .map(|s| s.axis_value(AxisId::LeftStickY))
                                    .unwrap_or(0.0),
                            )
                        } else {
                            (
                                primary_state
                                    .map(|s| s.axis_value(AxisId::LeftStickX))
                                    .unwrap_or(0.0),
                                primary_state
                                    .map(|s| s.axis_value(AxisId::LeftStickY))
                                    .unwrap_or(0.0),
                            )
                        };

                        vec![
                            (
                                ports::MUX_OUTPUT,
                                CtrlEvent::Input(InputEvent::axis(AxisId::LeftStickX, x_val)),
                            ),
                            (
                                ports::MUX_OUTPUT,
                                CtrlEvent::Input(InputEvent::axis(AxisId::LeftStickY, y_val)),
                            ),
                        ]
                    }

                    AxisId::RightStickX | AxisId::RightStickY => {
                        let assist_active = assist_state.is_some_and(|s| {
                            s.is_stick_active(
                                AxisId::RightStickX,
                                AxisId::RightStickY,
                                Self::DEADZONE,
                            )
                        });

                        // Block primary if assist is active
                        if input_port == ports::MUX_PRIMARY_IN && assist_active {
                            return vec![];
                        }

                        // Get values from the active controller
                        let (x_val, y_val) = if assist_active {
                            (
                                assist_state
                                    .map(|s| s.axis_value(AxisId::RightStickX))
                                    .unwrap_or(0.0),
                                assist_state
                                    .map(|s| s.axis_value(AxisId::RightStickY))
                                    .unwrap_or(0.0),
                            )
                        } else {
                            (
                                primary_state
                                    .map(|s| s.axis_value(AxisId::RightStickX))
                                    .unwrap_or(0.0),
                                primary_state
                                    .map(|s| s.axis_value(AxisId::RightStickY))
                                    .unwrap_or(0.0),
                            )
                        };

                        vec![
                            (
                                ports::MUX_OUTPUT,
                                CtrlEvent::Input(InputEvent::axis(AxisId::RightStickX, x_val)),
                            ),
                            (
                                ports::MUX_OUTPUT,
                                CtrlEvent::Input(InputEvent::axis(AxisId::RightStickY, y_val)),
                            ),
                        ]
                    }
                }
            }
        }
    }

    /// Process with Average mode: Blend inputs from both controllers
    fn process_average(
        &self,
        _input_port: PortId,
        event: CtrlEvent,
        ctx: &ProcessContext,
    ) -> Vec<(PortId, CtrlEvent)> {
        let CtrlEvent::Input(ref input) = event else {
            return vec![];
        };

        let primary_state = ctx.state(ports::MUX_PRIMARY_IN);
        let assist_state = ctx.state(ports::MUX_ASSIST_IN);

        match &input.input {
            InputKind::Button { id, pressed: _ } => {
                // OR logic: either pressed = pressed
                let primary_pressed = primary_state.is_some_and(|s| s.is_pressed(*id));
                let assist_pressed = assist_state.is_some_and(|s| s.is_pressed(*id));
                let final_pressed = primary_pressed || assist_pressed;

                vec![(
                    ports::MUX_OUTPUT,
                    CtrlEvent::Input(InputEvent::button(*id, final_pressed)),
                )]
            }

            InputKind::Axis { id, value: _ } => {
                let primary_val = primary_state.map(|s| s.axis_value(*id)).unwrap_or(0.0);
                let assist_val = assist_state.map(|s| s.axis_value(*id)).unwrap_or(0.0);

                // Average when both active, otherwise use the active one
                let primary_active = primary_val.abs() > Self::DEADZONE;
                let assist_active = assist_val.abs() > Self::DEADZONE;

                let final_val = match (primary_active, assist_active) {
                    (true, true) => (primary_val + assist_val) / 2.0,
                    (true, false) => primary_val,
                    (false, true) => assist_val,
                    (false, false) => 0.0,
                };

                vec![(
                    ports::MUX_OUTPUT,
                    CtrlEvent::Input(InputEvent::axis(*id, final_val)),
                )]
            }
        }
    }

    /// Process with Toggle mode: Switch between controllers with Mode button
    fn process_toggle(
        &mut self,
        input_port: PortId,
        event: CtrlEvent,
        ctx: &ProcessContext,
    ) -> Vec<(PortId, CtrlEvent)> {
        let CtrlEvent::Input(ref input) = event else {
            return vec![];
        };

        // Check for Mode button press on assist to toggle
        if input_port == ports::MUX_ASSIST_IN {
            if let InputKind::Button {
                id: ButtonId::Mode,
                pressed: true,
            } = input.input
            {
                // Toggle active port
                self.toggle_active_port = if self.toggle_active_port == ports::MUX_PRIMARY_IN {
                    ports::MUX_ASSIST_IN
                } else {
                    ports::MUX_PRIMARY_IN
                };

                // Sync state from newly active controller
                let active_state = ctx.state(self.toggle_active_port);
                return self.sync_all_inputs(active_state);
            }
        }

        // Only forward events from the active port
        if input_port != self.toggle_active_port {
            return vec![];
        }

        vec![(ports::MUX_OUTPUT, event)]
    }

    /// Sync all inputs when toggling (emits current state of all buttons/axes)
    fn sync_all_inputs(&self, state: Option<&EdgeState>) -> Vec<(PortId, CtrlEvent)> {
        let Some(state) = state else {
            return vec![];
        };

        let mut events = Vec::new();

        // Sync all buttons
        for (id, data) in state.buttons() {
            events.push((
                ports::MUX_OUTPUT,
                CtrlEvent::Input(InputEvent::button(id, data.pressed)),
            ));
        }

        // Sync all axes
        for (id, data) in state.axes() {
            events.push((
                ports::MUX_OUTPUT,
                CtrlEvent::Input(InputEvent::axis(id, data.value)),
            ));
        }

        events
    }
}

impl Node for MuxNode {
    fn name(&self) -> &str {
        "MuxNode"
    }

    fn ports(&self) -> NodePorts {
        NodePorts {
            inputs: vec![
                PortDef::new(ports::MUX_PRIMARY_IN, "primary"),
                PortDef::new(ports::MUX_ASSIST_IN, "assist"),
            ],
            outputs: vec![PortDef::new(ports::MUX_OUTPUT, "output")],
        }
    }

    fn process(
        &mut self,
        input_port: PortId,
        event: CtrlEvent,
        ctx: &mut ProcessContext,
    ) -> Vec<(PortId, CtrlEvent)> {
        match self.mode {
            MuxModeType::Priority => self.process_priority(input_port, event, ctx),
            MuxModeType::Average => self.process_average(input_port, event, ctx),
            MuxModeType::Toggle => self.process_toggle(input_port, event, ctx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::state::EdgeState;

    fn make_context<'a>(
        primary: &'a EdgeState,
        assist: &'a EdgeState,
    ) -> (
        HashMap<PortId, EdgeState>,
        Box<dyn std::any::Any + Send + Sync>,
    ) {
        let mut states = HashMap::new();
        states.insert(ports::MUX_PRIMARY_IN, primary.clone());
        states.insert(ports::MUX_ASSIST_IN, assist.clone());
        (states, Box::new(()))
    }

    #[test]
    fn test_priority_button_assist_blocks_primary() {
        let mut mux = MuxNode::new(MuxModeType::Priority);

        // Assist is holding South button
        let mut assist_state = EdgeState::new();
        assist_state.apply(&CtrlEvent::Input(InputEvent::button(ButtonId::South, true)));

        let primary_state = EdgeState::new();

        let (states, mut node_state) = make_context(&primary_state, &assist_state);
        let mut ctx = ProcessContext {
            input_states: &states,
            node_state: node_state.as_mut(),
        };

        // Primary tries to press South - should be blocked
        let event = CtrlEvent::Input(InputEvent::button(ButtonId::South, true));
        let output = mux.process(ports::MUX_PRIMARY_IN, event, &mut ctx);

        assert!(
            output.is_empty(),
            "Primary button should be blocked when assist is holding"
        );
    }

    #[test]
    fn test_priority_button_assist_not_holding() {
        let mut mux = MuxNode::new(MuxModeType::Priority);

        // Both not holding
        let primary_state = EdgeState::new();
        let assist_state = EdgeState::new();

        let (states, mut node_state) = make_context(&primary_state, &assist_state);
        let mut ctx = ProcessContext {
            input_states: &states,
            node_state: node_state.as_mut(),
        };

        // Primary presses South - should pass through
        let event = CtrlEvent::Input(InputEvent::button(ButtonId::South, true));
        let output = mux.process(ports::MUX_PRIMARY_IN, event, &mut ctx);

        assert_eq!(output.len(), 1, "Button press should pass through");
    }

    #[test]
    fn test_toggle_mode_switch() {
        let mut mux = MuxNode::new(MuxModeType::Toggle);

        let primary_state = EdgeState::new();
        let assist_state = EdgeState::new();

        let (states, mut node_state) = make_context(&primary_state, &assist_state);
        let mut ctx = ProcessContext {
            input_states: &states,
            node_state: node_state.as_mut(),
        };

        // Initially primary is active, so primary events pass
        let event = CtrlEvent::Input(InputEvent::button(ButtonId::South, true));
        let output = mux.process(ports::MUX_PRIMARY_IN, event.clone(), &mut ctx);
        assert!(!output.is_empty(), "Primary should be active initially");

        // Assist events should be blocked
        let output = mux.process(ports::MUX_ASSIST_IN, event.clone(), &mut ctx);
        assert!(output.is_empty(), "Assist should be blocked initially");

        // Press Mode on assist to toggle
        let mode_event = CtrlEvent::Input(InputEvent::button(ButtonId::Mode, true));
        let _ = mux.process(ports::MUX_ASSIST_IN, mode_event, &mut ctx);

        // Now assist should be active
        let output = mux.process(ports::MUX_ASSIST_IN, event.clone(), &mut ctx);
        assert!(!output.is_empty(), "Assist should be active after toggle");
    }
}
