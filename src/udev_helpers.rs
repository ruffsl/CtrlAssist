use crate::gilrs_helper::GamepadResource;
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use udev::{Device, Enumerator};

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
    let root_syspath = physical_root.syspath();

    // 3. Find all child nodes (input/hidraw) belonging to that physical parent
    let related_nodes = find_related_devnodes(root_syspath)?;

    // 4. Restrict them
    for node in related_nodes {
        hide_and_track(&node, hidden_paths);
    }

    Ok(())
}

/// Helper: Applies permissions and updates the tracking set
fn hide_and_track(path: &Path, hidden_paths: &mut HashSet<PathBuf>) {
    // We only insert into the set if the hide operation succeeds
    if hide_device(path).is_ok() && hidden_paths.insert(path.to_path_buf()) {
        log::info!("Hidden: {}", path.display());
    } else {
        log::warn!("Failed to hide or track: {}", path.display());
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
    let mut current = start_device.clone();

    // Use an iterator to walk up the parent chain
    let ancestors = std::iter::successors(Some(start_device.clone()), |d| d.parent());

    for ancestor in ancestors {
        let subsystem = ancestor.subsystem().and_then(|s| s.to_str());

        // If we hit a physical bus, this is our root
        if matches!(subsystem, Some("usb" | "bluetooth")) {
            return ancestor;
        }

        current = ancestor;
    }

    // If we exhausted the tree without finding USB/BT, return the highest node found
    current
}

/// Finds all devnodes (input/hidraw) that are descendants of the given syspath.
fn find_related_devnodes(parent_syspath: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let mut enumerator = Enumerator::new()?;

    // We can scan for multiple subsystems
    for device in enumerator.scan_devices()? {
        let subsystem = device.subsystem().and_then(|s| s.to_str());
        // Simple filter for subsystems we care about
        if !matches!(subsystem, Some("input" | "hidraw")) {
            continue;
        }

        // Walk up from this device to see if it belongs to our parent_syspath
        let is_descendant = std::iter::successors(Some(device.clone()), |d| d.parent())
            .any(|d| d.syspath() == parent_syspath);

        if is_descendant && let Some(devnode) = device.devnode() {
            paths.push(devnode.to_path_buf());
        }
    }

    Ok(paths)
}

/// Set permissions to root-only (read/write).
fn hide_device(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

/// Restore permissions to root and input group (read/write).
pub fn restore_device(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o660))
}
