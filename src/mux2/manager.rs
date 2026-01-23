//! Mux2 manager: Handles device setup and runtime lifecycle.
//!
//! This module is responsible for:
//! - Creating the virtual gamepad device
//! - Setting up gilrs driver
//! - Spawning the input processing thread
//! - Spawning the FF forwarding thread
//! - Managing device hiding (optional)

use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread::{self, JoinHandle};

use gilrs::GamepadId;
use log::info;
use parking_lot::RwLock;

use crate::HideType;
use crate::core::drivers::gilrs_driver::GilrsDriver;

use crate::core::nodes::mux::MuxModeType;
use crate::mux2::runtime::{Mux2RuntimeSettings, run_ff_loop, run_input_loop};
use crate::utils::evdev::{VirtualGamepadInfo, create_virtual_gamepad};
use crate::utils::gilrs::{discover_gamepad_resources, new_gilrs, wait_for_virtual_device};
use crate::utils::hide::ScopedDeviceHider;

/// Handle returned from start_mux2 to allow controlling the running session
pub struct Mux2Handle {
    pub shutdown: Arc<AtomicBool>,
    pub settings: Arc<RwLock<Mux2RuntimeSettings>>,
    pub input_thread: JoinHandle<()>,
    pub ff_thread: Option<JoinHandle<()>>,
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
        .ok_or("Primary controller not found")?
        .clone();
    let assist_res = resources
        .get(&assist_id)
        .ok_or("Assist controller not found")?
        .clone();

    // Setup device hiding
    let mut hider = ScopedDeviceHider::new(hide_type.clone())?;
    hider.hide_gamepad_devices(&primary_res)?;
    hider.hide_gamepad_devices(&assist_res)?;

    // Create virtual gamepad info (use spoofed or default)
    let vgp_info = spoof_info.unwrap_or_else(|| VirtualGamepadInfo {
        name: "CtrlAssist Virtual Gamepad".to_string(),
        vendor_id: Some(0x045e),  // Xbox controller vendor
        product_id: Some(0x028e), // Xbox controller product
    });

    // Create virtual gamepad with FF support
    let mut virtual_device = create_virtual_gamepad(&vgp_info, Some("mux2"))?;
    info!("Created virtual gamepad: {}", vgp_info.name);

    // Wait for virtual device to be registered
    let _vdev_resource = wait_for_virtual_device(&mut virtual_device)?;
    info!("Virtual device ready");

    // Create shared state
    let shutdown = Arc::new(AtomicBool::new(false));
    let settings = Arc::new(RwLock::new(Mux2RuntimeSettings::new(mode)));

    // Split the virtual device - we need it for both input and FF
    // For FF we need a separate reference to fetch_events, but EvdevSink takes ownership
    // Solution: Create the sink from the VirtualDevice, but pass a clone of resource for FF

    // Clone resources for FF thread before moving them
    let primary_res_ff = primary_res.clone();
    let assist_res_ff = assist_res.clone();

    // For input loop - the EvdevSink forwards events TO the virtual device
    // We need to separate: VirtualDevice for writing (sink) vs reading FF events
    // Since both need the same VirtualDevice, we'll use Arc<Mutex> or pass raw handle
    //
    // Actually, the EvdevSink needs to own VirtualDevice for emit().
    // For FF, we need to call fetch_events() and process_ff_upload() on VirtualDevice.
    // These can't both own the VirtualDevice.
    //
    // Solution: Pass VirtualDevice to FF thread, create a secondary evdev Device
    // for writing (like the original mux does with v_dev vs v_uinput).

    // The original mux uses v_dev (from wait_for_virtual_device) for input sending
    // and v_uinput (the VirtualDevice) for FF handling. Let's do the same.

    // Get the device path from the virtual device resource
    let vdev_evdev = _vdev_resource.device;

    // Create sink from the evdev Device (for writing input events)
    let sink = EvdevSinkFromDevice::new(vdev_evdev);

    // Create drivers
    let driver = GilrsDriver::new(gilrs, vec![primary_id, assist_id]);

    // Clone for threads
    let shutdown_input = Arc::clone(&shutdown);
    let shutdown_ff = Arc::clone(&shutdown);
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
                shutdown_input,
                settings_clone,
            );
        })?;

    // Spawn FF processing thread
    let ff_thread = thread::Builder::new()
        .name("mux2-ff".to_string())
        .spawn(move || {
            let mut v_uinput = virtual_device;
            run_ff_loop(&mut v_uinput, primary_res_ff, assist_res_ff, shutdown_ff);
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
        ff_thread: Some(ff_thread),
        hider: hider_opt,
    })
}

/// Sink that writes events to an evdev Device (not VirtualDevice).
/// This is used when we need the VirtualDevice for FF processing
/// but still need to send input events to the virtual gamepad.
pub struct EvdevSinkFromDevice {
    device: evdev::Device,
}

impl EvdevSinkFromDevice {
    pub fn new(device: evdev::Device) -> Self {
        Self { device }
    }

    /// Write a CtrlEvent to the device
    pub fn write_event(
        &mut self,
        event: &crate::core::event::CtrlEvent,
    ) -> Result<(), std::io::Error> {
        use crate::core::drivers::sink::ctrl_event_to_evdev;
        use evdev::EventType;

        let evdev_events = ctrl_event_to_evdev(event);
        if !evdev_events.is_empty() {
            // Add sync event
            let mut all_events = evdev_events;
            all_events.push(evdev::InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0));
            self.device.send_events(&all_events)?;
        }
        Ok(())
    }
}
