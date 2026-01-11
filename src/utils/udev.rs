use crate::HideType;
use crate::utils::gilrs::GamepadResource;
use std::collections::HashSet;
use std::error::Error;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use udev::{Device, Enumerator};

const MODE_HIDDEN: u32 = 0o600;
const MODE_RESTORED: u32 = 0o660;

pub struct ScopedDeviceHider {
    state: HiderState,
}

enum HiderState {
    None,
    System {
        hidden_paths: HashSet<PathBuf>,
    },
    Steam {
        config_path: PathBuf,
        added_ids: HashSet<String>,
    },
}

impl ScopedDeviceHider {
    pub fn new(hide_type: HideType) -> Self {
        let state = match hide_type {
            HideType::None => HiderState::None,
            HideType::System => HiderState::System {
                hidden_paths: HashSet::new(),
            },
            HideType::Steam => HiderState::Steam {
                config_path: dirs::home_dir()
                    .map(|h| h.join(".local/share/Steam/config/config.vdf"))
                    .unwrap_or_default(),
                added_ids: HashSet::new(),
            },
        };
        Self { state }
    }

    pub fn hide_gamepad_devices(
        &mut self,
        resource: &GamepadResource,
    ) -> Result<(), Box<dyn Error>> {
        match &mut self.state {
            HiderState::None => Ok(()),

            HiderState::System { hidden_paths } => {
                let path = resource.path.as_path();
                let Some(device) = find_udev_device(path)? else {
                    return hide_path_fs(path, hidden_paths);
                };

                let root = find_physical_root(device);
                for node_path in find_child_devnodes(&root)? {
                    hide_path_fs(&node_path, hidden_paths)?;
                }
                Ok(())
            }

            HiderState::Steam {
                config_path,
                added_ids,
            } => {
                if config_path.as_os_str().is_empty() {
                    return Err("Home directory not found; cannot locate Steam config".into());
                }

                let input_id = resource.device.input_id();
                let id = format!("{:04x}/{:04x}", input_id.vendor(), input_id.product());

                if added_ids.insert(id.clone()) {
                    modify_steam_config(config_path, |blacklist| {
                        blacklist.insert(id);
                    })?;
                    log::info!("Added {} to Steam blacklist", resource.path.display());
                }
                Ok(())
            }
        }
    }
}

impl Drop for ScopedDeviceHider {
    fn drop(&mut self) {
        let result = match &mut self.state {
            HiderState::None => Ok(()),

            HiderState::System { hidden_paths } => {
                for path in hidden_paths.drain() {
                    if let Err(e) =
                        fs::set_permissions(&path, fs::Permissions::from_mode(MODE_RESTORED))
                    {
                        log::error!("Failed to restore {}: {}", path.display(), e);
                    } else {
                        log::info!("Restored (system): {}", path.display());
                    }
                }
                Ok(())
            }

            HiderState::Steam {
                config_path,
                added_ids,
            } => {
                if !added_ids.is_empty() && config_path.exists() {
                    log::info!("Removing added IDs from Steam blacklist...");
                    modify_steam_config(config_path, |blacklist| {
                        for id in added_ids.iter() {
                            blacklist.remove(id);
                        }
                    })
                } else {
                    Ok(())
                }
            }
        };

        if let Err(e) = result {
            log::error!("Error during cleanup: {}", e);
        }
    }
}

fn modify_steam_config<F>(path: &Path, modifier: F) -> Result<(), Box<dyn Error>>
where
    F: FnOnce(&mut HashSet<String>),
{
    let content = fs::read_to_string(path)?;
    let mut lines: Vec<String> = content.lines().map(String::from).collect();

    let (line_idx, current_val) = match lines
        .iter()
        .enumerate()
        .find(|(_, l)| l.trim().starts_with("\"controller_blacklist\""))
        .map(|(i, l)| (i, parse_vdf_value(l)))
    {
        Some((i, v)) => (i, v),
        None => {
            // Fallback: insert inside the InstallConfigStore block, just after its opening brace.
            let install_idx = lines
                .iter()
                .position(|l| l.contains("\"InstallConfigStore\""))
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "Steam config missing InstallConfigStore",
                    )
                })?;

            // Check if the opening brace is on the same line as InstallConfigStore
            let install_line = &lines[install_idx];
            let brace_idx = if install_line.contains('{') {
                install_idx
            } else {
                // Otherwise, find the next non-empty, non-whitespace-only line that starts with '{'
                lines
                    .iter()
                    .skip(install_idx + 1)
                    .position(|l| !l.trim().is_empty() && l.trim_start().starts_with('{'))
                    .map(|rel| install_idx + 1 + rel)
                    .ok_or_else(|| {
                        io::Error::new(
                            io::ErrorKind::InvalidData,
                            "Steam config InstallConfigStore block missing opening '{'",
                        )
                    })?
            };

            (brace_idx + 1, None)
        }
    };

    let mut blacklist: HashSet<String> = current_val
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();

    modifier(&mut blacklist);

    let mut sorted_ids: Vec<_> = blacklist.into_iter().collect();
    sorted_ids.sort();
    let new_val = sorted_ids.join(",");

    let new_line = format!("\t\"controller_blacklist\"\t\t\"{}\"", new_val);

    if current_val.is_some() {
        lines[line_idx] = new_line;
    } else {
        lines.insert(line_idx, new_line);
    }

    let tmp_path = path.with_extension("tmp");
    let mut file = fs::File::create(&tmp_path)?;
    file.write_all(lines.join("\n").as_bytes())?;
    file.sync_all()?;
    fs::rename(tmp_path, path)?;

    Ok(())
}

fn parse_vdf_value(line: &str) -> Option<String> {
    line.split('"').nth(3).map(String::from)
}

fn hide_path_fs(path: &Path, tracker: &mut HashSet<PathBuf>) -> Result<(), Box<dyn Error>> {
    if !tracker.contains(path) {
        fs::set_permissions(path, fs::Permissions::from_mode(MODE_HIDDEN))?;
        tracker.insert(path.to_path_buf());
        log::info!("Hidden (system): {}", path.display());
    }
    Ok(())
}

fn find_udev_device(path: &Path) -> io::Result<Option<Device>> {
    let mut enumerator = Enumerator::new()?;
    enumerator.match_subsystem("input")?;
    Ok(enumerator
        .scan_devices()?
        .find(|d| d.devnode() == Some(path)))
}

fn find_physical_root(start: Device) -> Device {
    std::iter::successors(Some(start.clone()), |d| d.parent())
        .find(|d| {
            d.subsystem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| matches!(s, "usb" | "bluetooth"))
        })
        .unwrap_or(start)
}

fn find_child_devnodes(parent: &Device) -> io::Result<Vec<PathBuf>> {
    let mut enumerator = Enumerator::new()?;
    enumerator.match_parent(parent)?;
    Ok(enumerator
        .scan_devices()?
        .filter(|d| {
            d.subsystem()
                .and_then(|s| s.to_str())
                .is_some_and(|s| matches!(s, "input" | "hidraw"))
        })
        .filter_map(|d| d.devnode().map(PathBuf::from))
        .collect())
}
