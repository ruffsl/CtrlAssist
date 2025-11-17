#[repr(C)]
#[derive(Default, Debug)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

const EVIOCGID: libc::c_ulong = 0x80084502;
use gilrs::Gamepad;
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use udev::Enumerator;
use uuid::Uuid;

/// Restrict access to gamepad devices matching vendor and product IDs
pub fn restrict_gamepad_devices(
    gamepad: &Gamepad,
    restricted_paths: &mut HashSet<String>,
) -> Result<(), Box<dyn Error>> {
    let target_uuid = Uuid::from_bytes(gamepad.uuid());

    for subsystem in ["input", "hidraw"] {
        let mut enumerator = Enumerator::new()?;
        enumerator.match_subsystem(subsystem)?;

        for device in enumerator.scan_devices()? {
            let devnode = match device.devnode() {
                Some(d) => d,
                None => continue,
            };
            let path = &devnode;
            let fd = match std::fs::File::open(path) {
                Ok(f) => f,
                Err(_) => continue,
            };
            use std::os::unix::io::AsRawFd;
            let raw_fd = fd.as_raw_fd();
            let mut input_id = InputId::default();
            if unsafe { libc::ioctl(raw_fd, EVIOCGID, &mut input_id) } != 0 {
                continue;
            }
            let bus_u32 = (input_id.bustype as u32).to_be();
            let vendor_u16 = input_id.vendor.to_be();
            let product_u16 = input_id.product.to_be();
            let version_u16 = input_id.version.to_be();
            let uuid = Uuid::from_fields(
                bus_u32,
                vendor_u16,
                0,
                &[
                    (product_u16 >> 8) as u8,
                    product_u16 as u8,
                    0,
                    0,
                    (version_u16 >> 8) as u8,
                    version_u16 as u8,
                    0,
                    0,
                ],
            );
            if uuid != target_uuid {
                continue;
            }
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
    Ok(())
}

/// Find all related device nodes (event, hidraw, js) for a given device path
fn find_related_input_paths(dev_path: &str) -> Result<Vec<String>, Box<dyn Error>> {
    let mut paths = vec![dev_path.to_string()];

    // Find parent syspath from input subsystem
    let mut enumerator = Enumerator::new()?;
    enumerator.match_subsystem("input")?;
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

    // Collect all siblings from input subsystem
    if let Some(parent_syspath) = target_parent_syspath {
        let mut enumerator = Enumerator::new()?;
        enumerator.match_subsystem("input")?;
        for device in enumerator.scan_devices()? {
            if let Some(parent) = device.parent() {
                if parent.syspath() == parent_syspath {
                    if let Some(devnode) = device.devnode() {
                        paths.push(devnode.to_string_lossy().to_string());
                    }
                }
            }
        }

        // Now, also search hidraw subsystem for matching vendor/product/serial
        // First, get vendor/product/serial from the input device
        let mut vendor_id = None;
        let mut product_id = None;
        let mut serial = None;
        let mut enumerator_input = Enumerator::new()?;
        enumerator_input.match_subsystem("input")?;
        for device in enumerator_input.scan_devices()? {
            if let Some(devnode) = device.devnode() {
                if devnode.to_str() == Some(dev_path) {
                    vendor_id = device
                        .property_value("ID_VENDOR_ID")
                        .map(|s| s.to_string_lossy().to_string());
                    product_id = device
                        .property_value("ID_MODEL_ID")
                        .map(|s| s.to_string_lossy().to_string());
                    serial = device
                        .property_value("ID_SERIAL_SHORT")
                        .map(|s| s.to_string_lossy().to_string());
                    break;
                }
            }
        }

        if let (Some(vendor), Some(product)) = (vendor_id, product_id) {
            let mut enumerator_hidraw = Enumerator::new()?;
            enumerator_hidraw.match_subsystem("hidraw")?;
            for device in enumerator_hidraw.scan_devices()? {
                let v = device
                    .property_value("ID_VENDOR_ID")
                    .map(|s| s.to_string_lossy().to_string());
                let p = device
                    .property_value("ID_MODEL_ID")
                    .map(|s| s.to_string_lossy().to_string());
                let s = device
                    .property_value("ID_SERIAL_SHORT")
                    .map(|s| s.to_string_lossy().to_string());
                if v == Some(vendor.clone()) && p == Some(product.clone()) {
                    // If serial is present, match it too
                    if let Some(ref serial_val) = serial {
                        if s != Some(serial_val.clone()) {
                            continue;
                        }
                    }
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
