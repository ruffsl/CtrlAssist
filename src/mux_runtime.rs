use crate::RumbleTarget;
use crate::mux_modes::ModeType;
use log::debug;
use parking_lot::RwLock;

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

use crate::ff_helpers;
use crate::gilrs_helper::GamepadResource;
use crate::mux_modes;
use evdev::uinput::VirtualDevice;
use evdev::{Device, EventType, InputEvent};
use gilrs::{GamepadId, Gilrs};
use log::{error, info, warn};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const NEXT_EVENT_TIMEOUT: Duration = Duration::from_millis(1000);

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
    use crate::ff_helpers::process_ff_event;
    use std::collections::HashMap;

    // Persistent effect memory: virt_id -> (FFEffect, FFEffectData)
    let mut effect_map: HashMap<i16, (evdev::FFEffect, evdev::FFEffectData)> = HashMap::new();
    // Track which effects are currently playing
    let mut playing_effects: HashMap<i16, bool> = HashMap::new();
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
            let mut new_phys_devs =
                build_ff_targets(&all_resources, current_rumble.clone(), p_id, a_id);
            // For each new device, re-upload all remembered effects
            for new_dev in &mut new_phys_devs {
                if !phys_devs
                    .iter()
                    .any(|d| d.resource.path == new_dev.resource.path)
                {
                    for (&virt_id, &(_, effect_data)) in &effect_map {
                        match new_dev.resource.device.upload_ff_effect(effect_data) {
                            Ok(_ff_effect) => {
                                debug!(
                                    "Re-uploaded effect to new device (virt_id: {}, phys: {})",
                                    virt_id,
                                    new_dev.resource.path.display()
                                );
                            }
                            Err(e) => {
                                error!(
                                    "Failed to re-upload effect to new device (virt_id: {}, phys: {}): {}",
                                    virt_id,
                                    new_dev.resource.path.display(),
                                    e
                                );
                            }
                        }
                    }
                }
            }
            phys_devs = new_phys_devs;
            // After rebuilding phys_devs, update their effect maps to match effect_map
            for phys_dev in &mut phys_devs {
                phys_dev.effects.clear();
                for (&virt_id, &(_, effect_data)) in &effect_map {
                    match phys_dev.resource.device.upload_ff_effect(effect_data) {
                        Ok(ff_effect) => {
                            phys_dev.effects.insert(virt_id, (ff_effect, effect_data));
                            debug!(
                                "Re-uploaded effect after rumble target switch (virt_id: {}, phys: {})",
                                virt_id,
                                phys_dev.resource.path.display()
                            );
                        }
                        Err(e) => {
                            error!(
                                "Failed to re-upload effect after rumble target switch (virt_id: {}, phys: {}): {}",
                                virt_id,
                                phys_dev.resource.path.display(),
                                e
                            );
                        }
                    }
                }
            }
            // Immediately replay any effects that were playing before the switch
            for (&virt_id, &is_playing) in &playing_effects {
                if is_playing {
                    for phys_dev in &mut phys_devs {
                        if let Some((effect, _)) = phys_dev.effects.get_mut(&virt_id) {
                            let _ = effect.play(1);
                        }
                    }
                }
            }
            last_rumble = current_rumble;
        }

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
                        // Upload to all phys_devs and store the returned FFEffect for each
                        for (i, phys_dev) in phys_devs.iter_mut().enumerate() {
                            match phys_dev.resource.device.upload_ff_effect(effect_data) {
                                Ok(ff_effect) => {
                                    phys_dev.effects.insert(virt_id, (ff_effect, effect_data));
                                    // For the first device, also upload again for effect_map
                                    if i == 0 {
                                        match phys_dev.resource.device.upload_ff_effect(effect_data) {
                                            Ok(ff_effect_map) => {
                                                effect_map.insert(virt_id, (ff_effect_map, effect_data));
                                            }
                                            Err(e) => {
                                                error!("Failed to upload effect for effect_map: {}", e);
                                            }
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to upload effect to physical device: {}", e);
                                }
                            }
                        }
                        // Mark as not playing until a playback event is received
                        playing_effects.insert(virt_id, false);
                    }
                }
                evdev::EventSummary::UInput(ev, evdev::UInputCode::UI_FF_ERASE, ..) => {
                    if let Ok(erase_ev) = v_uinput.process_ff_erase(ev) {
                        let virt_id = erase_ev.effect_id() as i16;
                        effect_map.remove(&virt_id);
                        playing_effects.remove(&virt_id);
                        for phys_dev in &mut phys_devs {
                            if let Some((mut effect, _)) = phys_dev.effects.remove(&virt_id) {
                                let _ = effect.stop();
                            }
                        }
                    }
                }
                evdev::EventSummary::ForceFeedback(_, effect_id, status) => {
                    let virt_id = effect_id.0 as i16;
                    let is_playing = status == evdev::FFStatusCode::FF_STATUS_PLAYING.0 as i32;
                    playing_effects.insert(virt_id, is_playing);
                    process_ff_event(event, v_uinput, &mut phys_devs);
                }
                _ => {
                    process_ff_event(event, v_uinput, &mut phys_devs);
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
) -> Vec<ff_helpers::PhysicalFFDev> {
    let rumble_ids = match rumble {
        RumbleTarget::Primary => vec![p_id],
        RumbleTarget::Assist => vec![a_id],
        RumbleTarget::Both => vec![p_id, a_id],
        RumbleTarget::None => vec![],
    };

    rumble_ids
        .into_iter()
        .filter_map(|id| {
            all_resources.get(&id).and_then(|res: &GamepadResource| {
                if res.device.supported_ff().is_some() {
                    // Clone the resource to create a new PhysicalFFDev
                    Some(ff_helpers::PhysicalFFDev {
                        resource: GamepadResource {
                            name: res.name.clone(),
                            path: res.path.clone(),
                            device: Device::open(&res.path).ok()?,
                        },
                        effects: HashMap::new(),
                    })
                } else {
                    warn!("Device {} does not support FF", res.name);
                    None
                }
            })
        })
        .collect()
}
