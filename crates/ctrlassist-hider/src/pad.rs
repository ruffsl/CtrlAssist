//! Deciding what a descriptor is, what names its pad to the rule, and
//! where the pad is after its driver has been re-bound.
//!
//! Everything read from a device here is treated as hostile: a seat user
//! can create devices with any name and address. Only numbers the service
//! parses itself, and strings that pass the narrow checks below, are ever
//! used to name a marker.

use std::collections::BTreeSet;
use std::os::fd::BorrowedFd;
use std::path::{Path, PathBuf};

use ctrlassist_hider::Refusal;

/// What tells this pad from others, beyond its markers.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Identity {
    pub(crate) bus: u16,
    pub(crate) vendor: u16,
    pub(crate) product: u16,
    /// The seat its event node is on.
    pub(crate) seat: String,
    /// Whether its device sits under `/sys/devices/virtual`, as any
    /// uhid or uinput device does, wherever a seat user made it.
    pub(crate) virtual_device: bool,
}

/// A pad the service has checked: the device whose driver is re-bound to
/// cut its readers off, and what names it to the rule.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Pad {
    /// The pad's HID device, or with none its USB interface: the device
    /// that was checked, which keeps its place in sysfs through a re-bind
    /// of its driver.
    pub(crate) parent: PathBuf,
    /// Which of the joystick event nodes under `parent` it is, in order.
    pub(crate) ordinal: usize,
    pub(crate) identity: Identity,
    /// The marker files that hide it, by name.
    pub(crate) markers: BTreeSet<String>,
}

/// `aa:bb:cc:dd:ee:ff`, lowercase.
pub(crate) fn is_address(text: &str) -> bool {
    text.len() == 17
        && text.bytes().enumerate().all(|(index, byte)| {
            if index % 3 == 2 {
                byte == b':'
            } else {
                matches!(byte, b'0'..=b'9' | b'a'..=b'f')
            }
        })
}

fn is_word(text: &str, allowed: impl Fn(u8) -> bool) -> bool {
    !text.is_empty() && text.len() <= 64 && text.bytes().all(allowed)
}

pub(crate) fn is_serial(text: &str) -> bool {
    is_word(text, |byte| byte.is_ascii_alphanumeric())
}

/// A HID device's own unique string, as its driver set it: an address on
/// Bluetooth, a serial number on USB.
pub(crate) fn is_uniq(text: &str) -> bool {
    is_word(text, |byte| byte.is_ascii_alphanumeric() || byte == b':')
}

/// A HID device's kernel name, exactly as the kernel forms it: bus,
/// vendor and product as four uppercase hex digits each, then an instance
/// number of four or more, such as `0003:054C:0CE6.0017`.
pub(crate) fn is_hid_name(text: &str) -> bool {
    let hex = |part: &str| {
        part.bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'A'..=b'F'))
    };
    let Some((ids, instance)) = text.split_once('.') else {
        return false;
    };
    let parts: Vec<&str> = ids.split(':').collect();
    parts.len() == 3
        && parts.iter().all(|part| part.len() == 4 && hex(part))
        && (4..=8).contains(&instance.len())
        && hex(instance)
}

/// A HID device's `HID_ID`: bus, vendor and product, such as
/// `0003:0000054C:00000CE6`.
pub(crate) fn is_hid_id(text: &str) -> bool {
    let parts: Vec<&str> = text.split(':').collect();
    parts.len() == 3
        && [4, 8, 8].iter().zip(&parts).all(|(length, part)| {
            part.len() == *length
                && part
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'A'..=b'F'))
        })
}

/// A USB port path as a pad's `phys` gives it, such as
/// `usb-0000:00:14.0-2/input3`: one slash, a plain name before it.
pub(crate) fn is_port(text: &str) -> bool {
    let Some((port, interface)) = text.split_once('/') else {
        return false;
    };
    let plain =
        |byte: u8| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'.' | b'-' | b'_');
    is_word(port, plain)
        && port.starts_with("usb-")
        && !port.contains("..")
        && interface.strip_prefix("input").is_some_and(|number| {
            (1..=3).contains(&number.len()) && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn attribute(device: &udev::Device, name: &str) -> Option<String> {
    Some(device.attribute_value(name)?.to_str()?.trim().to_string())
}

fn property<'a>(device: &'a udev::Device, name: &str) -> Option<&'a str> {
    device.property_value(name)?.to_str()
}

