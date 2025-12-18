use evdev::uinput::VirtualDevice;
use evdev::{EventSummary, FFStatusCode, InputEvent, UInputCode};
use log::{error, info};

use crate::PhysicalFFDev;

pub fn process_ff_event(
    event: InputEvent,
    v_dev: &mut VirtualDevice,
    phys_devs: &mut Vec<PhysicalFFDev>,
) {
    match event.destructure() {
        EventSummary::UInput(ev, UInputCode::UI_FF_UPLOAD, ..) => {
            info!("FF Upload Event: {:?}", ev);
            handle_ff_upload(ev, v_dev, phys_devs);
        }
        EventSummary::UInput(ev, UInputCode::UI_FF_ERASE, ..) => {
            info!("FF Erase Event: {:?}", ev);
            handle_ff_erase(ev, v_dev, phys_devs);
        }
        EventSummary::ForceFeedback(.., effect_id, status) => {
            info!("FF Playback Event: id={:?}, status={}", effect_id, status);
            handle_ff_playback(effect_id.0, status, phys_devs);
        }
        _ => {}
    }
}

pub fn handle_ff_upload(
    ev: evdev::UInputEvent,
    v_dev: &mut VirtualDevice,
    phys_devs: &mut Vec<PhysicalFFDev>,
) {
    let event = match v_dev.process_ff_upload(ev) {
        Ok(e) => e,
        Err(e) => {
            error!("FF Upload Process failed: {}", e);
            return;
        }
    };

    let virt_id = event.effect_id();
    let effect_data = event.effect();

    for phys_dev in phys_devs {
        match phys_dev.dev.upload_ff_effect(effect_data) {
            Ok(ff_effect) => {
                info!(
                    "Uploaded effect to physical device (virt_id: {}, phys_id: {})",
                    virt_id,
                    ff_effect.id()
                );
                phys_dev.effect_map.insert(virt_id, ff_effect);
            }
            Err(e) => error!("Failed to upload effect to physical device: {}", e),
        }
    }
}

pub fn handle_ff_erase(
    ev: evdev::UInputEvent,
    v_dev: &mut VirtualDevice,
    phys_devs: &mut Vec<PhysicalFFDev>,
) {
    match v_dev.process_ff_erase(ev) {
        Ok(ev) => {
            let virt_id = ev.effect_id() as i16;

            for phys_dev in phys_devs {
                if let Some(mut effect) = phys_dev.effect_map.remove(&virt_id)
                    && let Err(e) = effect.stop()
                {
                    error!(
                        "Failed to stop effect during erase (id: {}): {}",
                        virt_id, e
                    );
                }
            }
        }
        Err(e) => error!("FF Erase Process failed: {}", e),
    }
}

pub fn handle_ff_playback(effect_id: u16, status: i32, phys_devs: &mut Vec<PhysicalFFDev>) {
    let virt_id = effect_id as i16;
    let is_playing = status == FFStatusCode::FF_STATUS_PLAYING.0 as i32;

    for phys_dev in phys_devs {
        if let Some(effect) = phys_dev.effect_map.get_mut(&virt_id) {
            let result = if is_playing {
                effect.play(1)
            } else {
                effect.stop()
            };
            if let Err(e) = result {
                error!("FF Playback error (id: {}): {}", virt_id, e);
            }
        }
    }
}
