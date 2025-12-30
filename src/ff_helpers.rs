use crate::gilrs_helper::GamepadResource;
use evdev::{Device, FFEffectData};
use log::{error, warn};
use std::collections::HashMap;

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
    pub fn upload_effect(
        &mut self,
        virt_id: i16,
        effect_data: FFEffectData,
    ) -> std::io::Result<()> {
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
            if !self.effects.contains_key(&virt_id)
                && let Err(e) = self.upload_effect(virt_id, effect_data)
            {
                errors.push((virt_id, e));
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

    /// Attempt to recover a disconnected device
    pub fn recover(&mut self, manager: &EffectManager) -> std::io::Result<()> {
        let path = self.resource.path.clone();

        // Try to reopen the device
        let new_device = Device::open(&path)?;
        self.resource.device = new_device;

        warn!("FF device reopened after disconnect: {}", path.display());

        // Clear old effect handles (they're invalid now)
        self.effects.clear();

        // Re-sync all effects from the manager
        let errors = self.sync_effects(manager);
        if !errors.is_empty() {
            error!(
                "Encountered {} errors while re-syncing effects after recovery for {}",
                errors.len(),
                path.display()
            );
            for (virt_id, err) in errors {
                error!("  - Effect {}: {}", virt_id, err);
            }
        }

        Ok(())
    }
}

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
