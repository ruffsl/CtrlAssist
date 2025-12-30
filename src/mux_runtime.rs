use crate::RumbleTarget;
use crate::ff_helpers::PhysicalFFDev;
use crate::gilrs_helper::GamepadResource;
use crate::mux_modes;
use crate::mux_modes::ModeType;
use evdev::uinput::VirtualDevice;
use evdev::{Device, EventType, InputEvent};
use gilrs::{GamepadId, Gilrs};
use log::{debug, error, info, warn};
use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const NEXT_EVENT_TIMEOUT: Duration = Duration::from_millis(1000);

/// Runtime-updatable mux settings
pub struct RuntimeSettings {
    pub mode: Arc<RwLock<ModeType>>,
    pub rumble: Arc<RwLock<RumbleTarget>>,
}

impl RuntimeSettings {
    pub fn new(mode: ModeType, rumble: RumbleTarget) -> Self {
        Self {
            mode: Arc::new(RwLock::new(mode)),
            rumble: Arc::new(RwLock::new(rumble)),
        }
    }

    pub fn update_mode(&self, new_mode: ModeType) -> ModeType {
        let mut mode = self.mode.write();
        let old_mode = mode.clone();
        *mode = new_mode;
        old_mode
    }

    pub fn update_rumble(&self, new_rumble: RumbleTarget) -> RumbleTarget {
        let mut rumble = self.rumble.write();
        let old_rumble = rumble.clone();
        *rumble = new_rumble;
        old_rumble
    }

    pub fn get_mode(&self) -> ModeType {
        self.mode.read().clone()
    }

    pub fn get_rumble(&self) -> RumbleTarget {
        self.rumble.read().clone()
    }
}

pub fn run_input_loop(
    mut gilrs: Gilrs,
    mut v_dev: Device,
    runtime_settings: Arc<RuntimeSettings>,
    p_id: GamepadId,
    a_id: GamepadId,
    shutdown: Arc<AtomicBool>,
) {
    let mut mux_mode = mux_modes::create_mux_mode(runtime_settings.get_mode());
    let mut last_mode = runtime_settings.get_mode();

    while !shutdown.load(Ordering::SeqCst) {
        // Check for mode changes
        let current_mode = runtime_settings.get_mode();
        if current_mode != last_mode {
            info!(
                "Switching mux mode from {:?} to {:?}",
                last_mode, current_mode
            );
            mux_mode = mux_modes::create_mux_mode(current_mode.clone());
            last_mode = current_mode;
        }

        while let Some(event) = gilrs.next_event_blocking(Some(NEXT_EVENT_TIMEOUT)) {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            if event.id != p_id && event.id != a_id {
                continue;
            }
            if let Some(mut out_events) = mux_mode.handle_event(&event, p_id, a_id, &gilrs)
                && !out_events.is_empty()
            {
                out_events.push(InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0));
                if let Err(e) = v_dev.send_events(&out_events) {
                    error!("Failed to write input events: {}", e);
                }
            }
        }
    }
}

pub fn run_ff_loop(
    v_uinput: &mut VirtualDevice,
    all_resources: HashMap<GamepadId, GamepadResource>,
    runtime_settings: Arc<RuntimeSettings>,
    p_id: GamepadId,
    a_id: GamepadId,
    shutdown: Arc<AtomicBool>,
) {
    use crate::ff_helpers::EffectManager;

    // Centralized effect state
    let mut effect_manager = EffectManager::new();

    // Current physical devices
    let mut phys_devs = build_ff_targets(&all_resources, runtime_settings.get_rumble(), p_id, a_id);
    let mut last_rumble = runtime_settings.get_rumble();

    info!("FF Thread started.");

    while !shutdown.load(Ordering::SeqCst) {
        // Check for rumble target changes
        let current_rumble = runtime_settings.get_rumble();
        if current_rumble != last_rumble {
            info!(
                "Switching rumble target from {:?} to {:?}",
                last_rumble, current_rumble
            );

            // Build new device set
            let mut new_phys_devs =
                build_ff_targets(&all_resources, current_rumble.clone(), p_id, a_id);

            // Synchronize all effects to new devices
            for dev in &mut new_phys_devs {
                let errors = dev.sync_effects(&effect_manager);
                for (virt_id, error) in errors {
                    error!(
                        "Failed to sync effect {} to {}: {}",
                        virt_id,
                        dev.resource.path.display(),
                        error
                    );
                }
            }

            // Stop all effects on old devices (cleanup)
            for dev in &mut phys_devs {
                for virt_id in effect_manager.get_playing() {
                    let _ = dev.control_effect(virt_id, false);
                }
            }

            phys_devs = new_phys_devs;
            last_rumble = current_rumble;
        }

        // Process events
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
                        match dev.control_effect(virt_id, is_playing) {
                            Ok(()) => {
                                // Success
                            }
                            Err(e) if e.raw_os_error() == Some(libc::ENODEV) => {
                                // Device disconnected, attempt recovery
                                warn!(
                                    "Device {} disconnected, attempting recovery",
                                    dev.resource.path.display()
                                );

                                match dev.recover(&effect_manager) {
                                    Ok(()) => {
                                        info!(
                                            "Successfully recovered device {}",
                                            dev.resource.path.display()
                                        );
                                        // Retry the control operation after recovery
                                        if let Err(retry_err) =
                                            dev.control_effect(virt_id, is_playing)
                                        {
                                            error!(
                                                "Failed to control effect {} after recovery on {}: {}",
                                                virt_id,
                                                dev.resource.path.display(),
                                                retry_err
                                            );
                                        }
                                    }
                                    Err(recover_err) => {
                                        error!(
                                            "Failed to recover device {}: {}",
                                            dev.resource.path.display(),
                                            recover_err
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                // Other error
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
                    debug!("Unhandled FF event: {:?}", event);
                }
            }
        }
    }
}

// Helper function to build FF targets based on rumble setting
fn build_ff_targets(
    all_resources: &HashMap<GamepadId, GamepadResource>,
    rumble: RumbleTarget,
    p_id: GamepadId,
    a_id: GamepadId,
) -> Vec<PhysicalFFDev> {
    let rumble_ids = match rumble {
        RumbleTarget::Primary => vec![p_id],
        RumbleTarget::Assist => vec![a_id],
        RumbleTarget::Both => vec![p_id, a_id],
        RumbleTarget::None => vec![],
    };

    rumble_ids
        .into_iter()
        .filter_map(|id| {
            all_resources.get(&id).and_then(|res| {
                if res.device.supported_ff().is_some() {
                    Some(PhysicalFFDev::new(res.clone()))
                } else {
                    warn!(
                        "Device {} ({}) does not support force feedback",
                        res.name,
                        res.path.display()
                    );
                    None
                }
            })
        })
        .collect()
}
