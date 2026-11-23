//! Which pads are connected, and when that changes.
//!
//! Both come from udev, after it has finished with a device: a pad is
//! reported once its nodes and their permissions are in place, so it can
//! be opened at once.

use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::PathBuf;

use crate::anchor::{self, Anchor};
use crate::pad::Identifiers;

/// A connected pad, before it is opened.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Found {
    /// Its event node.
    pub node: PathBuf,
    /// Its event node's place in sysfs, which no node made after it has:
    /// the input device above it is numbered afresh each time, where the
    /// event node's own number is handed on.
    pub syspath: PathBuf,
    /// What tells it from others.
    pub identifiers: Identifiers,
    /// What its node hangs from, where that could be read.
    pub anchor: Option<Anchor>,
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
    /// The device at this place in sysfs, which a pad can be
    /// [anchored](Anchor) to, has gone: after every node under it, udev
    /// sending a device's removal after its children's.
    Unplugged(PathBuf),
    /// The hiding service has asked udev to re-bind the driver of the
    /// device at `anchor`, which a pad can be anchored to. udev runs the
    /// rule that re-binds it before it sends the request on, so this
    /// comes once the kernel has removed the device's nodes and made new
    /// ones, and before udev sends any of that: `present` is the place in
    /// sysfs of each event node under the device now, and a node of the
    /// device's that isn't among them is going for the re-bind.
    Rebinding {
        /// The device re-bound.
        anchor: PathBuf,
        /// The event nodes under it as the request is heard.
        present: Vec<PathBuf>,
    },
    /// Changes were lost, the monitor not read in time: what is connected
    /// is to be looked at again, with [`connected`] and [`anchors`]. What
    /// is read after this came after the loss.
    Overran,
}

/// The argument the hiding service's request for a re-bind carries, as
/// udev has it: the kernel prefixes each argument written to a device's
/// `uevent` file with `SYNTH_ARG_`, and nothing a device or a driver
/// reports has that name. Only root and the service's group may write
/// the `uevent` files of the devices the service may ask for (the rule
/// gives those to the group), and udev's listeners take its events from
/// root alone, so neither a pad nor another user can forge it.
const REBIND: (&str, &str) = ("SYNTH_ARG_CTRLASSIST", "rebind");

/// Whether a udev change event is the hiding service's request for a
/// re-bind of a device a pad can be anchored to.
fn is_rebind(device: &udev::Device) -> bool {
    anchor::can_be(device)
        && device
            .property_value(REBIND.0)
            .is_some_and(|value| value == REBIND.1)
}

