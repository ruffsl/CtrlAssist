use evdev::Device;
use evdev::InputId;
use evdev::uinput::VirtualDevice;
use gilrs::{GamepadId, Gilrs};
use log::error;
use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

const RETRY_INTERVAL: Duration = Duration::from_millis(50);
const VIRTUAL_DEV_TIMEOUT: Duration = Duration::from_secs(2);

/// Represents a physical gamepad and its associated Linux event device.
pub struct GamepadResource {
    pub name: String,
    pub path: PathBuf,
    pub device: Device,
}

impl Clone for GamepadResource {
    fn clone(&self) -> Self {
        GamepadResource {
            name: self.name.clone(),
            path: self.path.clone(),
            device: Device::open(&self.path).expect("Failed to clone device handle"),
        }
    }
}

pub fn wait_for_virtual_device(
    v_dev: &mut VirtualDevice,
) -> Result<GamepadResource, Box<dyn Error>> {
    let v_path = v_dev
        .enumerate_dev_nodes_blocking()?
        .filter_map(Result::ok)
        .find(|pb| pb.to_string_lossy().contains("event"))
        .ok_or("Could not find virtual device path")?;

    let start = Instant::now();
    while start.elapsed() < VIRTUAL_DEV_TIMEOUT {
        if let Ok(dev) = Device::open(&v_path) {
            let resource = GamepadResource {
                name: dev.name().unwrap().to_string(),
                device: dev,
                path: v_path.clone(),
            };
            return Ok(resource);
        }
        thread::sleep(RETRY_INTERVAL);
    }
    Err("Timed out waiting for virtual device".into())
}

/// Computes the gilrs gamepad UUID for the Linux platform.
/// This is adapted from gilrs-core for evdev::InputId.
pub fn create_uuid(iid: InputId) -> Uuid {
    let bus = iid.bus_type().0 as u32;
    let vendor = iid.vendor();
    let product = iid.product();
    let version = iid.version();
    Uuid::from_fields(
        bus.to_be(),
        vendor.to_be(),
        0,
        &[
            product as u8,
            (product >> 8) as u8,
            0,
            0,
            version as u8,
            (version >> 8) as u8,
            0,
            0,
        ],
    )
}

/// Matches Gilrs gamepads to /dev/input/event* nodes.
pub fn discover_gamepad_resources(gilrs: &Gilrs) -> HashMap<GamepadId, GamepadResource> {
    let mut resources = HashMap::new();
    let mut available_paths: HashSet<PathBuf> = fs::read_dir("/dev/input")
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|s| s.starts_with("event"))
        })
        .collect();

    for (id, gamepad) in gilrs.gamepads() {
        let mut matched_path = None;

        for path in &available_paths {
            if let Ok(device) = Device::open(path) {
                let input_id = device.input_id();
                let name_match = device.name().is_some_and(|n| n == gamepad.os_name());
                let uuid_match = Uuid::from_bytes(gamepad.uuid()) == create_uuid(input_id);

                if name_match && uuid_match {
                    matched_path = Some((path.clone(), device));
                    break;
                }
            }
        }

        if let Some((path, device)) = matched_path {
            available_paths.remove(&path);
            resources.insert(
                id,
                GamepadResource {
                    name: gamepad.name().to_string(),
                    path,
                    device,
                },
            );
        } else {
            error!(
                "Failed to match Gilrs gamepad {:?} ('{}') to a Linux event device.",
                id,
                gamepad.name()
            );
        }
    }
    resources
}
