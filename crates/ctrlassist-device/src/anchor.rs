//! What a pad's nodes hang from: the device a re-bind of the pad's
//! driver leaves in place.
//!
//! Re-binding a pad's driver, as the hiding service does to hide a pad
//! and to show it again, removes the pad's input nodes and makes new
//! ones, and never removes the device whose driver it re-binds. So a node
//! that goes while that device is still there is the pad away for a
//! moment, and the device's own removal is the pad unplugged.

use std::path::PathBuf;

/// One enumeration of the device a pad's driver is bound to: its HID
/// device, or for a pad with none its USB interface. A pad with neither,
/// as a uinput pad another program made, has no anchor: nothing re-binds
/// it, and its node is what goes when it does.
///
/// This must be the device the hiding service records for the pad and
/// re-binds (its `pad::Device`), found and stamped the same way: an
/// anchor below that device would go with each re-bind, and the pad would
/// be taken for unplugged.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Anchor {
    /// Its place in sysfs.
    pub path: PathBuf,
    /// When udev first saw the USB device at or above it, for a device on
    /// USB: a USB interface's place is its port's, and whatever is next
    /// plugged in there has the same one. A HID device's place names one
    /// enumeration already, its instance number never handed out twice.
    pub enumeration: Option<u64>,
}

fn subsystem(device: &udev::Device) -> Option<&str> {
    device.subsystem()?.to_str()
}

fn is_usb(device: &udev::Device, kind: &str) -> bool {
    subsystem(device) == Some("usb")
        && device.devtype().and_then(|found| found.to_str()) == Some(kind)
}

/// The anchor of the pad whose node `node` is, read without opening it;
/// none for a pad with no driver to re-bind, or whose USB device udev
/// hasn't stamped (a node is reported once udev has finished with it,
/// and so with its parents, so that is a pad going as it is read).
pub(crate) fn of(node: &udev::Device) -> Option<Anchor> {
    let ancestors = || std::iter::successors(Some(node.clone()), udev::Device::parent);
    let device = ancestors()
        .find(|device| subsystem(device) == Some("hid"))
        .or_else(|| ancestors().find(|device| is_usb(device, "usb_interface")))?;
    let usb = std::iter::successors(Some(device.clone()), udev::Device::parent)
        .find(|device| is_usb(device, "usb_device"));
    let enumeration = match usb {
        Some(usb) => Some(
            usb.property_value("USEC_INITIALIZED")?
                .to_str()?
                .parse::<u64>()
                .ok()?,
        ),
        None => None,
    };
    Some(Anchor {
        path: device.syspath().to_path_buf(),
        enumeration,
    })
}

/// Whether a device udev reports removed is one a pad can be anchored
/// to: a HID device or a USB interface.
pub(crate) fn can_be(device: &udev::Device) -> bool {
    subsystem(device) == Some("hid") || is_usb(device, "usb_interface")
}