/// The place in sysfs of each event node under `device` now; none if
/// udev can't be asked.
fn event_nodes_under(device: &udev::Device) -> Vec<PathBuf> {
    let listed = udev::Enumerator::new().and_then(|mut nodes| {
        nodes.match_parent(device)?;
        nodes.match_subsystem("input")?;
        nodes.match_sysname("event*")?;
        Ok(nodes
            .scan_devices()?
            .map(|node| node.syspath().to_path_buf())
            .collect())
    });
    listed.unwrap_or_default()
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
pub(crate) fn found(device: &udev::Device) -> Option<Found> {
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
    let identifiers = Identifiers {
        name: attribute(&input, "name").unwrap_or_default(),
        bus: hex(attribute(&input, "id/bustype")),
        vendor: hex(attribute(&input, "id/vendor")),
        product: hex(attribute(&input, "id/product")),
        version: hex(attribute(&input, "id/version")),
        address: attribute(&input, "uniq"),
        serial,
        port: attribute(&input, "phys"),
    };
    // A pad that goes while it is being read leaves some of this empty,
    // the `phys` that marks the daemon's own pads included. What was
    // read is a pad only if its node was still there afterwards: once
    // its input device has gone, udev takes the nearest device above
    // that is still there for the node's parent, its HID device, say.
    let anchor = anchor::of(device);
    let syspath = device.syspath().to_path_buf();
    syspath.exists().then_some(Found {
        node,
        syspath,
        identifiers,
        anchor,
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

/// Every device a pad can be anchored to now, each as the enumeration it
/// is.
///
/// # Errors
///
/// Fails if udev can't be asked.
pub fn anchors() -> io::Result<Vec<Anchor>> {
    let mut found = Vec::new();
    for subsystem in ["hid", "usb"] {
        let mut devices = udev::Enumerator::new()?;
        devices.match_subsystem(subsystem)?;
        found.extend(
            devices
                .scan_devices()?
                .filter(anchor::can_be)
                .filter_map(|device| anchor::at(&device)),
        );
    }
    Ok(found)
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
    /// Beside input nodes it hears the devices a pad can be anchored to,
    /// filtered by udev before they are sent.
    ///
    /// Its socket's buffer is as large as an unprivileged process can
    /// have: udev's library asks 128 MiB, which the kernel caps at
    /// `net.core.rmem_max` for a process without `CAP_NET_ADMIN`, so
    /// nothing more is asked here. Events lost all the same are said by
    /// [`Change::Overran`].
    ///
    /// # Errors
    ///
    /// Fails if udev can't be listened to.
    pub fn new() -> io::Result<Self> {
        let socket = udev::MonitorBuilder::new()?
            .match_subsystem("input")?
            .match_subsystem("hid")?
            .match_subsystem_devtype("usb", "usb_interface")?
            .listen()?;
        Ok(Self { socket })
    }

    /// Passes every change waiting to `change`, and returns when none is
    /// left. The monitor is readable, as a descriptor, when one is.
    pub fn read(&mut self, mut change: impl FnMut(Change)) {
        let socket = &self.socket;
        drain(
            || socket.iter().next(),
            || io::Error::last_os_error().raw_os_error(),
            |event| match event {
                Some(event) => changed(&event.device(), event.event_type(), &mut change),
                None => change(Change::Overran),
            },
        );
    }
}

/// Reads what `next` gives until it gives nothing, passing each to
/// `each`. udev's library says why it had nothing in `errno`: once after
/// the kernel dropped events for a socket not read in time, it is
/// `ENOBUFS`, which is passed on as `None`, and what was queued after the
/// loss is read still.
fn drain<T>(
    mut next: impl FnMut() -> Option<T>,
    errno: impl Fn() -> Option<i32>,
    mut each: impl FnMut(Option<T>),
) {
    loop {
        match next() {
            Some(item) => each(Some(item)),
            None if errno() == Some(libc::ENOBUFS) => each(None),
            None => return,
        }
    }
}

/// Passes on what an event of `kind` on `device` says of pads, if
/// anything.
fn changed(device: &udev::Device, kind: udev::EventType, change: &mut impl FnMut(Change)) {
    match kind {
        udev::EventType::Remove if anchor::can_be(device) => {
            change(Change::Unplugged(device.syspath().to_path_buf()));
        }
        udev::EventType::Change if is_rebind(device) => {
            change(Change::Rebinding {
                anchor: device.syspath().to_path_buf(),
                present: event_nodes_under(device),
            });
        }
        udev::EventType::Add | udev::EventType::Change => {
            if let Some(found) = found(device) {
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

impl AsFd for Monitor {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.socket.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::collections::VecDeque;

    use super::*;

    /// What the socket gives, in turn: an event, or nothing with `errno`
    /// saying why.
    enum Read {
        Event(u32),
        Nothing(i32),
    }

    /// Drains `script` as the monitor drains its socket, and says what
    /// was passed on, an overrun as `None`, and how much is left unread.
    fn drained(script: Vec<Read>) -> (Vec<Option<u32>>, usize) {
        let script = RefCell::new(VecDeque::from(script));
        let errno = Cell::new(0);
        let passed = RefCell::new(Vec::new());
        drain(
            || match script.borrow_mut().pop_front()? {
                Read::Event(event) => Some(event),
                Read::Nothing(error) => {
                    errno.set(error);
                    None
                }
            },
            || Some(errno.get()),
            |event| passed.borrow_mut().push(event),
        );
        (passed.into_inner(), script.into_inner().len())
    }

    /// Events lost to a socket not read in time are said, once each time
    /// the kernel says so, and what was queued after is still read.
    #[test]
    fn an_overrun_is_said_and_what_follows_is_read() {
        let (passed, left) = drained(vec![
            Read::Event(1),
            Read::Nothing(libc::ENOBUFS),
            Read::Event(2),
            Read::Nothing(libc::EAGAIN),
            Read::Event(3),
        ]);
        assert_eq!(passed, [Some(1), None, Some(2)]);
        assert_eq!(left, 1, "read at the socket's next wake");
    }

    /// Nothing for any other reason ends the read without an overrun.
    #[test]
    fn nothing_else_is_an_overrun() {
        let (passed, left) = drained(vec![Read::Nothing(libc::EINVAL), Read::Event(1)]);
        assert_eq!((passed, left), (Vec::new(), 1));
    }
}
