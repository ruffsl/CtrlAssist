use crate::ff_helpers::PhysicalFFDev;
use crate::gilrs_helper::GamepadResource;
use crate::demux_modes::{self, DemuxModeType};
use evdev::{Device, EventType, InputEvent};
use gilrs::{GamepadId, Gilrs};
use log::{debug, error, info};
use parking_lot::RwLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const NEXT_EVENT_TIMEOUT: Duration = Duration::from_millis(1000);

/// Rumble target for demux
#[derive(clap::ValueEnum, Clone, Debug, Default, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum DemuxRumbleTarget {
    #[default]
    Active,
    None,
}

/// Runtime-updatable demux settings
pub struct DemuxRuntimeSettings {
    pub mode: Arc<RwLock<DemuxModeType>>,
    pub rumble: Arc<RwLock<DemuxRumbleTarget>>,
}

impl DemuxRuntimeSettings {
    pub fn new(mode: DemuxModeType, rumble: DemuxRumbleTarget) -> Self {
        Self {
            mode: Arc::new(RwLock::new(mode)),
            rumble: Arc::new(RwLock::new(rumble)),
        }
    }

    pub fn update_mode(&self, new_mode: DemuxModeType) {
        let mut mode = self.mode.write();
        *mode = new_mode;
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
    let virtual_count = v_devs.len();
    let mut demux_mode = demux_modes::create_demux_mode(runtime_settings.get_mode());
    let mut last_mode = runtime_settings.get_mode();

    while !shutdown.load(Ordering::SeqCst) {
        // Check for mode changes
        let current_mode = runtime_settings.get_mode();
        if current_mode != last_mode {
            info!(
                "Switching demux mode from {:?} to {:?}",
                last_mode, current_mode
            );
            demux_mode = demux_modes::create_demux_mode(current_mode.clone());
            last_mode = current_mode;
        }

        while let Some(event) = gilrs.next_event_blocking(Some(NEXT_EVENT_TIMEOUT)) {
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            
            if let Some(routing) = demux_mode.handle_event(&event, p_id, virtual_count, &gilrs) {
                for (virt_idx, mut out_events) in routing {
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
    shutdown: Arc<AtomicBool>,
) {
    use crate::ff_helpers::EffectManager;

    let mut effect_manager = EffectManager::new();
    let mut phys_dev = PhysicalFFDev::new(primary_resource);

    info!("FF Thread started for virtual device");

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
        let rumble_enabled = runtime_settings.get_rumble() == DemuxRumbleTarget::Active;

        for event in events {
            match event.destructure() {
                evdev::EventSummary::UInput(ev, evdev::UInputCode::UI_FF_UPLOAD, ..) => {
                    if let Ok(upload_ev) = v_uinput.process_ff_upload(ev) {
                        let virt_id = upload_ev.effect_id();
                        let effect_data = upload_ev.effect();

                        effect_manager.upload(virt_id, effect_data);

                        if rumble_enabled {
                            if let Err(e) = phys_dev.upload_effect(virt_id, effect_data) {
                                error!("Failed to upload effect {}: {}", virt_id, e);
                            }
                        }
                    }
                }

                evdev::EventSummary::UInput(ev, evdev::UInputCode::UI_FF_ERASE, ..) => {
                    if let Ok(erase_ev) = v_uinput.process_ff_erase(ev) {
                        let virt_id = erase_ev.effect_id() as i16;

                        if rumble_enabled {
                            if let Err(e) = phys_dev.erase_effect(virt_id) {
                                error!("Failed to erase effect {}: {}", virt_id, e);
                            }
                        }

                        effect_manager.erase(virt_id);
                    }
                }

                evdev::EventSummary::ForceFeedback(_, effect_id, status) => {
                    let virt_id = effect_id.0 as i16;
                    let is_playing = status == evdev::FFStatusCode::FF_STATUS_PLAYING.0 as i32;

                    effect_manager.set_playing(virt_id, is_playing);

                    if rumble_enabled {
                        if let Err(e) = phys_dev.control_effect(virt_id, is_playing) {
                            error!("Failed to control effect {}: {}", virt_id, e);
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
