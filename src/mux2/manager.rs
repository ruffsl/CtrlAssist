//! Graph-based mux manager and runtime.
//!
//! This module implements the mux functionality using the new graph
//! architecture with MuxNode, GilrsDriver, and EvdevSink.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gilrs::{GamepadId, Gilrs};
use log::{debug, error, info};

use crate::HideType;
use crate::SpoofTarget;
use crate::core::drivers::gilrs_driver::{GilrsDriver, gilrs_event_to_ctrl_event};
use crate::core::drivers::sink::EvdevSink;
use crate::core::event::CtrlEvent;
use crate::core::node::{PortId, ProcessContext, ports};
use crate::core::nodes::mux::{MuxModeType, MuxNode};
use crate::core::state::EdgeState;
use crate::utils::evdev::create_virtual_gamepad;

/// Configuration for mux2
pub struct Mux2Config {
    pub primary_id: GamepadId,
    pub assist_id: GamepadId,
    pub mode: MuxModeType,
    pub hide: HideType,
    pub spoof: SpoofTarget,
}

/// Handle to control a running mux2 session
pub struct Mux2Handle {
    shutdown: Arc<AtomicBool>,
}

impl Mux2Handle {
    /// Signal the mux2 session to shut down
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

/// Start the graph-based mux session.
///
/// This creates:
/// - A MuxNode with the configured mode
/// - A GilrsDriver to poll physical controllers
/// - An EvdevSink to write to the virtual device
///
/// Returns a handle to control the session.
pub fn start_mux2(
    gilrs: Gilrs,
    config: Mux2Config,
) -> Result<Mux2Handle, Box<dyn std::error::Error + Send + Sync>> {
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_clone = shutdown.clone();

    // Set up device hiding if requested
    let hide_guard = setup_hiding(&gilrs, &config)?;

    // Create the virtual device
    let spoof_name = match config.spoof {
        SpoofTarget::Primary => Some(gilrs.gamepad(config.primary_id).name().to_string()),
        SpoofTarget::Assist => Some(gilrs.gamepad(config.assist_id).name().to_string()),
        SpoofTarget::None => None,
    };

    let v_dev = create_virtual_gamepad(spoof_name.as_deref(), true)?;
    let dev_path = v_dev
        .get_syspath()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    info!("Created virtual device at {}", dev_path);
    println!("Virtual: {}", dev_path);

    // Run the main loop in the current thread
    run_mux2_loop(gilrs, config, v_dev, shutdown_clone)?;

    // Clean up hiding
    drop(hide_guard);

    Ok(Mux2Handle { shutdown })
}

/// Set up device hiding based on configuration
fn setup_hiding(
    gilrs: &Gilrs,
    config: &Mux2Config,
) -> Result<Option<Box<dyn std::any::Any>>, Box<dyn std::error::Error + Send + Sync>> {
    match config.hide {
        HideType::None => Ok(None),
        HideType::Steam => {
            let resources = crate::utils::gilrs::discover_gamepad_resources(gilrs);
            let paths: Vec<_> = [config.primary_id, config.assist_id]
                .iter()
                .filter_map(|id| resources.get(id).map(|r| r.path.clone()))
                .collect();
            crate::utils::hide::hide_devices_steam(&paths)?;
            // Return a guard that will restore on drop
            Ok(Some(Box::new(SteamHideGuard)))
        }
        HideType::System => {
            let resources = crate::utils::gilrs::discover_gamepad_resources(gilrs);
            let paths: Vec<_> = [config.primary_id, config.assist_id]
                .iter()
                .filter_map(|id| resources.get(id).map(|r| r.path.clone()))
                .collect();
            let guard = crate::utils::hide::hide_devices_system(&paths)?;
            Ok(Some(Box::new(guard)))
        }
    }
}

/// Placeholder guard for Steam hiding (no cleanup needed)
struct SteamHideGuard;

/// Main mux2 event loop using the graph architecture
fn run_mux2_loop(
    gilrs: Gilrs,
    config: Mux2Config,
    v_dev: evdev::uinput::VirtualDevice,
    shutdown: Arc<AtomicBool>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Create the components
    let mut driver = GilrsDriver::new(gilrs, vec![config.primary_id, config.assist_id]);
    let mut sink = EvdevSink::new(v_dev);
    let mut mux_node = MuxNode::new(config.mode);

    // Create edge states for primary and assist inputs
    let mut primary_state = EdgeState::new();
    let mut assist_state = EdgeState::new();

    info!("Starting mux2 event loop with mode: {:?}", config.mode);
    println!("Mux2 active. Press Ctrl+C to stop.");

    // Main event loop
    loop {
        if shutdown.load(Ordering::SeqCst) {
            info!("Shutdown signal received");
            break;
        }

        // Poll for events from gilrs
        if let Some((gamepad_id, ctrl_event)) = driver.try_poll_event() {
            // Determine which port this event came from
            let (input_port, edge_state) = if gamepad_id == config.primary_id {
                (ports::MUX_PRIMARY_IN, &mut primary_state)
            } else {
                (ports::MUX_ASSIST_IN, &mut assist_state)
            };

            // Update edge state with the incoming event
            edge_state.apply(&ctrl_event);

            // Build input states map for ProcessContext
            let mut input_states = HashMap::new();
            input_states.insert(ports::MUX_PRIMARY_IN, primary_state.clone());
            input_states.insert(ports::MUX_ASSIST_IN, assist_state.clone());

            // Create context and process
            let mut node_state: Box<dyn std::any::Any + Send + Sync> = Box::new(());
            let mut ctx = ProcessContext {
                input_states: &input_states,
                node_state: node_state.as_mut(),
            };

            let outputs = mux_node.process(input_port, ctrl_event, &mut ctx);

            // Write outputs to the virtual device
            for (_, output_event) in outputs {
                if let Err(e) = sink.write_event(&output_event) {
                    error!("Failed to write event: {}", e);
                }
            }
        } else {
            // No event available, sleep briefly to avoid busy-waiting
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    info!("Mux2 event loop exited");
    Ok(())
}
