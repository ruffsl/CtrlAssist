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

/// One enumeration of a device: its place in sysfs, and what tells this
/// enumeration from a later one at the same place.
///
/// A HID device's place names one enumeration already: its kernel name
/// ends in an instance number the kernel never hands out twice. A USB
/// interface's place is its port's, and whatever is next plugged in there
/// has the same one. So a device on USB is told by when udev first saw
/// the USB device above it (`USEC_INITIALIZED`), a time in microseconds
/// that udev stamps a device with once, as it first finishes with it, and
/// keeps through every later event of that device, a re-bind of its
/// interfaces' drivers among them; a device enumerated afresh is stamped
/// afresh. The USB device's own number (`devnum`) would do as well but
/// for wrapping after 127 enumerations on a bus, and the interface itself
/// has no record in udev's database to keep a stamp in.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Device {
    pub(crate) path: PathBuf,
    /// When udev first saw the USB device at or above it, if it is on
    /// USB.
    pub(crate) enumeration: Option<u64>,
}

impl Device {
    /// The enumeration of the device at `path` there now.
    pub(crate) fn at(path: &Path) -> Option<Self> {
        let device = udev::Device::from_syspath(path).ok()?;
        Some(Self {
            path: path.to_path_buf(),
            enumeration: enumeration(&device)?,
        })
    }

    /// Whether this enumeration of the device is still there: not
    /// unplugged, and not replaced by another plugged into the same port.
    pub(crate) fn is_there(&self) -> bool {
        Self::at(&self.path).is_some_and(|now| now == *self)
    }
}

/// When udev first saw the USB device at or above `device`: `Some(None)`
/// for a device not on USB, and `None` where udev hasn't stamped the USB
/// device yet, which isn't then one this service has checked.
fn enumeration(device: &udev::Device) -> Option<Option<u64>> {
    let usb = std::iter::successors(Some(device.clone()), udev::Device::parent).find(|device| {
        subsystem(device) == Some("usb")
            && device.devtype().and_then(|kind| kind.to_str()) == Some("usb_device")
    });
    let Some(usb) = usb else {
        return Some(None);
    };
    let stamp = property(&usb, "USEC_INITIALIZED")?.parse::<u64>().ok()?;
    Some(Some(stamp))
}

/// A pad the service has checked: the device whose driver is re-bound to
/// cut its readers off, and what names it to the rule.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct Pad {
    /// The pad's HID device, or with none its USB interface: the device
    /// that was checked, which keeps its place in sysfs through a re-bind
    /// of its driver, as the enumeration of it that was checked.
    pub(crate) parent: Device,
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

/// A USB interface's kernel name, exactly as the kernel forms it: the
/// bus, the path of ports to the device, then the configuration and the
/// interface, such as `1-2:1.0` or `3-1.4.2:1.3`. It names the port a
/// pad is plugged into, which a device made through uhid has none of.
pub(crate) fn is_usb_interface(text: &str) -> bool {
    let number = |part: &str| {
        (1..=3).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit())
    };
    let Some((device, interface)) = text.split_once(':') else {
        return false;
    };
    let Some((bus, ports)) = device.split_once('-') else {
        return false;
    };
    let Some((configuration, interface)) = interface.split_once('.') else {
        return false;
    };
    text.len() <= 64
        && number(bus)
        && ports.split('.').all(number)
        && number(configuration)
        && number(interface)
}

/// What names a pad to the rule, as read from its devices, before any of
/// it is checked.
#[derive(Clone, Default, Debug)]
pub(crate) struct Names {
    /// The input device's `uniq`.
    pub(crate) uniq: Option<String>,
    /// udev's `ID_SERIAL_SHORT` for the event node.
    pub(crate) serial: Option<String>,
    pub(crate) vendor: Option<u16>,
    pub(crate) product: Option<u16>,
    /// The HID device's kernel name.
    pub(crate) hid: Option<String>,
    /// The HID device's own `HID_UNIQ` and `HID_ID`.
    pub(crate) hid_uniq: Option<String>,
    pub(crate) hid_id: Option<String>,
    /// The kernel name of the USB interface the pad hangs from.
    pub(crate) interface: Option<String>,
}

