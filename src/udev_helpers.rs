use crate::gilrs_helper::GamepadResource;
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use udev::{Device, Enumerator};

const MODE_ROOT_ONLY: u32 = 0o600;
const MODE_ROOT_GROUP: u32 = 0o660;

/// A RAII guard that hides devices and automatically restores them when dropped.
pub struct ScopedDeviceHider {
    hidden_paths: HashSet<PathBuf>,
}

impl ScopedDeviceHider {
    pub fn new() -> Self {
        Self {
            hidden_paths: HashSet::new(),
        }
    }

    /// Restrict access to all device nodes related to a physical gamepad.
    pub fn hide_gamepad_devices(
        &mut self,
        resource: &GamepadResource,
    ) -> Result<(), Box<dyn Error>> {
        let event_path = resource.path.as_path();

        // 1. Find the specific udev device for the given path
        let device = match find_device_by_path(event_path)? {
            Some(d) => d,
            None => {
                // Fallback: just hide the event path itself if udev can't find it
                self.hide_and_track(event_path);
                return Ok(());
            }
        };

        // 2. Find the physical parent (USB/Bluetooth root)
        let physical_root = find_physical_root(&device);

        // 3. Find all child nodes (input/hidraw) belonging to that physical parent
        let related_nodes = find_related_devnodes(&physical_root)?;

        // 4. Restrict them
        for node in related_nodes {
            self.hide_and_track(&node);
        }

        Ok(())
    }

    /// Helper: Applies permissions and tracks the path internally.
    fn hide_and_track(&mut self, path: &Path) {
        // Skip if we are already tracking this path to avoid redundant syscalls
        if self.hidden_paths.contains(path) {
            return;
        }

        match set_permissions(path, MODE_ROOT_ONLY) {
            Ok(_) => {
                self.hidden_paths.insert(path.to_path_buf());
                log::info!("Hidden: {}", path.display());
            }
            Err(e) => log::warn!("Failed to hide {}: {}", path.display(), e),
        }
    }
}

// Ensure devices are restored when the struct goes out of scope (e.g. app exit/panic).
impl Drop for ScopedDeviceHider {
    fn drop(&mut self) {
        for path in &self.hidden_paths {
            if let Err(e) = set_permissions(path, MODE_ROOT_GROUP) {
                log::error!("Failed to restore {}: {}", path.display(), e);
            } else {
                log::info!("Restored: {}", path.display());
            }
        }
    }
}

// --- Helper Functions (Stateless) ---

fn find_device_by_path(target_path: &Path) -> io::Result<Option<Device>> {
    let mut enumerator = Enumerator::new()?;
    enumerator.match_subsystem("input")?;

    for device in enumerator.scan_devices()? {
        if let Some(devnode) = device.devnode()
            && devnode == target_path
        {
            return Ok(Some(device));
        }
    }
    Ok(None)
}

/// Walks up the device tree to find the physical root (USB or Bluetooth),
/// or returns the top-most parent if neither is found.
fn find_physical_root(start_device: &Device) -> Device {
    let mut last_device = start_device.clone();

    // Walk up the ancestry chain
    let ancestors = std::iter::successors(Some(start_device.clone()), |d| d.parent());

    for ancestor in ancestors {
        if let Some(subsystem) = ancestor.subsystem().and_then(|s| s.to_str())
            && matches!(subsystem, "usb" | "bluetooth")
        {
            return ancestor;
        }
        last_device = ancestor;
    }

    // If we exhausted the tree without finding USB/BT, return the highest node found
    last_device
}

/// Finds all devnodes (input/hidraw) that are descendants of the given parent device.
fn find_related_devnodes(parent_device: &Device) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut enumerator = Enumerator::new()?;

    // Let udev handle the parent matching
    enumerator.match_parent(parent_device)?;

    for device in enumerator.scan_devices()? {
        let subsystem = device.subsystem().and_then(|s| s.to_str());

        // Filter for subsystems we care about
        if matches!(subsystem, Some("input" | "hidraw"))
            && let Some(devnode) = device.devnode()
        {
            paths.push(devnode.to_path_buf());
        }
    }
    Ok(paths)
}

fn set_permissions(path: &Path, mode: u32) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}
