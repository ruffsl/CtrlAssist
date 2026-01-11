use evdev::Device;
use evdev::uinput::VirtualDevice;
use gilrs::{GamepadId, Gilrs, GilrsBuilder, LinuxGamepadExt};
use std::collections::HashMap;
use std::error::Error;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

const RETRY_INTERVAL: Duration = Duration::from_millis(50);
const VIRTUAL_DEV_TIMEOUT: Duration = Duration::from_secs(2);

#[allow(clippy::result_large_err)]
/// Helper to create Gilrs with CtrlAssist's preferred settings
pub fn new_gilrs() -> Result<Gilrs, gilrs::Error> {
    GilrsBuilder::new().with_force_feedback(false).build()
}

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

/// Matches Gilrs gamepads to their evdev devices using the devpath API
pub fn discover_gamepad_resources(gilrs: &Gilrs) -> HashMap<GamepadId, GamepadResource> {
    let mut resources = HashMap::new();

    for (id, gamepad) in gilrs.gamepads() {
        let path = gamepad.devpath().to_path_buf();
        match Device::open(&path) {
            Ok(device) => {
                resources.insert(
                    id,
                    GamepadResource {
                        name: gamepad.name().to_string(),
                        path: path.clone(),
                        device,
                    },
                );
            }
            Err(e) => {
                eprintln!(
                    "Failed to open device {} for gamepad {}: {}",
                    path.display(),
                    gamepad.name(),
                    e
                );
            }
        }
    }
    resources
}