/// The marker files that hide a pad, as the rule names them, from what
/// its devices say: by address, else serial number; by HID device; by
/// port where neither; and its raw node as [`raw_markers`] says. Only
/// what passes the checks above reaches a name, and no name has a `/` in
/// it.
pub(crate) fn markers(names: &Names) -> BTreeSet<String> {
    let mut markers = raw_markers(names);
    let address = names.uniq.as_deref().filter(|uniq| is_address(uniq));
    let serial = names.serial.as_deref().filter(|serial| is_serial(serial));
    if let Some(address) = address {
        markers.insert(format!("addr-{address}"));
    } else if let (Some(serial), Some(vendor), Some(product)) =
        (serial, names.vendor, names.product)
    {
        markers.insert(format!("serial-{vendor:04x}-{product:04x}-{serial}"));
    }
    if address.is_none()
        && serial.is_none()
        && let Some(interface) = interface(names)
    {
        markers.insert(format!("port-{interface}"));
    }
    markers
}

/// The USB interface `names` hang from, if its name is the kernel's.
fn interface(names: &Names) -> Option<&str> {
    names
        .interface
        .as_deref()
        .filter(|interface| is_usb_interface(interface))
}

/// The marker files that hide a HID device's raw node, as the rule names
/// them: by the HID device itself; by its own unique string; and, on USB,
/// by its port and its model together, so that only a HID device of the
/// same bus, vendor and product plugged into that port is taken for it,
/// never a keyboard, a headset or a security key. None without a HID
/// device.
pub(crate) fn raw_markers(names: &Names) -> BTreeSet<String> {
    let mut markers = BTreeSet::new();
    let Some(hid) = names.hid.as_deref().filter(|name| is_hid_name(name)) else {
        return markers;
    };
    markers.insert(format!("hid-{hid}"));
    let uniq = names.hid_uniq.as_deref().filter(|uniq| is_uniq(uniq));
    let id = names.hid_id.as_deref().filter(|id| is_hid_id(id));
    if let (Some(uniq), Some(id)) = (uniq, id) {
        markers.insert(format!("uniq-{id}-{uniq}"));
    }
    // Whatever the HID device says of its address: a driver can set it
    // after the raw node is made, as a DualSense's does on USB, so the
    // node of one that comes back may be made before it.
    if let (Some(interface), Some(id)) = (interface(names), id) {
        markers.insert(format!("raw-{interface}-{id}"));
    }
    markers
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

/// A keyboard or a pointer, as udev classifies an input. A pad's own
/// touchpad and motion sensors are neither: they go with the pad.
fn is_keyboard_or_pointer(input: &udev::Device) -> bool {
    [
        "ID_INPUT_KEYBOARD",
        "ID_INPUT_MOUSE",
        "ID_INPUT_TABLET",
        "ID_INPUT_POINTINGSTICK",
    ]
    .iter()
    .any(|name| flag(input, name))
}

/// The devices of `subsystem` under `parent`, as the kernel has them now,
/// whether or not udev has finished with them.
fn children(parent: &Path, subsystem: &str) -> Option<Vec<udev::Device>> {
    let parent = udev::Device::from_syspath(parent).ok()?;
    let mut found = udev::Enumerator::new().ok()?;
    found.match_parent(&parent).ok()?;
    found.match_subsystem(subsystem).ok()?;
    Some(found.scan_devices().ok()?.collect())
}

/// Whether a keyboard or a pointer is among the inputs under `parent`:
/// re-binding the pad's driver would cut it off too, and the rule can't
/// see one from the pad's own node. When that can't be read, it is taken
/// that one is.
fn shares_with_keyboard_or_pointer(parent: &Path) -> bool {
    children(parent, "input").is_none_or(|inputs| inputs.iter().any(is_keyboard_or_pointer))
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

/// What names this event node's pad to the rule.
fn names_of(event: &udev::Device, input: &udev::Device, hid: Option<&udev::Device>) -> Names {
    let interface =
        std::iter::successors(Some(input.clone()), udev::Device::parent).find(|device| {
            subsystem(device) == Some("usb")
                && device.devtype().and_then(|kind| kind.to_str()) == Some("usb_interface")
        });
    Names {
        uniq: attribute(input, "uniq"),
        serial: property(event, "ID_SERIAL_SHORT").map(str::to_string),
        vendor: attribute(input, "id/vendor").and_then(|text| hex(&text)),
        product: attribute(input, "id/product").and_then(|text| hex(&text)),
        hid: hid
            .and_then(|hid| hid.sysname().to_str())
            .map(str::to_string),
        hid_uniq: hid
            .and_then(|hid| property(hid, "HID_UNIQ"))
            .map(str::to_string),
        hid_id: hid
            .and_then(|hid| property(hid, "HID_ID"))
            .map(str::to_string),
        interface: interface.and_then(|interface| interface.sysname().to_str().map(str::to_string)),
    }
}

/// Each joystick event node under `parent`, in the order the kernel made
/// them. Only those udev has finished with have the properties that say
/// what they are.
fn joysticks_under(parent: &Path) -> Vec<udev::Device> {
    let mut joysticks: Vec<udev::Device> = children(parent, "input")
        .unwrap_or_default()
        .into_iter()
        .filter(is_only_a_pad)
        .collect();
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
    let markers = markers(&names_of(event, &input, hid.as_ref()));
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
        parent: Device {
            path: parent.syspath().to_path_buf(),
            enumeration: enumeration(&parent).ok_or(Refusal::Failed)?,
        },
        ordinal,
        identity,
        markers,
    })
}

