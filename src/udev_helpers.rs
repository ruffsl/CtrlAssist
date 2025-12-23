use crate::gilrs_helper::GamepadResource;
use crate::HideType;
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use udev::{Device, Enumerator};

const MODE_ROOT_ONLY: u32 = 0o600;
const MODE_ROOT_GROUP: u32 = 0o660;

/// A RAII guard that hides devices and automatically restores them when dropped.
pub struct ScopedDeviceHider {
    hide_type: HideType,
    system_state: SystemHideState,
    steam_state: SteamHideState,
}

/// Tracks system-level permission changes
struct SystemHideState {
    hidden_paths: HashSet<PathBuf>,
}

/// Tracks Steam config modifications
struct SteamHideState {
    config_path: PathBuf,
    original_blacklist: Option<String>,
    added_ids: Vec<String>,
}

impl ScopedDeviceHider {
    pub fn new(hide_type: HideType) -> Self {
        let steam_config_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".local/share/Steam/config/config.vdf");

        Self {
            hide_type,
            system_state: SystemHideState {
                hidden_paths: HashSet::new(),
            },
            steam_state: SteamHideState {
                config_path: steam_config_path,
                original_blacklist: None,
                added_ids: Vec::new(),
            },
        }
    }

    /// Hide a gamepad device according to the configured hide type
    pub fn hide_gamepad_devices(
        &mut self,
        resource: &GamepadResource,
    ) -> Result<(), Box<dyn Error>> {
        match self.hide_type {
            HideType::None => Ok(()),
            HideType::System => self.hide_system(resource),
            HideType::Steam => self.hide_steam(resource),
        }
    }

    /// System hiding: restrict device permissions
    fn hide_system(&mut self, resource: &GamepadResource) -> Result<(), Box<dyn Error>> {
        let event_path = resource.path.as_path();

        // Find the specific udev device
        let device = match find_device_by_path(event_path)? {
            Some(d) => d,
            None => {
                self.system_state.hide_and_track(event_path);
                return Ok(());
            }
        };

        // Find the physical parent and all related nodes
        let physical_root = find_physical_root(&device);
        let related_nodes = find_related_devnodes(&physical_root)?;

        for node in related_nodes {
            self.system_state.hide_and_track(&node);
        }

        Ok(())
    }

    /// Steam hiding: add controller to Steam's blacklist
    fn hide_steam(&mut self, resource: &GamepadResource) -> Result<(), Box<dyn Error>> {
        // Extract vendor/product IDs directly from evdev Device
        let input_id = resource.device.input_id();
        let vendor_id = input_id.vendor();
        let product_id = input_id.product();
        let id_pair = format!("{:04x}/{:04x}", vendor_id, product_id);

        // Skip if already added
        if self.steam_state.added_ids.contains(&id_pair) {
            return Ok(());
        }

        log::info!("Adding {} to Steam blacklist", id_pair);

        // Read and modify Steam config
        if self.steam_state.original_blacklist.is_none() {
            // First time - backup original config
            let config_content = fs::read_to_string(&self.steam_state.config_path)
                .map_err(|e| format!("Failed to read Steam config: {}", e))?;

            let original_blacklist = parse_controller_blacklist(&config_content);
            self.steam_state.original_blacklist = Some(original_blacklist.unwrap_or_default());
        }

        // Add new ID
        self.steam_state.added_ids.push(id_pair.clone());

        // Build new blacklist
        let mut all_ids = Vec::new();
        if let Some(original) = &self.steam_state.original_blacklist {
            if !original.is_empty() {
                all_ids.push(original.clone());
            }
        }
        all_ids.extend(self.steam_state.added_ids.clone());
        let new_blacklist = all_ids.join(",");

        // Update config file
        update_steam_config(&self.steam_state.config_path, &new_blacklist)?;

        Ok(())
    }
}

impl SystemHideState {
    fn hide_and_track(&mut self, path: &Path) {
        // Skip if we are already tracking this path to avoid redundant syscalls
        if self.hidden_paths.contains(path) {
            return;
        }

        match set_permissions(path, MODE_ROOT_ONLY) {
            Ok(_) => {
                self.hidden_paths.insert(path.to_path_buf());
                log::info!("Hidden (system): {}", path.display());
            }
            Err(e) => log::warn!("Failed to hide {}: {}", path.display(), e),
        }
    }
}

// Ensure devices are restored when the struct goes out of scope (e.g. app exit/panic).
impl Drop for ScopedDeviceHider {
    fn drop(&mut self) {
        match self.hide_type {
            HideType::None => {}
            HideType::System => {
                // Restore system permissions
                for path in &self.system_state.hidden_paths {
                    if let Err(e) = set_permissions(path, MODE_ROOT_GROUP) {
                        log::error!("Failed to restore {}: {}", path.display(), e);
                    } else {
                        log::info!("Restored (system): {}", path.display());
                    }
                }
            }
            HideType::Steam => {
                // Restore original Steam config
                if let Some(original) = &self.steam_state.original_blacklist {
                    if let Err(e) = update_steam_config(&self.steam_state.config_path, original) {
                        log::error!("Failed to restore Steam config: {}", e);
                    } else {
                        log::info!("Restored Steam blacklist to original state");
                    }
                }
            }
        }
    }
}

// --- Steam Config Helpers ---

/// Parse the controller_blacklist value from Steam's VDF config
fn parse_controller_blacklist(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("\"controller_blacklist\"") {
            // Extract value between quotes after the key
            if let Some(start) = trimmed.find('\t') {
                let value_part = &trimmed[start..].trim();
                if let Some(quote_start) = value_part.find('"') {
                    let after_quote = &value_part[quote_start + 1..];
                    if let Some(quote_end) = after_quote.find('"') {
                        return Some(after_quote[..quote_end].to_string());
                    }
                }
            }
        }
    }
    None
}

/// Update Steam's config.vdf with new controller blacklist
fn update_steam_config(config_path: &Path, new_blacklist: &str) -> Result<(), Box<dyn Error>> {
    let content = fs::read_to_string(config_path)?;
    let mut lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();

    let mut found = false;
    let mut install_config_idx = None;

    // Find InstallConfigStore section
    for (idx, line) in lines.iter().enumerate() {
        if line.contains("\"InstallConfigStore\"") {
            install_config_idx = Some(idx);
        }
        if line.trim().starts_with("\"controller_blacklist\"") {
            // Replace existing line
            let indent = line.chars().take_while(|c| c.is_whitespace()).collect::<String>();
            lines[idx] = format!("{}\"controller_blacklist\"\t\"{}\"", indent, new_blacklist);
            found = true;
            break;
        }
    }

    // If not found, add after InstallConfigStore opening brace
    if !found {
        if let Some(idx) = install_config_idx {
            // Find the opening brace
            if let Some(brace_idx) = lines[idx..].iter().position(|l| l.contains('{')) {
                let insert_idx = idx + brace_idx + 1;
                lines.insert(insert_idx, format!("\t\"controller_blacklist\"\t\"{}\"", new_blacklist));
            }
        } else {
            return Err("Could not find InstallConfigStore in Steam config".into());
        }
    }

    // Write back
    let new_content = lines.join("\n");
    let mut file = fs::File::create(config_path)?;
    file.write_all(new_content.as_bytes())?;

    Ok(())
}

// --- Device Discovery Helpers ---

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
