//! Mux2 manager: Handles device setup and runtime lifecycle.
//!
//! This module is responsible for:
//! - Creating the virtual gamepad device
//! - Setting up gilrs driver
//! - Spawning the input processing thread
//! - Managing device hiding (optional)

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};

use gilrs::GamepadId;
use log::info;
use parking_lot::RwLock;

use crate::HideType;
use crate::core::drivers::gilrs_driver::GilrsDriver;
use crate::core::drivers::sink::EvdevSink;
use crate::core::nodes::mux::MuxModeType;
use crate::mux2::runtime::{Mux2RuntimeSettings, run_input_loop};
use crate::utils::evdev::{VirtualGamepadInfo, create_virtual_gamepad};
use crate::utils::gilrs::{discover_gamepad_resources, new_gilrs, wait_for_virtual_device};
use crate::utils::hide::ScopedDeviceHider;

/// Handle returned from start_mux2 to allow controlling the running session
pub struct Mux2Handle {
    pub shutdown: Arc<AtomicBool>,
    pub settings: Arc<RwLock<Mux2RuntimeSettings>>,
    pub input_thread: JoinHandle<()>,
    // Keep the hider alive so it restores permissions on drop
    #[allow(dead_code)]
    hider: Option<ScopedDeviceHider>,
}

/// Start a mux2 session with the graph-based architecture.
///
/// This is the main entry point for the mux2 functionality.
pub fn start_mux2(
    primary_id: GamepadId,
    assist_id: GamepadId,
    mode: MuxModeType,
    hide_type: HideType,
    spoof_info: Option<VirtualGamepadInfo>,
) -> Result<Mux2Handle, Box<dyn Error>> {
    info!("Starting mux2 with mode {:?}", mode);

    // Initialize gilrs
    let gilrs = new_gilrs()?;

    // Discover device paths
    let resources = discover_gamepad_resources(&gilrs);
    let primary_res = resources
        .get(&primary_id)
        .ok_or("Primary controller not found")?;
    let assist_res = resources
        .get(&assist_id)
        .ok_or("Assist controller not found")?;

    // Setup device hiding
    let mut hider = ScopedDeviceHider::new(hide_type.clone())?;
    hider.hide_gamepad_devices(primary_res)?;
    hider.hide_gamepad_devices(assist_res)?;

    // Create virtual gamepad info (use spoofed or default)
    let vgp_info = spoof_info.unwrap_or_else(|| VirtualGamepadInfo {
        name: "CtrlAssist Virtual Gamepad".to_string(),
        vendor_id: Some(0x045e),  // Xbox controller vendor
        product_id: Some(0x028e), // Xbox controller product
    });

    // Create virtual gamepad
    let mut virtual_device = create_virtual_gamepad(&vgp_info, Some("mux2"))?;
    info!("Created virtual gamepad: {}", vgp_info.name);

    // Wait for virtual device to be registered
    let _vdev_resource = wait_for_virtual_device(&mut virtual_device)?;
    info!("Virtual device ready");

    // Create drivers
    let driver = GilrsDriver::new(gilrs, vec![primary_id, assist_id]);
    let sink = EvdevSink::new(virtual_device);

    // Create shared state
    let shutdown = Arc::new(AtomicBool::new(false));
    let settings = Arc::new(RwLock::new(Mux2RuntimeSettings::new(mode)));

    // Clone for thread
    let shutdown_clone = Arc::clone(&shutdown);
    let settings_clone = Arc::clone(&settings);

    // Spawn input processing thread
    let input_thread = thread::Builder::new()
        .name("mux2-input".to_string())
        .spawn(move || {
            run_input_loop(
                driver,
                sink,
                primary_id,
                assist_id,
                shutdown_clone,
                settings_clone,
            );
        })?;

    // Keep hider alive if we're actually hiding
    let hider_opt = if hide_type == HideType::None {
        None
    } else {
        Some(hider)
    };

    Ok(Mux2Handle {
        shutdown,
        settings,
        input_thread,
        hider: hider_opt,
    })
}
