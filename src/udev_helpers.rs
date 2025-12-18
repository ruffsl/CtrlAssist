use gilrs::Gamepad;
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use udev::Enumerator;

use gilrs::Gilrs;

/// Resolves the `/dev/input/event*` path for a given Gilrs `GamepadId` by matching
/// the device's name and, if available, its vendor and product IDs. This function attempts
/// to uniquely identify the correct event node for a gamepad, but may return the wrong device
/// if multiple devices share the same name and hardware IDs (e.g., identical controllers).
///
/// # Parameters
/// - `target_id`: The `GamepadId` of the target gamepad as provided by Gilrs.
///
/// # Returns
/// - `Some(PathBuf)`: The path to the matching `/dev/input/event*` node if found.
/// - `None`: If no matching device is found or an error occurs.
///
/// # Caveats
/// - If multiple devices have the same name and hardware IDs, the first match is returned.
///   This may not always be the intended device.
/// - If vendor/product IDs are unavailable, only the device name is used for matching.
///
/// # Failure Cases
/// - Returns `None` if the Gilrs context cannot be created, the gamepad is not found,
///   or no matching event device is found.
pub fn resolve_event_path(target_id: gilrs::GamepadId) -> Option<PathBuf> {
    let gilrs = Gilrs::new().ok()?;
    let gamepad = gilrs.gamepad(target_id);
    let target_name = gamepad.os_name();
    let target_vendor = gamepad.vendor_id();
    let target_product = gamepad.product_id();

    std::fs::read_dir("/dev/input")
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .map(|f| f.to_string_lossy().starts_with("event"))
                .unwrap_or(false)
                && evdev::Device::open(path).ok().is_some_and(|device| {
                    let name_match = device.name().map(|n| n == target_name).unwrap_or(false);
                    let input_id = device.input_id();
                    let vendor_match = target_vendor.is_none_or(|tv| tv == input_id.vendor());
                    let product_match = target_product.is_none_or(|tp| tp == input_id.product());
                    name_match && vendor_match && product_match
                })
        })
}

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
