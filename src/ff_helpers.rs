use crate::gilrs_helper::GamepadResource;
use evdev::uinput::VirtualDevice;
use evdev::{EventSummary, FFStatusCode, InputEvent, UInputCode};
use log::{debug, error, warn};

pub struct PhysicalFFDev {
    pub resource: GamepadResource,
    /// Mapping: virt_id -> physical FFEffect handle
    effects: HashMap<i16, evdev::FFEffect>,
}

impl PhysicalFFDev {
    pub fn new(resource: GamepadResource) -> Self {
        Self {
            resource,
            effects: HashMap::new(),
        }
    }

    /// Upload an effect to this device and store the handle
    pub fn upload_effect(&mut self, virt_id: i16, effect_data: FFEffectData) -> std::io::Result<()> {
        let ff_effect = self.resource.device.upload_ff_effect(effect_data)?;
        self.effects.insert(virt_id, ff_effect);
        Ok(())
    }

    /// Remove an effect from this device
    pub fn erase_effect(&mut self, virt_id: i16) -> std::io::Result<()> {
        if let Some(mut effect) = self.effects.remove(&virt_id) {
            effect.stop()?;
        }
        Ok(())
    }

    /// Play or stop an effect on this device
    pub fn control_effect(&mut self, virt_id: i16, is_playing: bool) -> std::io::Result<()> {
        if let Some(effect) = self.effects.get_mut(&virt_id) {
            if is_playing {
                effect.play(1)
            } else {
                effect.stop()
            }
        } else {
            Ok(()) // Effect not on this device, that's fine
        }
    }

    /// Synchronize all effects from the manager
    pub fn sync_effects(&mut self, manager: &EffectManager) -> Vec<(i16, std::io::Error)> {
        let mut errors = Vec::new();

        // Upload missing effects
        for (virt_id, effect_data) in manager.get_effects() {
            if !self.effects.contains_key(&virt_id) {
                if let Err(e) = self.upload_effect(virt_id, effect_data) {
                    errors.push((virt_id, e));
                }
            }
        }

        // Start playing effects that should be playing
        for virt_id in manager.get_playing() {
            if let Err(e) = self.control_effect(virt_id, true) {
                errors.push((virt_id, e));
            }
        }

        errors
    }
}

// src/ff_helpers.rs - Update process_ff_event

pub fn process_ff_event(
    event: InputEvent,
    v_dev: &mut VirtualDevice,
    phys_devs: &mut Vec<PhysicalFFDev>,
) {
    // This function now only handles events we don't process in the main loop
    // Could be removed or kept for future extension
    match event.destructure() {
        EventSummary::UInput(..) | EventSummary::ForceFeedback(..) => {
            // Already handled in main loop
        }
        _ => {
            debug!("Unhandled FF event: {:?}", event);
        }
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
                phys_dev.effects.insert(virt_id, ff_effect);
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
                if let Some(mut effect) = phys_dev.effects.remove(&virt_id)
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
        let needs_recovery = if let Some(effect) = phys_dev.effects.get_mut(&virt_id) {
            playback_effect(effect, is_playing, virt_id)
                .is_err_and(|e| matches!(e.raw_os_error(), Some(libc::ENODEV)))
        } else {
            false
        };

        if needs_recovery {
            recover_physical_ff_dev(phys_dev);
            if let Some(effect) = phys_dev.effects.get_mut(&virt_id)
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
            // Re-upload all remembered effects (idiomatic borrow pattern)
            // To restore effects, we need to get the effect data from EffectManager.
            // This function should be updated to accept EffectManager as a parameter.
            // For now, this is a placeholder for the correct logic.
            // TODO: Pass EffectManager to recover_physical_ff_dev and use it here.
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

// src/ff_helpers.rs - Add this new structure

use std::collections::HashMap;
use evdev::FFEffectData;

/// Centralized manager for force feedback effects
pub struct EffectManager {
    /// Master copy of all uploaded effects: virt_id -> effect_data
    effects: HashMap<i16, FFEffectData>,
    /// Track which effects are currently playing
    playing: HashMap<i16, bool>,
}

impl EffectManager {
    pub fn new() -> Self {
        Self {
            effects: HashMap::new(),
            playing: HashMap::new(),
        }
    }

    /// Record a new effect upload
    pub fn upload(&mut self, virt_id: i16, effect_data: FFEffectData) {
        self.effects.insert(virt_id, effect_data);
        self.playing.insert(virt_id, false);
    }

    /// Remove an effect
    pub fn erase(&mut self, virt_id: i16) {
        self.effects.remove(&virt_id);
        self.playing.remove(&virt_id);
    }

    /// Mark effect as playing or stopped
    pub fn set_playing(&mut self, virt_id: i16, is_playing: bool) {
        self.playing.insert(virt_id, is_playing);
    }

    /// Get all effects that should be on a device
    pub fn get_effects(&self) -> impl Iterator<Item = (i16, FFEffectData)> + '_ {
        self.effects.iter().map(|(&id, &data)| (id, data))
    }

    /// Get all currently playing effects
    pub fn get_playing(&self) -> impl Iterator<Item = i16> + '_ {
        self.playing
            .iter()
            .filter(|&(_, &is_playing)| is_playing)
            .map(|(&id, _)| id)
    }
}