fn flag(device: &udev::Device, name: &str) -> bool {
    property(device, name) == Some("1")
}

fn subsystem(device: &udev::Device) -> Option<&str> {
    device.subsystem()?.to_str()
}

fn hex(text: &str) -> Option<u16> {
    u16::from_str_radix(text, 16).ok()
}

fn is_event(device: &udev::Device) -> bool {
    subsystem(device) == Some("input")
        && device
            .sysname()
            .to_str()
            .is_some_and(|name| name.starts_with("event"))
}

/// A joystick, and nothing a keystroke or a pointer could come from: one
/// HID interface can carry a keyboard beside a joystick. The rule that
/// lets the service ask for a re-bind makes the same test.
fn is_only_a_pad(event: &udev::Device) -> bool {
    is_event(event)
        && flag(event, "ID_INPUT_JOYSTICK")
        && !flag(event, "ID_INPUT_KEYBOARD")
        && !flag(event, "ID_INPUT_KEY")
        && !flag(event, "ID_INPUT_MOUSE")
        && !flag(event, "ID_INPUT_TOUCHPAD")
        && !flag(event, "ID_INPUT_TABLET")
}

/// The device whose driver is re-bound to cut off an input device's
/// readers: its HID device if it has one bound to a driver, else its USB
/// interface if bound, else none. Returns it with its HID device, if any.
fn parent_of(input: &udev::Device) -> Option<(udev::Device, Option<udev::Device>)> {
    let ancestors = || std::iter::successors(Some(input.clone()), udev::Device::parent);
    let hid = ancestors().find(|device| subsystem(device) == Some("hid"));
    if let Some(hid) = hid {
        let named = hid.sysname().to_str().is_some_and(is_hid_name);
        return (named && hid.driver().is_some()).then(|| (hid.clone(), Some(hid)));
    }
    let interface = ancestors().find(|device| {
        subsystem(device) == Some("usb")
            && device.devtype().and_then(|kind| kind.to_str()) == Some("usb_interface")
    })?;
    interface.driver().is_some().then_some((interface, None))
}

/// The marker files that hide this event node's pad, as the rule names
/// them: by address, else serial number; by HID device; by port where
/// neither; and for its hidraw nodes, by the HID device's own unique
/// string, else its port.
fn markers_of(
    event: &udev::Device,
    input: &udev::Device,
    hid: Option<&udev::Device>,
) -> BTreeSet<String> {
    let mut markers = BTreeSet::new();
    let vendor = attribute(input, "id/vendor").and_then(|text| hex(&text));
    let product = attribute(input, "id/product").and_then(|text| hex(&text));
    let address = attribute(input, "uniq").filter(|uniq| is_address(uniq));
    let serial = property(event, "ID_SERIAL_SHORT").filter(|serial| is_serial(serial));
    if let Some(address) = &address {
        markers.insert(format!("addr-{address}"));
    } else if let (Some(serial), Some(vendor), Some(product)) = (serial, vendor, product) {
        markers.insert(format!("serial-{vendor:04x}-{product:04x}-{serial}"));
    }
    if address.is_none()
        && serial.is_none()
        && let Some(phys) = attribute(input, "phys").filter(|phys| is_port(phys))
    {
        markers.insert(format!("port-{phys}"));
    }
    if let Some(hid) = hid {
        if let Some(name) = hid.sysname().to_str().filter(|name| is_hid_name(name)) {
            markers.insert(format!("hid-{name}"));
        }
        let uniq = property(hid, "HID_UNIQ").filter(|uniq| is_uniq(uniq));
        let id = property(hid, "HID_ID").filter(|id| is_hid_id(id));
        match (uniq, id) {
            (Some(uniq), Some(id)) => {
                markers.insert(format!("uniq-{id}-{uniq}"));
            }
            _ => {
                if let Some(phys) = property(hid, "HID_PHYS").filter(|phys| is_port(phys)) {
                    markers.insert(format!("port-{phys}"));
                }
            }
        }
    }
    markers
}

