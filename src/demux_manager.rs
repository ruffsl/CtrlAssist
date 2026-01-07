use crate::demux_modes::DemuxModeType;
use crate::demux_runtime::DemuxRuntimeSettings;
use crate::evdev_helpers::{self, VirtualGamepadInfo};
use crate::gilrs_helper;
use crate::udev_helpers::ScopedDeviceHider;
use crate::{DemuxRumbleTarget, HideType, SpoofTarget};
use evdev::Device;
use gilrs::{GamepadId, Gilrs};
use log::info;
use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::thread;

/// Configuration for starting a demux session
pub struct DemuxConfig {
    pub primary_id: GamepadId,
    pub sinks: usize,
    pub mode: DemuxModeType,
    pub hide: HideType,
    pub spoof: SpoofTarget,
    pub rumble: DemuxRumbleTarget,
}

/// Virtual device info with its path
pub struct VirtualDeviceInfo {
    pub path: PathBuf,
}

/// Handle to a running demux session
pub struct DemuxHandle {
    pub input_handle: thread::JoinHandle<()>,
    pub ff_handles: Vec<thread::JoinHandle<()>>,
    pub shutdown: Arc<AtomicBool>,
    pub virtual_device_paths: Vec<PathBuf>,
}

impl DemuxHandle {
    /// Request shutdown and wait for threads to complete
    pub fn shutdown(self) {
        use std::sync::atomic::Ordering;

        self.shutdown.store(true, Ordering::SeqCst);

        // Unblock FF threads by sending no-op events to each virtual device
        for path in &self.virtual_device_paths {
            if let Ok(mut vdev) = Device::open(path) {
                use evdev::{EventType, InputEvent};
                let _ = vdev.send_events(&[
                    InputEvent::new(EventType::FORCEFEEDBACK.0, 0, 0),
                    InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0),
                ]);
            }
        }

        let _ = self.input_handle.join();
        for handle in self.ff_handles {
            let _ = handle.join();
        }
    }
}

/// Start a demux session with the given configuration
pub fn start_demux(
    gilrs: Gilrs,
    config: DemuxConfig,
) -> Result<(DemuxHandle, Arc<DemuxRuntimeSettings>), Box<dyn Error>> {
    let resources = gilrs_helper::discover_gamepad_resources(&gilrs);

    // Validate input: sinks must be greater than 0
    if config.sinks == 0 {
        return Err("DemuxConfig.sinks must be greater than 0".into());
    }

    // Setup hiding
    let mut _hider = ScopedDeviceHider::new(config.hide.clone());
    if let Some(primary_res) = resources.get(&config.primary_id) {
        _hider.hide_gamepad_devices(primary_res)?;
    }

    // Determine virtual device info
    let virtual_info = match config.spoof {
        SpoofTarget::Primary => VirtualGamepadInfo::from(&gilrs.gamepad(config.primary_id)),
        SpoofTarget::None | SpoofTarget::Assist => VirtualGamepadInfo {
            name: "CtrlAssist Virtual Gamepad".into(),
            vendor_id: None,
            product_id: None,
        },
    };

    // Create virtual devices
    let mut v_uinputs = Vec::new();
    let mut virtual_devices = Vec::new();

    for i in 0..config.sinks {
        let tag_string = format!("[{}]", i);
        let tag_str = Some(tag_string.as_str());

        let mut v_uinput = evdev_helpers::create_virtual_gamepad(&virtual_info, tag_str)?;
        let v_resource = gilrs_helper::wait_for_virtual_device(&mut v_uinput)?;

        info!(
            "Virtual: {} @ {}",
            v_resource.name,
            v_resource.path.display()
        );

        virtual_devices.push(VirtualDeviceInfo {
            path: v_resource.path.clone(),
        });

        v_uinputs.push((v_uinput, v_resource));
    }

    // Create runtime settings
    let runtime_settings = Arc::new(DemuxRuntimeSettings::new(
        config.mode,
        config.rumble,
        config.sinks,
    ));

    // Setup shutdown signal
    let shutdown = Arc::new(AtomicBool::new(false));

    // Collect device paths for unblocking
    let virtual_device_paths: Vec<PathBuf> =
        virtual_devices.iter().map(|v| v.path.clone()).collect();

    // Clone for FF threads
    let primary_resource = resources
        .get(&config.primary_id)
        .ok_or("Primary controller not found")?
        .clone();

    // Open new Device instances from paths for input thread
    let v_devices: Vec<_> = virtual_devices
        .iter()
        .map(|v| Device::open(&v.path))
        .collect::<Result<Vec<_>, _>>()?;

    // Spawn input thread
    let shutdown_input = Arc::clone(&shutdown);
    let runtime_settings_input = Arc::clone(&runtime_settings);

    let input_handle = thread::spawn(move || {
        crate::demux_runtime::run_input_loop(
            gilrs,
            v_devices,
            runtime_settings_input,
            config.primary_id,
            shutdown_input,
        );
    });

    // Spawn FF threads (one per virtual device)
    let mut ff_handles = Vec::new();
    for (sink_idx, (mut v_uinput, _)) in v_uinputs.into_iter().enumerate() {
        let shutdown_ff = Arc::clone(&shutdown);
        let runtime_settings_ff = Arc::clone(&runtime_settings);
        let primary_ff = primary_resource.clone();

        let handle = thread::spawn(move || {
            crate::demux_runtime::run_ff_loop(
                &mut v_uinput,
                primary_ff,
                runtime_settings_ff,
                sink_idx,
                shutdown_ff,
            );
        });

        ff_handles.push(handle);
    }

    Ok((
        DemuxHandle {
            input_handle,
            ff_handles,
            shutdown,
            virtual_device_paths,
        },
        runtime_settings,
    ))
}
