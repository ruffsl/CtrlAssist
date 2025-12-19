use crate::gilrs_helper::GamepadResource;
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use udev::Enumerator;


/// Restrict access to all device nodes related to a physical gamepad, starting from its event path.
pub fn restrict_gamepad_devices(
    resource: &GamepadResource,
    restricted_paths: &mut HashSet<String>,
) -> Result<(), Box<dyn Error>> {
    let event_path = &resource.path;
    let mut enumerator = Enumerator::new()?;
    let mut parent_syspath = None;

    // Find the udev device that matches the event path
    enumerator.match_subsystem("input")?;
    for device in enumerator.scan_devices()? {
        if let Some(devnode) = device.devnode() {
            if devnode == event_path {
                // Found the matching udev device, walk up to find the physical parent
                if let Some(mut walker) = device.parent() {
                    loop {
                        let subsystem = walker.subsystem().and_then(|s| s.to_str());
                        if matches!(subsystem, Some("usb" | "bluetooth")) {
                            parent_syspath = Some(walker.syspath().to_path_buf());
                            break;
                        }
                        if let Some(next_parent) = walker.parent() {
                            walker = next_parent;
                        } else {
                            parent_syspath = Some(walker.syspath().to_path_buf());
                            break;
                        }
                    }
                }
                break;
            }
        }
    }

    // If no parent found, just restrict the event path itself
    if parent_syspath.is_none() {
        let path = event_path.to_string_lossy().to_string();
        if restrict_device(&path).is_ok() && restricted_paths.insert(path.clone()) {
            log::info!("Hidden: {}", path);
        }
        return Ok(());
    }
    let parent_syspath = parent_syspath.unwrap();

    // Find all child devnodes that share this parent
    let mut paths_to_restrict = HashSet::new();
    let mut enumerator = Enumerator::new()?;
    for subsystem in ["input", "hidraw"] {
        enumerator.match_subsystem(subsystem)?;
        for device in enumerator.scan_devices()? {
            let mut current = device.clone();
            while let Some(parent) = current.parent() {
                if parent.syspath() == parent_syspath {
                    if let Some(devnode) = device.devnode() {
                        paths_to_restrict.insert(devnode.to_string_lossy().to_string());
                    }
                    break;
                }
                current = parent;
            }
        }
    }

    // Restrict all found device paths
    for path in paths_to_restrict {
        if restrict_device(&path).is_ok() && restricted_paths.insert(path.clone()) {
            log::info!("Hidden: {}", path);
        }
    }
    Ok(())
}

/// Set permissions to root-only (read/write)
fn restrict_device(path: &str) -> std::io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

/// Restore permissions to root and input group (read/write)
pub fn restore_device(path: &str) -> std::io::Result<()> {
    // 0o660 is a common default (rw for owner, rw for group)
    fs::set_permissions(path, fs::Permissions::from_mode(0o660))
}