/// Each joystick event node under `parent`, in the order the kernel made
/// them. Only those udev has finished with have the properties that say
/// what they are.
fn joysticks_under(parent: &Path) -> Vec<udev::Device> {
    let Ok(parent) = udev::Device::from_syspath(parent) else {
        return Vec::new();
    };
    let Ok(mut found) = udev::Enumerator::new() else {
        return Vec::new();
    };
    if found.match_parent(&parent).is_err() || found.match_subsystem("input").is_err() {
        return Vec::new();
    }
    let Ok(devices) = found.scan_devices() else {
        return Vec::new();
    };
    let mut joysticks: Vec<udev::Device> = devices.filter(is_only_a_pad).collect();
    joysticks.sort_by_key(|event| event.parent().and_then(|input| input.sysnum()));
    joysticks
}

/// What an event device is, if it is a pad. `tagged` asks that it carry
/// the seat-access tag, as a pad a seat user could open does; a hidden
/// pad doesn't.
fn classify(event: &udev::Device, tagged: bool) -> Result<Pad, Refusal> {
    let seat_may_open =
        property(event, "CURRENT_TAGS").is_some_and(|tags| tags.contains(":uaccess:"));
    if !is_only_a_pad(event) || (tagged && !seat_may_open) {
        return Err(Refusal::NotAPad);
    }
    let input = event.parent().ok_or(Refusal::NotAPad)?;
    // A pad with no driver to re-bind can't be cut off from whoever has
    // it open; it is the caller's to grab.
    let (parent, hid) = parent_of(&input).ok_or(Refusal::Unsupported)?;
    let ordinal = joysticks_under(parent.syspath())
        .iter()
        .position(|joystick| joystick.syspath() == event.syspath())
        .ok_or(Refusal::NotAPad)?;
    let markers = markers_of(event, &input, hid.as_ref());
    let identity = Identity {
        bus: hex(&attribute(&input, "id/bustype").ok_or(Refusal::NotAPad)?)
            .ok_or(Refusal::NotAPad)?,
        vendor: hex(&attribute(&input, "id/vendor").ok_or(Refusal::NotAPad)?)
            .ok_or(Refusal::NotAPad)?,
        product: hex(&attribute(&input, "id/product").ok_or(Refusal::NotAPad)?)
            .ok_or(Refusal::NotAPad)?,
        seat: property(event, "ID_SEAT").unwrap_or("seat0").to_string(),
        virtual_device: parent.syspath().starts_with("/sys/devices/virtual"),
    };
    Ok(Pad {
        parent: parent.syspath().to_path_buf(),
        ordinal,
        identity,
        markers,
    })
}

/// The event device the descriptor is open on.
/// The pad the descriptor is open on, if it was opened for reading and a
/// seat user could open it. Which seat it is on is the caller's to check.
pub(crate) fn identify(fd: BorrowedFd<'_>) -> Result<Pad, Refusal> {
    let devnum = crate::descriptor::event_node(fd)?;
    let event = udev::Device::from_devnum(udev::DeviceType::Character, devnum)
        .map_err(|_| Refusal::NotAPad)?;
    classify(&event, true)
}

/// The event node of the pad under `parent` that is `ordinal`th, now.
pub(crate) fn event_node(parent: &Path, ordinal: usize) -> Option<PathBuf> {
    joysticks_under(parent)
        .get(ordinal)
        .and_then(|event| Some(event.devnode()?.to_path_buf()))
}

/// Whether the device the pad was checked as is still there: a pad that
/// reconnected has another.
pub(crate) fn is_there(pad: &Pad) -> bool {
    pad.parent.exists()
}

/// Whether a node belongs to `group`, which only the rule that hides a
/// pad gives it.
fn hidden_by(node: &Path, group: u32) -> bool {
    std::os::unix::fs::MetadataExt::gid(&match std::fs::metadata(node) {
        Ok(metadata) => metadata,
        Err(_) => return false,
    }) == group
}

