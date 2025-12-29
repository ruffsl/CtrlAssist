// src/mux_runtime.rs - Add at the top

use parking_lot::RwLock;
use crate::mux_modes::ModeType;
use crate::RumbleTarget;

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
        let old_mode = *mode;
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
        *self.mode.read()
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

// src/mux_runtime.rs - Replace run_input_loop

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
            info!("Switching mux mode from {:?} to {:?}", last_mode, current_mode);
            mux_mode = mux_modes::create_mux_mode(current_mode);
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

// src/mux_runtime.rs - Replace run_ff_loop signature and add rumble management

pub fn run_ff_loop(
    v_uinput: &mut VirtualDevice,
    all_resources: HashMap<GamepadId, GamepadResource>,
    runtime_settings: Arc<RuntimeSettings>,
    p_id: GamepadId,
    a_id: GamepadId,
    shutdown: Arc<AtomicBool>,
) {
    let mut phys_devs = build_ff_targets(
        &all_resources,
        runtime_settings.get_rumble(),
        p_id,
        a_id,
    );
    let mut last_rumble = runtime_settings.get_rumble();

    info!("FF Thread started.");

    while !shutdown.load(Ordering::SeqCst) {
        // Check for rumble target changes
        let current_rumble = runtime_settings.get_rumble();
        if current_rumble != last_rumble {
            info!("Switching rumble target from {:?} to {:?}", last_rumble, current_rumble);
            phys_devs = build_ff_targets(&all_resources, current_rumble, p_id, a_id);
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
            ff_helpers::process_ff_event(event, v_uinput, &mut phys_devs);
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
            all_resources.get(&id).and_then(|res| {
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
