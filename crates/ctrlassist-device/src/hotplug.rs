//! Which pads are connected, and when that changes.
//!
//! Both come from udev, after it has finished with a device: a pad is
//! reported once its nodes and their permissions are in place, so it can
//! be opened at once.

use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::PathBuf;

use crate::pad::Identifiers;

/// A connected pad, before it is opened.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Found {
    /// Its event node.
    pub node: PathBuf,
    /// What tells it from others.
    pub identifiers: Identifiers,
}

impl Found {
    /// Whether this is one of the daemon's own virtual pads.
    #[must_use]
    pub fn is_own_output(&self) -> bool {
        self.identifiers.is_own_output()
    }
}

/// A change in which pads are connected.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Change {
    /// A pad has connected, or one already connected changed.
    Arrived(Found),
    /// The pad whose event node this was has gone.
    Left(PathBuf),
}

fn attribute(device: &udev::Device, name: &str) -> Option<String> {
    let text = device.attribute_value(name)?.to_str()?.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn hex(text: Option<String>) -> u16 {
    text.and_then(|text| u16::from_str_radix(&text, 16).ok())
        .unwrap_or(0)
}

/// Whether a udev device is a pad's event node.
fn is_pad(device: &udev::Device) -> bool {
    device
        .sysname()
        .to_str()
        .is_some_and(|name| name.starts_with("event"))
        && device
            .property_value("ID_INPUT_JOYSTICK")
            .is_some_and(|value| value == "1")
}

/// The pad a udev event device is, read without opening it.
fn found(device: &udev::Device) -> Option<Found> {
    if !is_pad(device) {
        return None;
    }
    let node = device.devnode()?.to_path_buf();
    let input = device.parent()?;
    let serial = device
        .property_value("ID_SERIAL_SHORT")
        .and_then(|value| value.to_str())
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    Some(Found {
        node,
        identifiers: Identifiers {
            name: attribute(&input, "name").unwrap_or_default(),
            bus: hex(attribute(&input, "id/bustype")),
            vendor: hex(attribute(&input, "id/vendor")),
            product: hex(attribute(&input, "id/product")),
            version: hex(attribute(&input, "id/version")),
            address: attribute(&input, "uniq"),
            serial,
            port: attribute(&input, "phys"),
        },
    })
}

/// Every pad connected now, the daemon's own virtual pads included; see
/// [`Found::is_own_output`].
///
/// # Errors
///
/// Fails if udev can't be asked.
pub fn connected() -> io::Result<Vec<Found>> {
    let mut inputs = udev::Enumerator::new()?;
    inputs.match_subsystem("input")?;
    Ok(inputs
        .scan_devices()?
        .filter_map(|device| found(&device))
        .collect())
}

/// Watches pads connect and disconnect.
pub struct Monitor {
    socket: udev::MonitorSocket,
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor").finish_non_exhaustive()
    }
}

impl Monitor {
    /// Starts watching. Changes from now on are kept until read, so
    /// calling this before [`connected`] misses nothing between the two.
    ///
    /// # Errors
    ///
    /// Fails if udev can't be listened to.
    pub fn new() -> io::Result<Self> {
        let socket = udev::MonitorBuilder::new()?
            .match_subsystem("input")?
            .listen()?;
        Ok(Self { socket })
    }

    /// Passes every change waiting to `change`, and returns when none is
    /// left. The monitor is readable, as a descriptor, when one is.
    pub fn read(&mut self, mut change: impl FnMut(Change)) {
        for event in self.socket.iter() {
            let device = event.device();
            match event.event_type() {
                udev::EventType::Add | udev::EventType::Change => {
                    if let Some(found) = found(&device) {
                        change(Change::Arrived(found));
                    }
                }
                udev::EventType::Remove => {
                    // What it was is gone with it; only its node is left
                    // to say which pad this was.
                    let is_event = device
                        .sysname()
                        .to_str()
                        .is_some_and(|name| name.starts_with("event"));
                    if let (true, Some(node)) = (is_event, device.devnode()) {
                        change(Change::Left(node.to_path_buf()));
                    }
                }
                _ => {}
            }
        }
    }
}

impl AsFd for Monitor {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }
}
