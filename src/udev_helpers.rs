use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use udev::Enumerator;
use gilrs::Gamepad;

/// Restrict access to gamepad devices matching vendor and product IDs
pub fn restrict_gamepad_devices(
    gamepad: &Gamepad,
    restricted_paths: &mut HashSet<String>,
) -> Result<(), Box<dyn Error>> {
    let (Some(vendor), Some(product)) = (gamepad.vendor_id(), gamepad.product_id()) else {
        // Only proceed if we have both IDs
        return Ok(());
    };

    let vendor_str = format!("{:04x}", vendor);
    let product_str = format!("{:04x}", product);

    for subsystem in ["input", "hidraw"] {
        let mut enumerator = Enumerator::new()?;
        enumerator.match_subsystem(subsystem)?;

        for device in enumerator.scan_devices()? {
            let sys_vendor = device
                .property_value("ID_VENDOR_ID")
                .and_then(|s| s.to_str());
            let sys_product = device
                .property_value("ID_MODEL_ID")
                .and_then(|s| s.to_str());

            let matches = sys_vendor == Some(&vendor_str) && sys_product == Some(&product_str);

            if matches {
                if let Some(devnode) = device.devnode() {
                    let path_str = devnode.to_string_lossy();
                    let related_paths = find_related_input_paths(&path_str)?;

                    for path in related_paths {
                        if restrict_device(&path).is_ok() && !restricted_paths.contains(&path) {
                            println!("    Restricted: {}", path);
                            restricted_paths.insert(path);
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// Find all related device nodes (event, hidraw, js) for a given device path
fn find_related_input_paths(dev_path: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let mut paths = vec![dev_path.to_string()];
    let mut enumerator = Enumerator::new()?;
    enumerator.match_subsystem("input")?;

    // First, find the device and its parent syspath
    let mut target_parent_syspath: Option<PathBuf> = None;
    for device in enumerator.scan_devices()? {
        if let Some(devnode) = device.devnode() {
            if devnode.to_str() == Some(dev_path) {
                if let Some(parent) = device.parent() {
                    target_parent_syspath = Some(parent.syspath().to_path_buf());
                }
                break;
            }
        }
    }

    // Now, collect all siblings with the same parent syspath
    if let Some(parent_syspath) = target_parent_syspath {
        for device in enumerator.scan_devices()? {
            if let Some(parent) = device.parent() {
                if parent.syspath() == parent_syspath {
                    if let Some(devnode) = device.devnode() {
                        paths.push(devnode.to_string_lossy().to_string());
                    }
                }
            }
        }
    }

    // Remove duplicates
    paths.sort();
    paths.dedup();
    Ok(paths)
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
