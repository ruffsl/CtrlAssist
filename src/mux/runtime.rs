use crate::mux::MuxRumbleTarget;
use crate::mux::modes::MuxModeType;
use crate::utils::ff::{EffectManager, PhysicalFFDev};
use crate::utils::gilrs::GamepadResource;
use evdev::uinput::VirtualDevice;
use evdev::{Device, EventType, InputEvent};
use gilrs::{GamepadId, Gilrs};
use log::{debug, error, info, warn};
use parking_lot::RwLock;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

const NEXT_EVENT_TIMEOUT: Duration = Duration::from_millis(1000);

/// Runtime-updatable mux settings
pub struct RuntimeSettings {
    pub mode: Arc<RwLock<MuxModeType>>,
    pub rumble: Arc<RwLock<MuxRumbleTarget>>,
    /// Track currently active controllers for force feedback
    active_controllers: Arc<RwLock<HashSet<GamepadId>>>,
    /// Generation counter incremented when FF targets need rebuilding
    /// This covers both rumble target changes AND active controller changes
    ff_generation: Arc<AtomicU64>,
}

impl RuntimeSettings {
    pub fn new(
        mode: MuxModeType,
        rumble: MuxRumbleTarget,
        primary_id: GamepadId,
        assist_id: GamepadId,
    ) -> Self {
        // Get initial active controllers from the mode
        let mode_impl = crate::mux::modes::create_mux_mode(mode.clone());
        let initial_active = mode_impl.initial_active_controllers(primary_id, assist_id);

        Self {
            mode: Arc::new(RwLock::new(mode)),
            rumble: Arc::new(RwLock::new(rumble)),
            active_controllers: Arc::new(RwLock::new(initial_active.into_iter().collect())),
            ff_generation: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn update_mode(&self, new_mode: MuxModeType, initial_active: Vec<GamepadId>) {
        let mut mode = self.mode.write();
        *mode = new_mode;

        // Update active controllers directly with the provided list
        let mut active = self.active_controllers.write();
        active.clear();
        active.extend(initial_active);

        // Increment generation to trigger FF rebuild
        self.ff_generation.fetch_add(1, Ordering::Release);
    }

    pub fn set_active_controllers(&self, controllers: Vec<GamepadId>) {
        let mut active = self.active_controllers.write();
        active.clear();
        active.extend(controllers);

        // Increment generation to trigger FF rebuild
        self.ff_generation.fetch_add(1, Ordering::Release);
    }

    pub fn update_rumble(&self, new_rumble: MuxRumbleTarget) {
        let mut rumble = self.rumble.write();
        *rumble = new_rumble;

        // Increment generation to trigger FF rebuild
        self.ff_generation.fetch_add(1, Ordering::Release);
    }

    pub fn get_mode(&self) -> MuxModeType {
        self.mode.read().clone()
    }

    pub fn get_rumble(&self) -> MuxRumbleTarget {
        self.rumble.read().clone()
    }

    /// Get current FF generation for change detection
    pub fn ff_generation(&self) -> u64 {
        self.ff_generation.load(Ordering::Acquire)
    }

    /// Get the current set of active controllers for FF routing
    pub fn get_active_controllers(&self) -> Vec<GamepadId> {
        self.active_controllers.read().iter().copied().collect()
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
    let mut mux_mode = crate::mux::modes::create_mux_mode(runtime_settings.get_mode());
    let mut last_mode = runtime_settings.get_mode();

    while !shutdown.load(Ordering::SeqCst) {
        // Check for mode changes
        let current_mode = runtime_settings.get_mode();

        if current_mode != last_mode {
            info!(
                "Switching mux mode from {:?} to {:?}",
                last_mode, current_mode
            );

            mux_mode = crate::mux::modes::create_mux_mode(current_mode.clone());
            let defaults = mux_mode.initial_active_controllers(p_id, a_id);
            runtime_settings.update_mode(current_mode.clone(), defaults);

            last_mode = current_mode;
        }

        while let Some(event) = gilrs.next_event_blocking(Some(NEXT_EVENT_TIMEOUT)) {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            if event.id != p_id && event.id != a_id {
                continue;
            }

            if let Some(output) = mux_mode.handle_event(&event, p_id, a_id, &gilrs) {
                // NEW: Update active controllers if requested by the mode
                if let Some(new_active) = output.set_active_controllers {
                    runtime_settings.set_active_controllers(new_active);
                }

                // Send events
                if !output.events.is_empty() {
                    let mut out_events = output.events;
                    out_events.push(InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0));
                    if let Err(e) = v_dev.send_events(&out_events) {
                        error!("Failed to write input events: {}", e);
                    }
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
    // Centralized effect state
    let mut effect_manager = EffectManager::new();

    // Current physical devices
    let mut phys_devs = build_ff_targets(&all_resources, &runtime_settings, p_id, a_id);
    let mut last_ff_generation = runtime_settings.ff_generation();

    info!("FF Thread started.");

    while !shutdown.load(Ordering::SeqCst) {
        // Check if FF targets need rebuilding (catches rumble changes AND active controller changes)
        let current_generation = runtime_settings.ff_generation();
        if current_generation != last_ff_generation {
            info!(
                "FF targets changed, rebuilding (generation {} -> {})",
                last_ff_generation, current_generation
            );

            // Build new device set
            let mut new_phys_devs = build_ff_targets(&all_resources, &runtime_settings, p_id, a_id);

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
            last_ff_generation = current_generation;
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
    runtime_settings: &Arc<RuntimeSettings>,
    p_id: GamepadId,
    a_id: GamepadId,
) -> Vec<PhysicalFFDev> {
    let rumble = runtime_settings.get_rumble();

    let rumble_ids = match rumble {
        MuxRumbleTarget::Active => runtime_settings.get_active_controllers(),
        MuxRumbleTarget::Assist => vec![a_id],
        MuxRumbleTarget::Both => vec![p_id, a_id],
        MuxRumbleTarget::None => vec![],
        MuxRumbleTarget::Primary => vec![p_id],
    };

    rumble_ids
        .into_iter()
        .filter_map(|id| {
            all_resources.get(&id).and_then(|res| {
                if res.device.supported_ff().is_some() {
                    Some(PhysicalFFDev::new(res.clone()))
                } else {
                    warn!(
                        "Device {} ({}) does not support force feedback (rumble setting: {:?})",
                        res.name,
                        res.path.display(),
                        rumble
                    );
                    None
                }
            })
        })
        .collect()
}
