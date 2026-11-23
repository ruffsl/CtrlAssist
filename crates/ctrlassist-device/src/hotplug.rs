//! Which pads are connected, and when that changes.
//!
//! Both come from udev, after it has finished with a device: a pad is
//! reported once its nodes and their permissions are in place, so it can
//! be opened at once.

use std::fs::File;
use std::io;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};

use rustix::event::{PollFd, PollFlags, Timespec, poll};

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
    /// The pad on an event node has gone.
    Left {
        /// The event node, whose name a node made since may have.
        node: PathBuf,
        /// Its place in sysfs, which no node made since has: what tells
        /// which pad this was, the node's name being handed on.
        syspath: PathBuf,
    },
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
    /// device's that isn't among them is going for the re-bind. A request
    /// whose nodes udev can't list is not said: taking a node still there
    /// for one going would leave it marked, and its going later, for no
    /// re-bind, taken for one. Unsaid, the re-bind is taken for the pad
    /// leaving and one connecting, as one by anyone but the service is.
    Rebinding {
        /// The device re-bound.
        anchor: PathBuf,
        /// The event nodes under it as the request is heard.
        present: Vec<PathBuf>,
    },
    /// Changes were lost, the monitor not read in time: what is connected
    /// is to be looked at again, with [`connected`] and [`anchors`]. Said
    /// once the queue has been read to its end. The kernel keeps what was
    /// queued before the loss, drops each change that comes after until
    /// the queue has been read empty, and says so before the first of
    /// those kept is read; so what was kept is passed on before this, in
    /// order, and what is passed on after this came after the loss.
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

/// The place in sysfs of each event node under `device` now; `None` if
/// udev can't be asked.
fn event_nodes_under(device: &udev::Device) -> Option<Vec<PathBuf>> {
    let listed = udev::Enumerator::new().and_then(|mut nodes| {
        nodes.match_parent(device)?;
        nodes.match_subsystem("input")?;
        nodes.match_sysname("event*")?;
        Ok(nodes
            .scan_devices()?
            .map(|node| node.syspath().to_path_buf())
            .collect())
    });
    listed.ok()
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

/// Opens the event node `node` for reading, as the node at `syspath` in
/// sysfs. A node's name is handed on: one opened a while after it was
/// found may be another pad's by then, which is refused. Opened first and
/// then looked up, it is the one at `syspath` if that is still there with
/// the opened file's number, a number being one device's for as long as
/// it is there.
///
/// # Errors
///
/// Fails if the node can't be opened, or is no longer the one at
/// `syspath`.
pub fn open_node(node: &Path, syspath: &Path) -> io::Result<File> {
    let file = File::open(node)?;
    let opened = rustix::fs::fstat(&file)?.st_rdev;
    let there = udev::Device::from_syspath(syspath)?.devnum();
    if there != Some(opened) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} is no longer {}", node.display(), syspath.display()),
        ));
    }
    Ok(file)
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
            || readable(socket),
            |event| match event {
                Some(event) => changed(&event.device(), event.event_type(), &mut change),
                None => change(Change::Overran),
            },
        );
    }
}

/// Whether something is queued on `socket` now.
fn readable(socket: &impl AsFd) -> bool {
    let mut fds = [PollFd::new(socket, PollFlags::IN)];
    let now = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    poll(&mut fds, Some(&now)).is_ok_and(|ready| ready > 0)
}

/// Reads what `next` gives until it gives nothing, passing each to
/// `each`. udev's library says why it had nothing in `errno`: once after
/// the kernel dropped events for a socket not read in time, it is
/// `ENOBUFS`. The kernel says that before the first event still queued,
/// and those queued were sent before the loss, so the loss is passed on,
/// as `None`, only once the socket has been read to its end, `readable`
/// saying when: udev's library has nothing for an event it sets aside as
/// well, an event from a sender it doesn't trust, with more queued.
fn drain<T>(
    mut next: impl FnMut() -> Option<T>,
    errno: impl Fn() -> Option<i32>,
    readable: impl Fn() -> bool,
    mut each: impl FnMut(Option<T>),
) {
    let mut overran = false;
    loop {
        match next() {
            Some(item) => each(Some(item)),
            None if errno() == Some(libc::ENOBUFS) => overran = true,
            None if overran && readable() => {}
            None => {
                if overran {
                    each(None);
                }
                return;
            }
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
            if let Some(present) = event_nodes_under(device) {
                change(Change::Rebinding {
                    anchor: device.syspath().to_path_buf(),
                    present,
                });
            }
        }
        udev::EventType::Add | udev::EventType::Change => {
            if let Some(found) = found(device) {
                change(Change::Arrived(found));
            }
        }
        udev::EventType::Remove => {
            // What it was is gone with it; only its node and its place
            // are left to say which pad this was.
            let is_event = device
                .sysname()
                .to_str()
                .is_some_and(|name| name.starts_with("event"));
            if let (true, Some(node)) = (is_event, device.devnode()) {
                change(Change::Left {
                    node: node.to_path_buf(),
                    syspath: device.syspath().to_path_buf(),
                });
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

    /// Drains `script` as the monitor drains its socket, the socket
    /// readable while any of the script is left, and says what was passed
    /// on, an overrun as `None`, and how much is left unread.
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
            || !script.borrow().is_empty(),
            |event| passed.borrow_mut().push(event),
        );
        (passed.into_inner(), script.into_inner().len())
    }

    /// The kernel says it dropped events before the first of those it
    /// kept, which were sent before the loss: the loss is said once they
    /// are read, so that what is connected is looked at again after them
    /// and not before. An event set aside by udev's library, with more
    /// queued, isn't the queue's end; once every event queued is read, it
    /// is.
    #[test]
    fn an_overrun_is_said_once_what_was_kept_is_read() {
        let (passed, left) = drained(vec![
            Read::Event(1),
            Read::Nothing(libc::ENOBUFS),
            Read::Event(2),
            Read::Nothing(libc::EAGAIN),
            Read::Event(3),
            Read::Nothing(libc::EAGAIN),
        ]);
        assert_eq!(passed, [Some(1), Some(2), Some(3), None]);
        assert_eq!(left, 0);
    }

    /// A node opened by name is the one at the place it was found, or is
    /// refused: here `/dev/null`, which is no other device's place.
    #[test]
    fn a_node_is_opened_only_as_the_device_it_was_found_as() {
        let null = Path::new("/dev/null");
        open_node(null, Path::new("/sys/devices/virtual/mem/null")).unwrap();
        let refused = open_node(null, Path::new("/sys/devices/virtual/mem/zero")).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::NotFound, "{refused}");
        assert!(open_node(null, Path::new("/sys/devices/virtual/mem/gone")).is_err());
    }

    /// A socket is readable while something is queued on it, and asking
    /// waits for nothing.
    #[test]
    fn a_socket_is_readable_while_something_is_queued() {
        let (one, other) = std::os::unix::net::UnixDatagram::pair().unwrap();
        assert!(!readable(&other));
        one.send(b"queued").unwrap();
        assert!(readable(&other));
        other.recv(&mut [0; 8]).unwrap();
        assert!(!readable(&other));
    }

    /// Without an overrun, nothing for whatever reason ends the read,
    /// what is left read at the socket's next wake.
    #[test]
    fn nothing_else_is_an_overrun() {
        for error in [libc::EINVAL, libc::EAGAIN] {
            let (passed, left) = drained(vec![Read::Nothing(error), Read::Event(1)]);
            assert_eq!((passed, left), (Vec::new(), 1));
        }
    }
}
