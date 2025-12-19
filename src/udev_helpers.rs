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

/// Restrict access to all device nodes related to a physical gamepad.
pub fn hide_gamepad_devices(
    resource: &GamepadResource,
    hidden_paths: &mut HashSet<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    let event_path = resource.path.as_path();

    // 1. Find the specific udev device for the given path
    let device = match find_device_by_path(event_path)? {
        Some(d) => d,
        None => {
            // Fallback: just hide the event path itself if udev can't find it
            hide_and_track(event_path, hidden_paths);
            return Ok(());
        }
    };

    // 2. Find the physical parent (USB/Bluetooth root)
    let physical_root = find_physical_root(&device);

    // 3. Find all child nodes (input/hidraw) belonging to that physical parent
    let related_nodes = find_related_devnodes(&physical_root)?;

    // 4. Restrict them
    for node in related_nodes {
        hide_and_track(&node, hidden_paths);
    }

    Ok(())
}

/// Helper: Applies permissions and updates the tracking set.
fn hide_and_track(path: &Path, hidden_paths: &mut HashSet<PathBuf>) {
    // Only track if the hide operation succeeds and it wasn't already tracked
    match set_permissions(path, MODE_ROOT_ONLY) {
        Ok(_) => {
            if hidden_paths.insert(path.to_path_buf()) {
                log::info!("Hidden: {}", path.display());
            }
        }
        Err(e) => log::warn!("Failed to hide {}: {}", path.display(), e),
    }
}

/// Scans udev to find the device corresponding to a file path.
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

    // OPTIMIZATION: Instead of scanning everything and manually checking parents,
    // let libudev filter devices that are children of our physical root.
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

/// Restore permissions to root and input group (read/write).
pub fn restore_device(path: &Path) -> io::Result<()> {
    set_permissions(path, MODE_ROOT_GROUP)
}

/// Internal helper to set specific unix mode permissions.
fn set_permissions(path: &Path, mode: u32) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}
