use crate::ff_helpers;
use crate::gilrs_helper::GamepadResource;
use crate::mux_modes;
use evdev::{Device, EventType, InputEvent};
use evdev::uinput::VirtualDevice;
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
    mode: mux_modes::ModeType,
    p_id: GamepadId,
    a_id: GamepadId,
    shutdown: Arc<AtomicBool>,
) {
    let mut mux_mode = mux_modes::create_mux_mode(mode);

    while !shutdown.load(Ordering::SeqCst) {
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
    targets: Vec<GamepadResource>,
    shutdown: Arc<AtomicBool>,
) {
    let mut phys_devs: Vec<ff_helpers::PhysicalFFDev> = targets
        .into_iter()
        .filter_map(|res| {
            if res.device.supported_ff().is_some() {
                Some(ff_helpers::PhysicalFFDev {
                    resource: res,
                    effects: HashMap::new(),
                })
            } else {
                warn!("Device {} does not support FF", res.name);
                None
            }
        })
        .collect();

    info!("FF Thread started.");

    while !shutdown.load(Ordering::SeqCst) {
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
