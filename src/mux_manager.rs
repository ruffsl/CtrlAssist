use crate::evdev_helpers::{self, VirtualGamepadInfo};
use crate::gilrs_helper::{self};
use crate::mux_modes::ModeType;
use crate::udev_helpers::ScopedDeviceHider;
use crate::{HideType, RumbleTarget, SpoofTarget};
use evdev::Device;
use gilrs::{GamepadId, Gilrs};
use log::info;
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;

/// Configuration for starting a mux session
pub struct MuxConfig {
    pub primary_id: GamepadId,
    pub assist_id: GamepadId,
    pub mode: ModeType,
    pub hide: HideType,
    pub spoof: SpoofTarget,
    pub rumble: RumbleTarget,
}

/// Handle to a running mux session
pub struct MuxHandle {
    pub input_handle: thread::JoinHandle<()>,
    pub ff_handle: thread::JoinHandle<()>,
    pub shutdown: Arc<AtomicBool>,
    pub virtual_device_path: PathBuf,
}

impl MuxHandle {
    /// Request shutdown and wait for threads to complete
    pub fn shutdown(self) {
        use std::sync::atomic::Ordering;

        self.shutdown.store(true, Ordering::SeqCst);

        // Unblock FF thread by sending no-op event
        if let Ok(mut vdev) = Device::open(&self.virtual_device_path) {
            use evdev::{EventType, InputEvent};
            let _ = vdev.send_events(&[
                InputEvent::new(EventType::FORCEFEEDBACK.0, 0, 0),
                InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
            ]);
        }

        let _ = self.input_handle.join();
        let _ = self.ff_handle.join();
    }
}

/// Start a mux session with the given configuration
///
/// This function:
/// 1. Sets up device hiding
/// 2. Creates the virtual gamepad
/// 3. Prepares FF targets
/// 4. Spawns input and FF threads
/// 5. Returns a handle for managing the session
pub fn start_mux(gilrs: Gilrs, config: MuxConfig) -> Result<MuxHandle, Box<dyn Error>> {
    let mut resources = gilrs_helper::discover_gamepad_resources(&gilrs);

    // Setup hiding
    let mut _hider = ScopedDeviceHider::new(config.hide.clone());
    if let Some(primary_res) = resources.get(&config.primary_id) {
        _hider.hide_gamepad_devices(primary_res)?;
    }
    if let Some(assist_res) = resources.get(&config.assist_id) {
        _hider.hide_gamepad_devices(assist_res)?;
    }

    // Setup virtual device
    let virtual_info = match config.spoof {
        SpoofTarget::Primary => VirtualGamepadInfo::from(&gilrs.gamepad(config.primary_id)),
        SpoofTarget::Assist => VirtualGamepadInfo::from(&gilrs.gamepad(config.assist_id)),
        SpoofTarget::None => VirtualGamepadInfo {
            name: "CtrlAssist Virtual Gamepad".into(),
            vendor_id: None,
            product_id: None,
        },
    };

    let mut v_uinput = evdev_helpers::create_virtual_gamepad(&virtual_info)?;
    let v_resource = gilrs_helper::wait_for_virtual_device(&mut v_uinput)?;
    let virtual_device_path = v_resource.path.clone();

    info!(
        "Virtual: {} @ {}",
        v_resource.name,
        v_resource.path.display()
    );

    // Prepare FF targets
    let mut ff_targets = Vec::new();
    let rumble_ids = match config.rumble {
        RumbleTarget::Primary => vec![config.primary_id],
        RumbleTarget::Assist => vec![config.assist_id],
        RumbleTarget::Both => vec![config.primary_id, config.assist_id],
        RumbleTarget::None => vec![],
    };

    for id in rumble_ids {
        if let Some(res) = resources.remove(&id) {
            ff_targets.push(res);
        }
    }

    // Setup shutdown signal
    let shutdown = Arc::new(AtomicBool::new(false));

    // Spawn input thread
    let shutdown_input = Arc::clone(&shutdown);
    let input_handle = thread::spawn(move || {
        crate::mux_runtime::run_input_loop(
            gilrs,
            v_resource.device,
            config.mode,
            config.primary_id,
            config.assist_id,
            shutdown_input,
        );
    });

    // Spawn FF thread
    let shutdown_ff = Arc::clone(&shutdown);
    let ff_handle = thread::spawn(move || {
        crate::mux_runtime::run_ff_loop(&mut v_uinput, ff_targets, shutdown_ff);
    });

    Ok(MuxHandle {
        input_handle,
        ff_handle,
        shutdown,
        virtual_device_path,
    })
}