/// The pad the descriptor is open on, if it was opened for reading, a
/// seat user could open it, and nothing a keystroke or a pointer could
/// come from shares its device. Which seat it is on is the caller's to
/// check.
pub(crate) fn identify(fd: BorrowedFd<'_>) -> Result<Pad, Refusal> {
    let devnum = crate::descriptor::event_node(fd)?;
    let event = udev::Device::from_devnum(udev::DeviceType::Character, devnum)
        .map_err(|_| Refusal::NotAPad)?;
    let pad = classify(&event, true)?;
    if shares_with_keyboard_or_pointer(&pad.parent.path) {
        tracing::info!(
            device = %pad.parent.path.display(),
            "refused a pad whose device has a keyboard or a pointer among its inputs: re-binding its driver would cut that off too"
        );
        return Err(Refusal::Unsupported);
    }
    Ok(pad)
}

/// The event node of the pad under `parent` that is `ordinal`th, now.
pub(crate) fn event_node(parent: &Path, ordinal: usize) -> Option<PathBuf> {
    joysticks_under(parent)
        .get(ordinal)
        .and_then(|event| Some(event.devnode()?.to_path_buf()))
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
            && !found
                .iter()
                .any(|other| other.parent.path == pad.parent.path)
        {
            found.push(pad);
        }
    }
    found
}

/// What the rule sets on every node it hides, and on no other: udev keeps
/// a node's properties from the last event it took, so this says the
/// node was hidden then. A raw node's group can't say it, the rule giving
/// a PlayStation pad's raw node the group whether it is hidden or not.
const HIDDEN: &str = "CTRLASSIST_HIDDEN";

/// The HID device of each raw node the rule hid when udev last took an
/// event of it, with the markers that would hide that node now. A raw
/// node can be hidden with its pad's event node not, as a pad of the
/// model of one hidden, plugged into its port while it was away, is.
pub(crate) fn hidden_raw() -> Vec<(Device, BTreeSet<String>)> {
    let Ok(mut nodes) = udev::Enumerator::new() else {
        return Vec::new();
    };
    if nodes.match_subsystem("hidraw").is_err() || nodes.match_is_initialized().is_err() {
        return Vec::new();
    }
    let Ok(nodes) = nodes.scan_devices() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for node in nodes {
        if property(&node, HIDDEN) != Some("1") {
            continue;
        }
        let ancestors = || std::iter::successors(Some(node.clone()), udev::Device::parent);
        let Some(hid) = ancestors().find(|device| subsystem(device) == Some("hid")) else {
            continue;
        };
        let interface = ancestors().find(|device| {
            subsystem(device) == Some("usb")
                && device.devtype().and_then(|kind| kind.to_str()) == Some("usb_interface")
        });
        let names = Names {
            hid: hid.sysname().to_str().map(str::to_string),
            hid_uniq: property(&hid, "HID_UNIQ").map(str::to_string),
            hid_id: property(&hid, "HID_ID").map(str::to_string),
            interface: interface
                .and_then(|interface| interface.sysname().to_str().map(str::to_string)),
            ..Names::default()
        };
        let Some(enumeration) = enumeration(&hid) else {
            continue;
        };
        let device = Device {
            path: hid.syspath().to_path_buf(),
            enumeration,
        };
        found.push((device, raw_markers(&names)));
    }
    found
}

