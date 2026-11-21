//! Deciding what a descriptor is, and finding that pad again.
//!
//! Everything read from a device here is treated as hostile: a seat user
//! can create devices with any name and address. Only numbers the service
//! parses itself, and strings that pass the narrow checks below, are ever
//! used to build a rule or a path.

use std::os::fd::BorrowedFd;
use std::path::PathBuf;

use ctrlassist_hider::Refusal;
use rustix::fs::{FileType, fstat};

/// How to cut off whatever has the pad open: the driver to unbind and
/// bind again, and the device it is bound to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Rebind {
    /// `hid` or `usb`.
    pub(crate) bus: &'static str,
    pub(crate) device: String,
    pub(crate) driver: String,
}

/// What tells this pad from others.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Identity {
    pub(crate) bus: u16,
    pub(crate) vendor: u16,
    pub(crate) product: u16,
    /// The pad's own address, if it reports a well-formed one.
    pub(crate) uniq: Option<String>,
    /// Its USB serial number, if it has a well-formed one.
    pub(crate) serial: Option<String>,
    /// Whether it has a HID device, and so hidraw nodes.
    pub(crate) hid: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Pad {
    pub(crate) identity: Identity,
    pub(crate) rebind: Option<Rebind>,
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

/// A HID device's kernel name, exactly as the kernel forms it: bus,
/// vendor, product and instance as four uppercase hex digits each, such
/// as `0003:054C:0CE6.0017`.
pub(crate) fn is_hid_name(text: &str) -> bool {
    text.len() == 19
        && text.bytes().enumerate().all(|(index, byte)| match index {
            4 | 9 => byte == b':',
            14 => byte == b'.',
            _ => matches!(byte, b'0'..=b'9' | b'A'..=b'F'),
        })
}

/// A USB interface's kernel name: a port path, then configuration and
/// interface numbers, such as `1-3:1.0` or `3-1.4.2:1.0`.
pub(crate) fn is_usb_interface(text: &str) -> bool {
    let numbers = |text: &str, separators: &[u8]| {
        !text.is_empty()
            && text.len() <= 32
            && text
                .split(|character: char| separators.contains(&(character as u8)))
                .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
    };
    text.split_once(':')
        .is_some_and(|(port, interface)| numbers(port, b"-.") && numbers(interface, b"."))
}

pub(crate) fn is_driver(text: &str) -> bool {
    is_word(
        text,
        |byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_'),
    )
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

/// What an event device is, if it is a pad a seat user could open.
fn classify(event: &udev::Device) -> Option<Pad> {
    let is_event = subsystem(event) == Some("input")
        && event
            .sysname()
            .to_str()
            .is_some_and(|name| name.starts_with("event"));
    // A joystick, and nothing a keystroke or a pointer could come from:
    // one HID interface can carry a keyboard beside a joystick.
    let is_only_a_pad = flag(event, "ID_INPUT_JOYSTICK")
        && !flag(event, "ID_INPUT_KEYBOARD")
        && !flag(event, "ID_INPUT_KEY")
        && !flag(event, "ID_INPUT_MOUSE")
        && !flag(event, "ID_INPUT_TOUCHPAD")
        && !flag(event, "ID_INPUT_TABLET");
    // Tagged for seat access: the caller could already open it.
    let seat_may_open =
        property(event, "CURRENT_TAGS").is_some_and(|tags| tags.contains(":uaccess:"));
    if !(is_event && is_only_a_pad && seat_may_open) {
        return None;
    }

    let input = event.parent()?;
    let (hid, rebind) = driver_of(&input);
    Some(Pad {
        identity: Identity {
            bus: hex(&attribute(&input, "id/bustype")?)?,
            vendor: hex(&attribute(&input, "id/vendor")?)?,
            product: hex(&attribute(&input, "id/product")?)?,
            uniq: attribute(&input, "uniq").filter(|uniq| is_address(uniq)),
            serial: property(event, "ID_SERIAL_SHORT")
                .filter(|serial| is_serial(serial))
                .map(str::to_string),
            hid,
        },
        rebind,
    })
}

/// Whether an input device sits on a HID device, and the driver to
/// re-bind to cut its readers off: the HID driver if it has one, else the
/// driver of its USB interface, else none.
fn driver_of(input: &udev::Device) -> (bool, Option<Rebind>) {
    let ancestors = || std::iter::successors(Some(input.clone()), udev::Device::parent);
    let hid = ancestors().find(|device| subsystem(device) == Some("hid"));
    let interface = ancestors().find(|device| {
        subsystem(device) == Some("usb")
            && device.devtype().and_then(|kind| kind.to_str()) == Some("usb_interface")
    });
    let bound = |bus: &'static str, device: &udev::Device, name_ok: fn(&str) -> bool| {
        let name = device.sysname().to_str()?.to_string();
        let driver = device.driver()?.to_str()?.to_string();
        (name_ok(&name) && is_driver(&driver)).then_some(Rebind {
            bus,
            device: name,
            driver,
        })
    };
    let rebind = match (&hid, &interface) {
        (Some(hid), _) => bound("hid", hid, is_hid_name),
        (None, Some(interface)) => bound("usb", interface, is_usb_interface),
        (None, None) => None,
    };
    (hid.is_some(), rebind)
}

/// What the descriptor is open on.
pub(crate) fn identify(fd: BorrowedFd<'_>) -> Result<Pad, Refusal> {
    let stat = fstat(fd).map_err(|_| Refusal::NotAPad)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::CharacterDevice {
        return Err(Refusal::NotAPad);
    }
    let device = udev::Device::from_devnum(udev::DeviceType::Character, stat.st_rdev)
        .map_err(|_| Refusal::NotAPad)?;
    classify(&device).ok_or(Refusal::NotAPad)
}

/// The event device of the connected pad with this identity.
///
/// The same reading of a device that `identify` made, so a pad is found
/// again by exactly what it was recognized by. The seat tag is not
/// required here: a hidden pad no longer carries it.
fn find(identity: &Identity) -> Option<udev::Device> {
    let mut inputs = udev::Enumerator::new().ok()?;
    inputs.match_subsystem("input").ok()?;
    inputs.scan_devices().ok()?.find(|event| {
        let Some(input) = event.parent() else {
            return false;
        };
        event.devnode().is_some()
            && event
                .sysname()
                .to_str()
                .is_some_and(|name| name.starts_with("event"))
            && flag(event, "ID_INPUT_JOYSTICK")
            && attribute(&input, "id/vendor").and_then(|text| hex(&text)) == Some(identity.vendor)
            && attribute(&input, "id/product").and_then(|text| hex(&text)) == Some(identity.product)
            && attribute(&input, "uniq").filter(|uniq| is_address(uniq)) == identity.uniq
            && property(event, "ID_SERIAL_SHORT").filter(|serial| is_serial(serial))
                == identity.serial.as_deref()
    })
}

/// The driver to re-bind for the connected pad with this identity, as it
/// is now: a pad that has reconnected has a new kernel name.
pub(crate) fn rebind(identity: &Identity) -> Option<Rebind> {
    driver_of(&find(identity)?.parent()?).1
}

/// Every device node of the connected pad with this identity: its event
/// node first, then its js and hidraw nodes.
pub(crate) fn nodes(identity: &Identity) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Some(event) = find(identity) else {
        return found;
    };
    let (Some(node), Some(input)) = (event.devnode(), event.parent()) else {
        return found;
    };
    found.push(node.to_path_buf());
    let parent = std::iter::successors(Some(input.clone()), udev::Device::parent)
        .find(|device| subsystem(device) == Some("hid"))
        .unwrap_or(input);
    let Ok(mut children) = udev::Enumerator::new() else {
        return found;
    };
    if children.match_parent(&parent).is_err() {
        return found;
    }
    if let Ok(children) = children.scan_devices() {
        for child in children {
            let Some(child_node) = child.devnode() else {
                continue;
            };
            let is_sibling = matches!(subsystem(&child), Some("input" | "hidraw"));
            if is_sibling && child_node != node {
                found.push(child_node.to_path_buf());
            }
        }
    }
    found
}
