//! Mux2 runtime: Graph-based input processing loop.
//!
//! This module contains the main runtime loop that:
//! 1. Polls gilrs for events from physical controllers
//! 2. Converts and routes them through MuxNode
//! 3. Writes output to virtual evdev device

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use gilrs::GamepadId;
use log::{debug, info};

use crate::core::drivers::gilrs_driver::{GilrsDriver, gilrs_event_to_ctrl_event};
use crate::core::drivers::sink::EvdevSink;
use crate::core::event::CtrlEvent;
use crate::core::node::{Node, PortId, ProcessContext, ports};
use crate::core::nodes::mux::{MuxModeType, MuxNode};
use crate::core::state::EdgeState;

/// Runtime settings that can be updated while running
pub struct Mux2RuntimeSettings {
    pub mode: MuxModeType,
}

impl Mux2RuntimeSettings {
    pub fn new(mode: MuxModeType) -> Self {
        Self { mode }
    }
}

/// Run the mux2 input processing loop.
///
/// This is the main event loop that:
/// - Polls gilrs for input events
/// - Routes them through the MuxNode
/// - Writes output to the virtual device
pub fn run_input_loop(
    mut driver: GilrsDriver,
    mut sink: EvdevSink,
    primary_id: GamepadId,
    assist_id: GamepadId,
    shutdown: Arc<AtomicBool>,
    settings: Arc<parking_lot::RwLock<Mux2RuntimeSettings>>,
) {
    info!("Starting mux2 input loop");

    // Create the MuxNode with initial mode
    let initial_mode = settings.read().mode;
    let mut mux_node = MuxNode::new(initial_mode);

    // Track EdgeState for each input port (simulating what GraphExecutor would do)
    let mut primary_state = EdgeState::new();
    let mut assist_state = EdgeState::new();

    // Empty node state (MuxNode tracks its own state)
    let mut node_state: Box<dyn std::any::Any + Send + Sync> = Box::new(());

    while !shutdown.load(Ordering::Relaxed) {
        // Check for mode changes
        let current_mode = settings.read().mode;
        if current_mode != mux_node.mode() {
            info!("Mode changed to {:?}", current_mode);
            mux_node = MuxNode::new(current_mode);
            // Reset states on mode change
            primary_state = EdgeState::new();
            assist_state = EdgeState::new();
        }

        // Poll for events (non-blocking)
        while let Some((gamepad_id, ctrl_event)) = driver.poll_event() {
            // Determine which port this event is for
            let input_port = if gamepad_id == primary_id {
                ports::MUX_PRIMARY_IN
            } else if gamepad_id == assist_id {
                ports::MUX_ASSIST_IN
            } else {
                continue; // Ignore other gamepads
            };

            // Update the EdgeState for this port BEFORE processing
            // (simulates what GraphExecutor does automatically)
            match input_port {
                p if p == ports::MUX_PRIMARY_IN => primary_state.apply(&ctrl_event),
                p if p == ports::MUX_ASSIST_IN => assist_state.apply(&ctrl_event),
                _ => {}
            }

            // Build input states map for ProcessContext
            let mut input_states = HashMap::new();
            input_states.insert(ports::MUX_PRIMARY_IN, primary_state.clone());
            input_states.insert(ports::MUX_ASSIST_IN, assist_state.clone());

            // Create ProcessContext
            let mut ctx = ProcessContext {
                input_states: &input_states,
                node_state: node_state.as_mut(),
            };

            // Process through MuxNode
            let outputs = mux_node.process(input_port, ctrl_event, &mut ctx);

            // Write outputs to sink
            for (_port, output_event) in outputs {
                if let Err(e) = sink.write_event(&output_event) {
                    debug!("Failed to write event: {}", e);
                }
            }
        }

        // Small sleep to avoid busy-waiting
        thread::sleep(Duration::from_micros(500));
    }

    info!("Mux2 input loop stopped");
}
