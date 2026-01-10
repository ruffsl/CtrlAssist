use crate::demux::modes::DemuxModeType;
use crate::utils::ff::{EffectManager, PhysicalFFDev};
use crate::utils::gilrs::GamepadResource;
use evdev::{Device, EventType, InputEvent};
use gilrs::{GamepadId, Gilrs};
use log::{debug, error, info};
use parking_lot::RwLock;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const NEXT_EVENT_TIMEOUT: Duration = Duration::from_millis(1000);

/// Rumble target for demux
#[derive(
    clap::ValueEnum, Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq,
)]
pub enum DemuxRumbleTarget {
    #[default]
    Active,
    None,
}

/// Runtime-updatable demux settings
pub struct DemuxRuntimeSettings {
    pub mode: Arc<RwLock<DemuxModeType>>,
    pub rumble: Arc<RwLock<DemuxRumbleTarget>>,
    pub active_virtuals: Arc<RwLock<HashSet<usize>>>,
}

impl DemuxRuntimeSettings {
    pub fn new(mode: DemuxModeType, rumble: DemuxRumbleTarget, virtuals: usize) -> Self {
        // Delegate initial active virtuals calculation to the mode implementation
        let mode_impl = crate::demux::modes::create_demux_mode(mode.clone());
        let initial_active = mode_impl
            .initial_active_virtuals(virtuals)
            .into_iter()
            .collect();

        Self {
            mode: Arc::new(RwLock::new(mode)),
            rumble: Arc::new(RwLock::new(rumble)),
            active_virtuals: Arc::new(RwLock::new(initial_active)),
        }
    }

    pub fn update_mode(&self, new_mode: DemuxModeType, virtuals: usize) {
        let mut mode_lock = self.mode.write();
        *mode_lock = new_mode.clone();

        // Reset active virtuals by asking the new mode for its defaults
        let mode_impl = crate::demux::modes::create_demux_mode(new_mode);
        let defaults = mode_impl.initial_active_virtuals(virtuals);

        let mut active_lock = self.active_virtuals.write();
        active_lock.clear();
        active_lock.extend(defaults);
    }

    pub fn set_active_virtuals(&self, indices: Vec<usize>) {
        let mut active = self.active_virtuals.write();
        active.clear();
        for idx in indices {
            active.insert(idx);
        }
    }

    pub fn is_virtual_active(&self, virtual_index: usize) -> bool {
        self.active_virtuals.read().contains(&virtual_index)
    }

    pub fn update_rumble(&self, new_rumble: DemuxRumbleTarget) {
        let mut rumble = self.rumble.write();
        *rumble = new_rumble;
    }

    pub fn get_mode(&self) -> DemuxModeType {
        self.mode.read().clone()
    }

    pub fn get_rumble(&self) -> DemuxRumbleTarget {
        self.rumble.read().clone()
    }
}

pub fn run_input_loop(
    mut gilrs: Gilrs,
    mut v_devs: Vec<Device>,
    runtime_settings: Arc<DemuxRuntimeSettings>,
    p_id: GamepadId,
    shutdown: Arc<AtomicBool>,
) {
    let virtuals = v_devs.len();
    let mut demux_mode = crate::demux::modes::create_demux_mode(runtime_settings.get_mode());
    let mut last_mode = runtime_settings.get_mode();

    while !shutdown.load(Ordering::SeqCst) {
        // Check for mode changes
        let current_mode = runtime_settings.get_mode();
        if current_mode != last_mode {
            info!(
                "Switching demux mode from {:?} to {:?}",
                last_mode, current_mode
            );
            // Ensure settings are synced with the new mode
            runtime_settings.update_mode(current_mode.clone(), virtuals);
            demux_mode = crate::demux::modes::create_demux_mode(current_mode.clone());
            last_mode = current_mode;
        }

        while let Some(event) = gilrs.next_event_blocking(Some(NEXT_EVENT_TIMEOUT)) {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }

            if let Some(output) = demux_mode.handle_event(&event, p_id, virtuals, &gilrs) {
                // Update active virtuals if requested by the mode
                if let Some(new_active) = output.set_active_virtuals {
                    runtime_settings.set_active_virtuals(new_active);
                }

                // Process output events
                for (virt_idx, mut out_events) in output.events {
                    if virt_idx >= v_devs.len() {
                        error!("Invalid virtual device index: {}", virt_idx);
                        continue;
                    }

                    if !out_events.is_empty() {
                        out_events.push(InputEvent::new(EventType::SYNCHRONIZATION.0, 0, 0));
                        if let Err(e) = v_devs[virt_idx].send_events(&out_events) {
                            error!("Failed to write input events to device {}: {}", virt_idx, e);
                        }
                    }
                }
            }
        }
    }
}

pub fn run_ff_loop(
    v_uinput: &mut evdev::uinput::VirtualDevice,
    primary_resource: GamepadResource,
    runtime_settings: Arc<DemuxRuntimeSettings>,
    virtual_index: usize,
    shutdown: Arc<AtomicBool>,
) {
    let mut effect_manager = EffectManager::new();
    let mut phys_dev = PhysicalFFDev::new(primary_resource);

    info!("FF Thread started for virtual device {}", virtual_index);

    while !shutdown.load(Ordering::SeqCst) {
        let events: Vec<_> = match v_uinput.fetch_events() {
            Ok(iter) => iter.collect(),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => vec![],
            Err(e) => {
                error!("Error fetching FF events: {}", e);
                vec![]
            }
        };

        // Check if rumble is enabled
        let is_rumble_active = runtime_settings.get_rumble() == DemuxRumbleTarget::Active;
        // Check if this specific virtual is currently active (allowed to rumble)
        let is_virtual_active = runtime_settings.is_virtual_active(virtual_index);

        for event in events {
            match event.destructure() {
                evdev::EventSummary::UInput(ev, evdev::UInputCode::UI_FF_UPLOAD, ..) => {
                    if let Ok(upload_ev) = v_uinput.process_ff_upload(ev) {
                        let virt_id = upload_ev.effect_id();
                        let effect_data = upload_ev.effect();

                        effect_manager.upload(virt_id, effect_data);

                        if let Err(e) = phys_dev.upload_effect(virt_id, effect_data) {
                            error!(
                                "Failed to upload effect {} (virtual {}): {}",
                                virt_id, virtual_index, e
                            );
                        }
                    }
                }

                evdev::EventSummary::UInput(ev, evdev::UInputCode::UI_FF_ERASE, ..) => {
                    if let Ok(erase_ev) = v_uinput.process_ff_erase(ev) {
                        let virt_id = erase_ev.effect_id() as i16;

                        if let Err(e) = phys_dev.erase_effect(virt_id) {
                            error!(
                                "Failed to erase effect {} (virtual {}): {}",
                                virt_id, virtual_index, e
                            );
                        }

                        effect_manager.erase(virt_id);
                    }
                }

                evdev::EventSummary::ForceFeedback(_, effect_id, status) => {
                    let virt_id = effect_id.0 as i16;
                    let is_playing = status == evdev::FFStatusCode::FF_STATUS_PLAYING.0 as i32;

                    effect_manager.set_playing(virt_id, is_playing);

                    // Only forward playback commands if rumble is enabled AND this virtual is active
                    if is_rumble_active
                        && is_virtual_active
                        && let Err(e) = phys_dev.control_effect(virt_id, is_playing)
                    {
                        error!(
                            "Failed to control effect {} (virtual {}): {}",
                            virt_id, virtual_index, e
                        );
                    }
                }

                _ => {
                    debug!("Unhandled FF event: {:?}", event);
                }
            }
        }
    }
}