/// How telling a marker is of the pad it names, most first: an address
/// or a serial number names one pad wherever it is plugged; a port names
/// whatever is plugged there.
fn rank(marker: &str) -> usize {
    ["addr-", "serial-", "uniq-", "port-", "raw-", "hid-"]
        .iter()
        .position(|prefix| marker.starts_with(prefix))
        .unwrap_or(usize::MAX)
}

/// Of `candidates`, the pad `was` came back as: with the same numbers,
/// seat and kind of device, named by one of `was`'s markers, and not one
/// of `taken`. By the most telling marker shared, when there are several.
fn best(was: &Pad, candidates: Vec<Pad>, taken: &[&Path]) -> Option<Pad> {
    candidates
        .into_iter()
        .filter(|pad| pad.identity == was.identity && !taken.contains(&pad.parent.path.as_path()))
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

/// The pad a hidden one came back as after reconnecting: hidden by the
/// rule, and [`best`] among those. One that now shares its device with a
/// keyboard or a pointer isn't taken, as it wouldn't be hidden.
pub(crate) fn returned(was: &Pad, group: u32, taken: &[&Path]) -> Option<Pad> {
    let candidates = hidden(group)
        .into_iter()
        .filter(|pad| !shares_with_keyboard_or_pointer(&pad.parent.path))
        .collect();
    best(was, candidates, taken)
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
    children(parent, "hidraw")
        .ok_or(Refusal::Failed)?
        .into_iter()
        .find_map(|child| Some(child.devnode()?.to_path_buf()))
        .ok_or(Refusal::Absent)
}

/// Each input device under `parent` the kernel has now, by its place in
/// sysfs. The kernel numbers input devices afresh each time, so one made
/// by a re-bind is never at a place one before it was.
pub(crate) fn inputs_under(parent: &Path) -> Vec<PathBuf> {
    children(parent, "input")
        .unwrap_or_default()
        .into_iter()
        .filter(|input| {
            input
                .sysname()
                .to_str()
                .is_some_and(|name| name.starts_with("input"))
        })
        .map(|input| input.syspath().to_path_buf())
        .collect()
}

/// Every node under `parent` the kernel has made, whether or not udev has
/// finished with it: each input's event, js and mouse nodes, and the raw
/// HID node.
pub(crate) fn nodes_under(parent: &Path) -> Vec<udev::Device> {
    ["input", "hidraw"]
        .iter()
        .flat_map(|subsystem| children(parent, subsystem).unwrap_or_default())
        .filter(|device| device.devnode().is_some())
        .collect()
}

/// Whether the device at `parent` has a driver bound.
pub(crate) fn has_driver(parent: &Path) -> bool {
    udev::Device::from_syspath(parent).is_ok_and(|device| device.driver().is_some())
}

/// Whether udev has finished with a node since `since` (`CLOCK_MONOTONIC`,
/// in microseconds), the programs its rules run included. A node udev
/// last finished with before then is one it hasn't seen since it was
/// made again.
pub(crate) fn finished_since(node: &udev::Device, since: u64) -> bool {
    node.is_initialized()
        && property(node, "ID_PROCESSING") != Some("1")
        && property(node, "USEC_INITIALIZED")
            .and_then(|usec| usec.parse::<u64>().ok())
            .is_some_and(|usec| usec >= since)
}

/// A node's owner, group and permission bits.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Ownership {
    pub(crate) uid: u32,
    pub(crate) gid: u32,
    pub(crate) mode: u32,
}

