use crate::gilrs_helper::GamepadResource;
use evdev::uinput::VirtualDevice;
use evdev::{EventSummary, FFStatusCode, InputEvent, UInputCode};
use log::{debug, error, warn};
use std::collections::HashMap;

pub(crate) struct PhysicalFFDev {
    pub(crate) resource: GamepadResource,
    pub(crate) effect_map: HashMap<i16, evdev::FFEffect>,
    pub(crate) effect_data_map: HashMap<i16, evdev::FFEffectData>,
}

pub fn process_ff_event(
    event: InputEvent,
    v_dev: &mut VirtualDevice,
    phys_devs: &mut Vec<PhysicalFFDev>,
) {
    match event.destructure() {
        EventSummary::UInput(ev, UInputCode::UI_FF_UPLOAD, ..) => {
            debug!("FF Upload Event: {:?}", ev);
            handle_ff_upload(ev, v_dev, phys_devs);
        }
        EventSummary::UInput(ev, UInputCode::UI_FF_ERASE, ..) => {
            debug!("FF Erase Event: {:?}", ev);
            handle_ff_erase(ev, v_dev, phys_devs);
        }
        EventSummary::ForceFeedback(.., effect_id, status) => {
            debug!("FF Playback Event: id={:?}, status={}", effect_id, status);
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
        match phys_dev.resource.device.upload_ff_effect(effect_data) {
            Ok(ff_effect) => {
                debug!(
                    "Uploaded effect to physical device (virt_id: {}, phys_id: {})",
                    virt_id,
                    ff_effect.id()
                );
                phys_dev.effect_map.insert(virt_id, ff_effect);
                phys_dev.effect_data_map.insert(virt_id, effect_data);
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
        let needs_recovery = if let Some(effect) = phys_dev.effect_map.get_mut(&virt_id) {
            playback_effect(effect, is_playing, virt_id)
                .is_err_and(|e| matches!(e.raw_os_error(), Some(libc::ENODEV)))
        } else {
            false
        };

        if needs_recovery {
            recover_physical_ff_dev(phys_dev);
            if let Some(effect) = phys_dev.effect_map.get_mut(&virt_id)
                && let Err(e) = playback_effect(effect, is_playing, virt_id)
            {
                error!(
                    "FF Playback retry after recovery failed (id: {}): {}",
                    virt_id, e
                );
            }
        }
    }
}

fn playback_effect(
    effect: &mut evdev::FFEffect,
    is_playing: bool,
    virt_id: i16,
) -> std::io::Result<()> {
    let result = if is_playing {
        effect.play(1)
    } else {
        effect.stop()
    };
    if let Err(ref e) = result {
        error!("FF Playback error (id: {}): {}", virt_id, e);
    }
    result
}

/// Attempt to recover a disconnected physical FF device by reopening it using its path.
fn recover_physical_ff_dev(phys_dev: &mut PhysicalFFDev) {
    let path = &phys_dev.resource.path;
    match evdev::Device::open(path) {
        Ok(new_dev) => {
            phys_dev.resource.device = new_dev;
            warn!("FF device reopened after disconnect: {}", path.display());
            // Re-upload all remembered effects
            for (&virt_id, effect_data) in &phys_dev.effect_data_map {
                match phys_dev.resource.device.upload_ff_effect(*effect_data) {
                    Ok(ff_effect) => {
                        phys_dev.effect_map.insert(virt_id, ff_effect);
                        debug!("Re-uploaded effect after recovery (virt_id: {})", virt_id);
                    }
                    Err(e) => {
                        error!(
                            "Failed to re-upload effect after recovery (virt_id: {}): {}",
                            virt_id, e
                        );
                    }
                }
            }
        }
        Err(open_err) => {
            error!(
                "Failed to reopen FF device: {}: {}",
                path.display(),
                open_err
            );
        }
    }
}
