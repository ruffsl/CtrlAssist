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

use crate::core::drivers::gilrs_driver::GilrsDriver;
use crate::core::node::{Node, ProcessContext, ports};
use crate::core::nodes::mux::{MuxModeType, MuxNode};
use crate::core::state::EdgeState;
use crate::mux2::manager::EvdevSinkFromDevice;

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
    mut sink: EvdevSinkFromDevice,
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

/// Run the Force Feedback forwarding loop.
///
/// This loop:
/// - Reads FF events from the virtual device (Upload, Erase, Play, Stop)
/// - Forwards them to the physical controllers based on rumble target settings
pub fn run_ff_loop(
    v_uinput: &mut evdev::uinput::VirtualDevice,
    primary_resource: crate::utils::gilrs::GamepadResource,
    assist_resource: crate::utils::gilrs::GamepadResource,
    shutdown: Arc<AtomicBool>,
) {
    use crate::utils::ff::{EffectManager, PhysicalFFDev};
    use log::error;

    // Centralized effect state
    let mut effect_manager = EffectManager::new();

    // Initialize physical FF devices (both for now - could add rumble target later)
    let mut phys_devs: Vec<PhysicalFFDev> = vec![];

    // Only add devices that support FF
    if primary_resource.device.supported_ff().is_some() {
        phys_devs.push(PhysicalFFDev::new(primary_resource));
    }
    if assist_resource.device.supported_ff().is_some() {
        phys_devs.push(PhysicalFFDev::new(assist_resource));
    }

    if phys_devs.is_empty() {
        info!("No FF-capable devices found, FF loop disabled");
        return;
    }

    info!("FF Thread started with {} device(s)", phys_devs.len());

    while !shutdown.load(Ordering::SeqCst) {
        // Process events from virtual device
        let events: Vec<_> = match v_uinput.fetch_events() {
            Ok(iter) => iter.collect(),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => vec![],
            Err(e) => {
                error!("Error fetching FF events: {}", e);
                vec![]
            }
        };

        for event in events {
            match event.destructure() {
                evdev::EventSummary::UInput(ev, evdev::UInputCode::UI_FF_UPLOAD, ..) => {
                    if let Ok(upload_ev) = v_uinput.process_ff_upload(ev) {
                        let virt_id = upload_ev.effect_id();
                        let effect_data = upload_ev.effect();

                        // Record in manager
                        effect_manager.upload(virt_id, effect_data);

                        // Upload to all current devices
                        for dev in &mut phys_devs {
                            if let Err(e) = dev.upload_effect(virt_id, effect_data) {
                                error!(
                                    "Failed to upload effect {} to {}: {}",
                                    virt_id,
                                    dev.resource.path.display(),
                                    e
                                );
                            }
                        }
                    }
                }

                evdev::EventSummary::UInput(ev, evdev::UInputCode::UI_FF_ERASE, ..) => {
                    if let Ok(erase_ev) = v_uinput.process_ff_erase(ev) {
                        let virt_id = erase_ev.effect_id() as i16;

                        // Stop and remove from all devices
                        for dev in &mut phys_devs {
                            if let Err(e) = dev.erase_effect(virt_id) {
                                error!(
                                    "Failed to erase effect {} from {}: {}",
                                    virt_id,
                                    dev.resource.path.display(),
                                    e
                                );
                            }
                        }

                        // Remove from manager
                        effect_manager.erase(virt_id);
                    }
                }

                evdev::EventSummary::ForceFeedback(_, effect_id, status) => {
                    let virt_id = effect_id.0 as i16;
                    let is_playing = status == evdev::FFStatusCode::FF_STATUS_PLAYING.0 as i32;

                    // Update manager state
                    effect_manager.set_playing(virt_id, is_playing);

                    // Apply to all devices
                    for dev in &mut phys_devs {
                        if let Err(e) = dev.control_effect(virt_id, is_playing) {
                            // Log but don't crash on ENODEV (device disconnected)
                            if e.raw_os_error() != Some(libc::ENODEV) {
                                error!(
                                    "Failed to control effect {} on {}: {}",
                                    virt_id,
                                    dev.resource.path.display(),
                                    e
                                );
                            }
                        }
                    }
                }

                _ => {
                    // Ignore other events
                }
            }
        }

        // Sleep briefly to avoid busy-waiting
        thread::sleep(Duration::from_millis(1));
    }

    info!("Mux2 FF loop stopped");
}
