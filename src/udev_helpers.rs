use gilrs::Gamepad;
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use udev::Enumerator;

/// Gets a udev property as an Option<String>.
fn get_udev_prop(device: &udev::Device, prop: &str) -> Option<String> {
    device
        .property_value(prop)
        .and_then(|s| s.to_str())
        .map(String::from)
}

/// Finds the main parent device's syspath from a matching gamepad.
/// This searches for a device matching the gamepad's Vendor and Product.
fn find_parent_syspath(
    enumerator: &mut Enumerator,
    target_vendor: Option<&str>,
    target_product: Option<&str>,
) -> Result<Option<PathBuf>, Box<dyn Error>> {
    for device in enumerator.scan_devices()? {
        let dev_vendor = get_udev_prop(&device, "ID_VENDOR_ID");
        let dev_product = get_udev_prop(&device, "ID_MODEL_ID");

        // Check for a match only on vendor and product.
        let is_match =
            target_vendor == dev_vendor.as_deref() && target_product == dev_product.as_deref();

        if is_match {
            // Found a match. Now find its "physical" parent device.
            // We walk up the tree until we find the main "usb_device" or "bluetooth" device.

            // Get the immediate parent and clone it so we can walk up the tree
            // without move/borrow conflicts.
            if let Some(immediate_parent) = device.parent() {
                let mut walker = immediate_parent.clone(); // Clone to own it
                loop {
                    let subsystem = walker.subsystem().and_then(|s| s.to_str());
                    if subsystem == Some("usb") || subsystem == Some("bluetooth") {
                        // Found the "physical" root for this device
                        return Ok(Some(walker.syspath().to_path_buf()));
                    }
                    // Try to go up one more level
                    if let Some(next_parent) = walker.parent() {
                        walker = next_parent;
                    } else {
                        // We're at the root and didn't find "usb".
                        // Fallback: return the syspath of the *immediate* parent.
                        return Ok(Some(immediate_parent.syspath().to_path_buf()));
                    }
                }
            }
            // If the device has no parent, we find nothing.
        }
    }
    Ok(None)
}

/// Restrict access to all device nodes related to a physical gamepad.
pub fn restrict_gamepad_devices(
    gamepad: &Gamepad,
    restricted_paths: &mut HashSet<String>,
) -> Result<(), Box<dyn Error>> {
    // 1. Get target properties from gilrs::Gamepad
    // Format vendor/product as 4-digit hex strings to match udev properties
    let target_vendor = gamepad.vendor_id().map(|v| format!("{:04x}", v));
    let target_product = gamepad.product_id().map(|p| format!("{:04x}", p));

    // 2. Find the single "parent" syspath for this physical device
    // We scan "input" and "hidraw" as these are most likely to have the properties.
    let mut parent_syspath = None;
    let mut enumerator = Enumerator::new()?;

    for subsystem in ["input", "hidraw"] {
        enumerator.match_subsystem(subsystem)?;
        parent_syspath = find_parent_syspath(
            &mut enumerator,
            target_vendor.as_deref(),
            target_product.as_deref(),
        )?;
        if parent_syspath.is_some() {
            break;
        }
    }

    let Some(parent_syspath) = parent_syspath else {
        // No matching device found in udev.
        // This is not an error, just means we can't restrict.
        eprintln!(
            "Warning: Could not find matching udev device for {}.",
            gamepad.name()
        );
        return Ok(());
    };

    // 3. Find all child devnodes that share this parent
    let mut paths_to_restrict = HashSet::new();
    let mut enumerator = Enumerator::new()?;

    for subsystem in ["input", "hidraw"] {
        enumerator.match_subsystem(subsystem)?;
        for device in enumerator.scan_devices()? {
            // Clone device so we can walk up parent chain without move/borrow error
            let mut current = device.clone();
            while let Some(parent) = current.parent() {
                if parent.syspath() == parent_syspath {
                    // This device is a child. Get its devnode.
                    if let Some(devnode) = device.devnode() {
                        paths_to_restrict.insert(devnode.to_string_lossy().to_string());
                    }
                    break; // Stop walking up
                }
                current = parent;
            }
        }
    }

    // 4. Restrict all found device paths
    for path in paths_to_restrict {
        if restrict_device(&path).is_ok() && !restricted_paths.contains(&path) {
            log::info!("Hidden: {}", path);
            restricted_paths.insert(path);
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