/// Every pad the rule has hidden from the seat, as `group`'s: each event
/// node that belongs to it, classified, by its pad's device.
pub(crate) fn hidden(group: u32) -> Vec<Pad> {
    let Ok(mut events) = udev::Enumerator::new() else {
        return Vec::new();
    };
    if events.match_subsystem("input").is_err() || events.match_is_initialized().is_err() {
        return Vec::new();
    }
    let Ok(devices) = events.scan_devices() else {
        return Vec::new();
    };
    let mut found: Vec<Pad> = Vec::new();
    for event in devices {
        let Some(node) = event.devnode() else {
            continue;
        };
        if !hidden_by(node, group) {
            continue;
        }
        if let Ok(pad) = classify(&event, false)
            && !found.iter().any(|other| other.parent == pad.parent)
        {
            found.push(pad);
        }
    }
    found
}

/// The pad a hidden one came back as after reconnecting: hidden by the
/// rule, with the same numbers, seat and kind of device, named by one of
/// `was`'s markers, and not one of `taken`. By the most telling marker
/// shared, when there are several.
pub(crate) fn returned(was: &Pad, group: u32, taken: &[&Path]) -> Option<Pad> {
    let rank = |marker: &str| {
        ["addr-", "serial-", "uniq-", "port-", "hid-"]
            .iter()
            .position(|prefix| marker.starts_with(prefix))
            .unwrap_or(usize::MAX)
    };
    hidden(group)
        .into_iter()
        .filter(|pad| pad.identity == was.identity && !taken.contains(&pad.parent.as_path()))
        .filter_map(|pad| {
            let best = pad
                .markers
                .intersection(&was.markers)
                .map(|marker| rank(marker))
                .min()?;
            Some((best, pad))
        })
        .min_by_key(|(best, _)| *best)
        .map(|(_, pad)| pad)
}

/// The HID drivers whose pads' raw nodes the service opens for a caller.
/// A raw node takes whatever the pad's firmware does, so this names the
/// pads a virtual pad is made to pass as, and nothing else.
const RAW_DRIVERS: [&str; 1] = ["playstation"];

/// The raw HID node of the HID device at `parent`, if its driver is one
/// whose raw nodes the service opens.
pub(crate) fn raw_node(parent: &Path) -> Result<PathBuf, Refusal> {
    let device = udev::Device::from_syspath(parent).map_err(|_| Refusal::Absent)?;
    let offered = subsystem(&device) == Some("hid")
        && device
            .driver()
            .and_then(|driver| driver.to_str())
            .is_some_and(|driver| RAW_DRIVERS.contains(&driver));
    if !offered {
        return Err(Refusal::Unsupported);
    }
    let mut children = udev::Enumerator::new().map_err(|_| Refusal::Failed)?;
    children
        .match_parent(&device)
        .map_err(|_| Refusal::Failed)?;
    children
        .match_subsystem("hidraw")
        .map_err(|_| Refusal::Failed)?;
    children
        .scan_devices()
        .map_err(|_| Refusal::Failed)?
        .find_map(|child| Some(child.devnode()?.to_path_buf()))
        .ok_or(Refusal::Absent)
}

/// Each event node under `parent` that the kernel has made, whether or
/// not udev has finished with it yet.
pub(crate) fn event_devices_under(parent: &Path) -> BTreeSet<PathBuf> {
    let Ok(device) = udev::Device::from_syspath(parent) else {
        return BTreeSet::new();
    };
    let Ok(mut found) = udev::Enumerator::new() else {
        return BTreeSet::new();
    };
    if found.match_parent(&device).is_err() || found.match_subsystem("input").is_err() {
        return BTreeSet::new();
    }
    found
        .scan_devices()
        .map(|devices| {
            devices
                .filter(is_event)
                .map(|event| event.syspath().to_path_buf())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the device at `parent` has a driver bound.
pub(crate) fn has_driver(parent: &Path) -> bool {
    udev::Device::from_syspath(parent).is_ok_and(|device| device.driver().is_some())
}