/// What is wrong with a node for one the rule has hidden, if anything:
/// it must be root's, `group`'s, mode 0660, and not tagged for seat
/// access. `tags` is udev's `CURRENT_TAGS` for it.
pub(crate) fn unhidden(ownership: Ownership, tags: Option<&str>, group: u32) -> Option<String> {
    let seat_may_open = tags.is_some_and(|tags| tags.split(':').any(|tag| tag == "uaccess"));
    if ownership.uid != 0 || ownership.gid != group || ownership.mode & 0o7777 != 0o660 {
        Some(format!(
            "owned by {}:{} with mode {:04o}, where a hidden node is root's and the group {group}'s with mode 0660",
            ownership.uid,
            ownership.gid,
            ownership.mode & 0o7777
        ))
    } else if seat_may_open {
        Some("tagged for seat access".to_string())
    } else {
        None
    }
}

/// What is wrong with a device's node for one the rule has hidden, if
/// anything, its ACL included.
pub(crate) fn unhidden_node(node: &udev::Device, group: u32) -> Option<String> {
    let path = node.devnode()?;
    match rustix::fs::stat(path) {
        Ok(stat) => unhidden(
            Ownership {
                uid: stat.st_uid,
                gid: stat.st_gid,
                mode: stat.st_mode,
            },
            property(node, "CURRENT_TAGS"),
            group,
        )
        .or_else(|| acl_wrong(|buffer| rustix::fs::getxattr(path, ACL, buffer))),
        Err(error) => Some(format!("not to be looked at: {error}")),
    }
}

/// The attribute a file's POSIX ACL is kept in.
pub(crate) const ACL: &str = "system.posix_acl_access";

/// Whether a POSIX ACL, as the kernel gives it from [`ACL`], grants
/// anything beyond the file's mode: an entry for a named user or group,
/// as logind's seat access adds, or the mask that comes with one. The
/// owner's, the group's and everyone else's entries are the mode itself.
/// What isn't shaped as an ACL is taken to.
fn acl_grants_beyond_mode(acl: &[u8]) -> bool {
    /// `ACL_USER_OBJ`, `ACL_GROUP_OBJ` and `ACL_OTHER`.
    const MODE: [u16; 3] = [0x01, 0x04, 0x20];
    let Some((version, entries)) = acl.split_first_chunk::<4>() else {
        return true;
    };
    let (entries, rest) = entries.as_chunks::<8>();
    if u32::from_le_bytes(*version) != 2 || !rest.is_empty() {
        return true;
    }
    entries
        .iter()
        .any(|[tag, high, ..]| !MODE.contains(&u16::from_le_bytes([*tag, *high])))
}

/// What a node's ACL grants beyond its mode, if anything. `read` reads
/// the node's [`ACL`] attribute into a buffer, as `fgetxattr` does.
pub(crate) fn acl_wrong(
    read: impl FnOnce(&mut [u8]) -> rustix::io::Result<usize>,
) -> Option<String> {
    // Room for the header and 31 entries; a hidden node has none.
    let mut buffer = [0u8; 252];
    match read(&mut buffer) {
        Ok(length) => buffer
            .get(..length)
            .is_none_or(acl_grants_beyond_mode)
            .then(|| "given access beyond its mode by an ACL".to_string()),
        // No ACL, or a file system that keeps none.
        Err(rustix::io::Errno::NODATA | rustix::io::Errno::OPNOTSUPP) => None,
        Err(rustix::io::Errno::RANGE) => {
            Some("given access beyond its mode by an ACL too long to read".to_string())
        }
        Err(error) => Some(format!("its ACL couldn't be read: {error}")),
    }
}

/// The device a node's number names now, as udev knows it.
pub(crate) fn device_of(kind: udev::DeviceType, devnum: libc::dev_t) -> Option<udev::Device> {
    udev::Device::from_devnum(kind, devnum).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(parent: &str, seat: &str, markers: &[&str]) -> Pad {
        Pad {
            parent: Device {
                path: PathBuf::from(parent),
                enumeration: None,
            },
            ordinal: 0,
            identity: Identity {
                bus: 3,
                vendor: 0x054c,
                product: 0x0ce6,
                seat: seat.to_string(),
                virtual_device: false,
            },
            markers: markers.iter().map(ToString::to_string).collect(),
        }
    }

    fn names() -> Names {
        Names {
            uniq: None,
            serial: None,
            vendor: Some(0x045e),
            product: Some(0x028e),
            hid: None,
            hid_uniq: None,
            hid_id: None,
            interface: None,
        }
    }

    fn set(markers: &[&str]) -> BTreeSet<String> {
        markers.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn a_pad_with_an_address_is_named_by_it_and_its_raw_node_by_its_port_too() {
        // A DualSense on USB, once its driver has read its address.
        let dualsense = Names {
            uniq: Some("0a:1b:2c:3d:4e:14".into()),
            hid: Some("0003:054C:0CE6.0017".into()),
            hid_uniq: Some("0a:1b:2c:3d:4e:14".into()),
            hid_id: Some("0003:0000054C:00000CE6".into()),
            interface: Some("1-2:1.3".into()),
            ..names()
        };
        assert_eq!(
            markers(&dualsense),
            set(&[
                "addr-0a:1b:2c:3d:4e:14",
                "hid-0003:054C:0CE6.0017",
                "uniq-0003:0000054C:00000CE6-0a:1b:2c:3d:4e:14",
                "raw-1-2:1.3-0003:0000054C:00000CE6",
            ])
        );
        // The same, its raw node seen before the address was set.
        let early = Names {
            hid_uniq: None,
            ..dualsense.clone()
        };
        assert_eq!(
            markers(&early),
            set(&[
                "addr-0a:1b:2c:3d:4e:14",
                "hid-0003:054C:0CE6.0017",
                "raw-1-2:1.3-0003:0000054C:00000CE6",
            ])
        );
        // On Bluetooth: no port.
        let bluetooth = Names {
            interface: None,
            ..dualsense
        };
        assert!(
            !markers(&bluetooth)
                .iter()
                .any(|marker| marker.starts_with("raw-"))
        );
    }

    #[test]
    fn a_pad_with_a_serial_and_no_address_is_named_by_its_serial_not_its_port() {
        let xpad = Names {
            serial: Some("XPADFIXTURE1".into()),
            interface: Some("1-1:1.0".into()),
            ..names()
        };
        assert_eq!(markers(&xpad), set(&["serial-045e-028e-XPADFIXTURE1"]));
    }

    #[test]
    fn a_pad_with_neither_is_named_by_its_port() {
        let xpad = Names {
            interface: Some("2-1:1.0".into()),
            ..names()
        };
        assert_eq!(markers(&xpad), set(&["port-2-1:1.0"]));
        // A HID pad on USB with neither: its nodes by port and HID device,
        // its raw node by port and model.
        let hid = Names {
            hid: Some("0003:1209:5EEE.0002".into()),
            hid_id: Some("0003:00001209:00005EEE".into()),
            interface: Some("3-1.4.2:1.0".into()),
            ..names()
        };
        assert_eq!(
            markers(&hid),
            set(&[
                "port-3-1.4.2:1.0",
                "hid-0003:1209:5EEE.0002",
                "raw-3-1.4.2:1.0-0003:00001209:00005EEE",
            ])
        );
        // A uhid pad has no USB interface, and nothing names it by port.
        let uhid = Names {
            hid: Some("0003:1209:5EEE.0002".into()),
            ..names()
        };
        assert_eq!(markers(&uhid), set(&["hid-0003:1209:5EEE.0002"]));
    }

    /// A keyboard, or any other HID device, plugged into a hidden pad's
    /// port afterwards: no raw marker of the pad's names its raw node, and
    /// with no model to go by none names the port alone.
    #[test]
    fn a_raw_node_is_named_by_its_port_only_with_its_model() {
        let pad = Names {
            hid: Some("0003:1209:5EF0.0004".into()),
            hid_id: Some("0003:00001209:00005EF0".into()),
            interface: Some("2-1:1.0".into()),
            ..names()
        };
        let keyboard = Names {
            hid: Some("0003:1209:5EFA.0005".into()),
            hid_id: Some("0003:00001209:00005EFA".into()),
            ..pad.clone()
        };
        assert!(raw_markers(&pad).is_disjoint(&raw_markers(&keyboard)));
        // The same model in the same port is taken for it, as it is meant
        // to be: it may be the pad itself, back.
        let again = Names {
            hid: Some("0003:1209:5EF0.0006".into()),
            ..pad.clone()
        };
        assert!(!raw_markers(&pad).is_disjoint(&raw_markers(&again)));
        let unknown = Names {
            hid_id: None,
            ..pad
        };
        assert_eq!(raw_markers(&unknown), set(&["hid-0003:1209:5EF0.0004"]));
    }

    #[test]
    fn nothing_a_device_says_unchecked_reaches_a_marker() {
        let hostile = Names {
            uniq: Some("../../../etc/passwd".into()),
            serial: Some("a/b".into()),
            vendor: Some(1),
            product: Some(2),
            hid: Some("0003:054C:0CE6.0017/..".into()),
            hid_uniq: Some("x/../y".into()),
            hid_id: Some("0003:0000054C:00000CE6/x".into()),
            interface: Some("usb-0000:00:14.0-2/../../../tmp".into()),
        };
        assert_eq!(markers(&hostile), BTreeSet::new());
        let hid = Names {
            hid: Some("0003:054C:0CE6.0017".into()),
            hid_uniq: Some("x/../y".into()),
            hid_id: Some("0003:0000054C:00000CE6".into()),
            ..hostile
        };
        assert_eq!(markers(&hid), set(&["hid-0003:054C:0CE6.0017"]));
        for marker in markers(&hid) {
            assert!(!marker.contains('/'), "{marker}");
        }
    }

    #[test]
    fn only_kernel_shaped_interface_names_name_a_port() {
        for name in ["1-2:1.0", "3-1.4.2:1.3", "10-0:1.0", "1-1.2.3.4.5.6.7:1.0"] {
            assert!(is_usb_interface(name), "{name}");
        }
        for hostile in [
            "",
            "1-2",
            "1-2:1",
            "1-2:1.0/x",
            "1-2:1.0/../..",
            "usb-0000:00:14.0-2/input3",
            "1-..:1.0",
            "1--2:1.0",
            "1-2.:1.0",
            "a-2:1.0",
            "1-2:1.0 ",
            "1-2:1.0.0",
            "1234-2:1.0",
        ] {
            assert!(!is_usb_interface(hostile), "{hostile:?}");
        }
    }

    #[test]
    fn a_pad_that_came_back_is_found_by_its_most_telling_marker() {
        let was = pad(
            "/sys/devices/old",
            "seat0",
            &[
                "addr-0a:1b:2c:3d:4e:14",
                "port-1-2:1.0",
                "hid-0003:054C:0CE6.0017",
            ],
        );
        // One in its port, one with its address, one of another model.
        let in_port = pad("/sys/devices/a", "seat0", &["port-1-2:1.0"]);
        let addressed = pad("/sys/devices/b", "seat0", &["addr-0a:1b:2c:3d:4e:14"]);
        let mut other = pad("/sys/devices/c", "seat0", &["addr-0a:1b:2c:3d:4e:14"]);
        other.identity.product = 0x09cc;
        let found = best(
            &was,
            vec![in_port.clone(), other.clone(), addressed.clone()],
            &[],
        );
        assert_eq!(
            found.map(|pad| pad.parent.path),
            Some(addressed.parent.path.clone())
        );
        // The one with its address is held by another hidden pad: the one
        // in its port is taken.
        let taken = [addressed.parent.path.as_path()];
        let found = best(&was, vec![in_port.clone(), addressed.clone()], &taken);
        assert_eq!(found.map(|pad| pad.parent.path), Some(in_port.parent.path));
        // Of another model, or named by nothing it was, none is.
        assert_eq!(best(&was, vec![other], &[]), None);
        let unnamed = pad("/sys/devices/d", "seat0", &["addr-02:00:00:00:00:09"]);
        assert_eq!(best(&was, vec![unnamed], &[]), None);
    }

    #[test]
    fn a_pad_that_came_back_on_another_seat_or_device_kind_is_not_taken() {
        let was = pad("/sys/devices/old", "seat0", &["addr-0a:1b:2c:3d:4e:14"]);
        let seat1 = pad("/sys/devices/a", "seat1", &["addr-0a:1b:2c:3d:4e:14"]);
        assert_eq!(best(&was, vec![seat1], &[]), None);
        let mut made = pad("/sys/devices/b", "seat0", &["addr-0a:1b:2c:3d:4e:14"]);
        made.identity.virtual_device = true;
        assert_eq!(best(&was, vec![made], &[]), None);
    }

    #[test]
    fn markers_rank_from_what_names_one_pad_to_what_names_a_place() {
        let ranks: Vec<usize> = [
            "addr-x", "serial-x", "uniq-x", "port-x", "raw-x", "hid-x", "other",
        ]
        .iter()
        .map(|marker| rank(marker))
        .collect();
        assert!(ranks.windows(2).all(|pair| pair[0] < pair[1]), "{ranks:?}");
    }

    #[test]
    fn a_hidden_node_is_root_s_and_the_group_s_with_no_seat_access() {
        let hidden = Ownership {
            uid: 0,
            gid: 999,
            mode: 0o20660,
        };
        assert_eq!(unhidden(hidden, Some(":seat:"), 999), None);
        assert_eq!(unhidden(hidden, None, 999), None);
        assert!(unhidden(hidden, Some(":seat:uaccess:"), 999).is_some());
        assert!(unhidden(Ownership { gid: 174, ..hidden }, None, 999).is_some());
        assert!(
            unhidden(
                Ownership {
                    mode: 0o20664,
                    ..hidden
                },
                None,
                999
            )
            .is_some()
        );
        assert!(
            unhidden(
                Ownership {
                    mode: 0o20666,
                    ..hidden
                },
                None,
                999
            )
            .is_some()
        );
        assert!(
            unhidden(
                Ownership {
                    uid: 1000,
                    ..hidden
                },
                None,
                999
            )
            .is_some()
        );
        // A tag that only contains the word isn't the tag.
        assert_eq!(unhidden(hidden, Some(":seat:not-uaccess:"), 999), None);
    }

    fn acl(entries: &[(u16, u16, u32)]) -> Vec<u8> {
        let mut bytes = 2u32.to_le_bytes().to_vec();
        for (tag, permissions, id) in entries {
            bytes.extend(tag.to_le_bytes());
            bytes.extend(permissions.to_le_bytes());
            bytes.extend(id.to_le_bytes());
        }
        bytes
    }

    #[test]
    fn a_hidden_node_has_no_acl_entry_beyond_its_mode() {
        let read = |bytes: Vec<u8>| {
            move |buffer: &mut [u8]| {
                buffer[..bytes.len()].copy_from_slice(&bytes);
                Ok(bytes.len())
            }
        };
        // The mode as an ACL: owner, group, others.
        let mode = acl(&[
            (0x01, 6, u32::MAX),
            (0x04, 6, u32::MAX),
            (0x20, 0, u32::MAX),
        ]);
        assert_eq!(acl_wrong(read(mode)), None);
        // What logind's seat access adds: the user, and the mask.
        let seat = acl(&[
            (0x01, 6, u32::MAX),
            (0x02, 6, 1000),
            (0x04, 6, u32::MAX),
            (0x10, 6, u32::MAX),
            (0x20, 0, u32::MAX),
        ]);
        assert!(acl_wrong(read(seat)).is_some());
        // A named group alone.
        assert!(acl_wrong(read(acl(&[(0x08, 4, 174)]))).is_some());
        // Not shaped as an ACL.
        assert!(acl_wrong(read(vec![1, 0, 0, 0])).is_some());
        assert!(acl_wrong(read(vec![2, 0, 0, 0, 1])).is_some());
        // None at all, or none kept.
        assert_eq!(acl_wrong(|_| Err(rustix::io::Errno::NODATA)), None);
        assert_eq!(acl_wrong(|_| Err(rustix::io::Errno::OPNOTSUPP)), None);
        assert!(acl_wrong(|_| Err(rustix::io::Errno::RANGE)).is_some());
        assert!(acl_wrong(|_| Err(rustix::io::Errno::IO)).is_some());
    }
}
