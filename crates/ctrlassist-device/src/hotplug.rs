//! Which pads are connected, and when that changes.
//!
//! Both come from udev, after it has finished with a device: a pad is
//! reported once its nodes and their permissions are in place, so it can
//! be opened at once.

use std::fs::{File, OpenOptions};
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
    /// A Steam Controller pad's raw node came, or udev was asked to
    /// handle it again, as it is once a rule that gives it to the seat is
    /// installed: the pad on this node is there, as for
    /// [`Arrived`](Self::Arrived), and what may be done with its raw node
    /// may have changed.
    RawNodeChanged(Found),
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

/// The driver that binds a Steam Controller, its own HID device and the
/// client HID device it makes beside it alike.
const STEAM_DRIVER: &str = "hid-steam";

fn is_hid(device: &udev::Device) -> bool {
    device.subsystem().and_then(|found| found.to_str()) == Some("hid")
}

/// The HID device a node hangs from.
fn hid_of(node: &udev::Device) -> Option<udev::Device> {
    std::iter::successors(Some(node.clone()), udev::Device::parent).find(is_hid)
}

/// The group the kernel's driver puts the client HID device it makes in,
/// as the device's `MODALIAS` says it (`hid:b<bus>g<group>v…`):
/// `HID_GROUP_STEAM`. A Puck's interface for its charging pins, or any
/// other of the same vendor and product the driver doesn't take for a
/// pad's own, has a raw node too, and isn't in it.
const STEAM_CLIENT_GROUP: &str = "g0103";

/// Whether `device` is a client HID device `hid-steam` made, through
/// which a program reads a Steam Controller pad raw.
fn is_steam_client(device: &udev::Device) -> bool {
    let group = device
        .property_value("MODALIAS")
        .and_then(|modalias| modalias.to_str())
        .and_then(|modalias| modalias.strip_prefix("hid:b"))
        .and_then(|rest| rest.get(4..9));
    is_hid(device)
        && device.driver().and_then(|driver| driver.to_str()) == Some(STEAM_DRIVER)
        && group == Some(STEAM_CLIENT_GROUP)
}

/// The raw nodes, udev having finished with each, that could be the one
/// a Steam Controller is read through, the pad's HID device being `hid`.
/// That is the node of the client HID device the kernel's driver makes
/// beside the pad's own, which has none: a sibling of it in the driver's
/// own group, that says it is plugged where the pad does (`HID_PHYS`).
/// The driver makes it after the pad's input devices, so a pad can be
/// seen before its raw node is there to open. On USB the parent is the
/// pad's own interface, and the node is the one; over Bluetooth it is the
/// uhid device every Bluetooth pad shares, and two pads on one adapter
/// say the same `HID_PHYS`.
fn steam_raw_beside(hid: &udev::Device) -> Vec<udev::Device> {
    let (Some(parent), Some(phys)) = (hid.parent(), hid.property_value("HID_PHYS")) else {
        return Vec::new();
    };
    let listed = udev::Enumerator::new().and_then(|mut nodes| {
        nodes.match_parent(&parent)?;
        nodes.match_subsystem("hidraw")?;
        Ok(nodes.scan_devices()?.collect::<Vec<_>>())
    });
    let Ok(listed) = listed else {
        return Vec::new();
    };
    listed
        .into_iter()
        .filter(udev::Device::is_initialized)
        .filter(|raw| {
            raw.parent().is_some_and(|client| {
                is_steam_client(&client)
                    && client.syspath() != hid.syspath()
                    && client.property_value("HID_PHYS") == Some(phys)
            })
        })
        .collect()
}

/// Where a Steam Controller is read, found from its event node: see
/// [`steam_raw_node`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SteamRaw {
    /// The event node's place in sysfs: the node the kernel's driver
    /// takes away while the raw node is open.
    pub syspath: PathBuf,
    /// The raw node to read the pad through.
    pub node: PathBuf,
}

/// Why a Steam Controller's raw node wasn't found.
#[derive(Debug)]
#[non_exhaustive]
pub enum SteamRawError {
    /// It isn't there to find, or the pad isn't: the event node went as
    /// it was looked up, or the driver hasn't made the raw node yet. The
    /// pad is to be found again once it is there.
    Missing(io::Error),
    /// More than one raw node could be the pad's, and nothing udev says
    /// tells them apart, as for two pads on one Bluetooth adapter: this
    /// lasts for as long as both are there.
    Ambiguous {
        /// The pad's HID device.
        hid: PathBuf,
    },
}

impl std::fmt::Display for SteamRawError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing(error) => write!(f, "its raw node isn't there: {error}"),
            Self::Ambiguous { hid } => write!(
                f,
                "more than one raw node of {STEAM_DRIVER} could be that of {}, and which is can't be told",
                hid.display()
            ),
        }
    }
}

impl std::error::Error for SteamRawError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Missing(error) => Some(error),
            Self::Ambiguous { .. } => None,
        }
    }
}

/// The raw node of the Steam Controller whose event node `event` is open,
/// which is the client HID device's that the kernel's driver makes beside
/// the pad's own HID device, and the event node's place in sysfs. Found by
/// the open descriptor's device number, which names one device for as
/// long as it is there, and not by a node's name, which is handed on.
///
/// # Errors
///
/// [`SteamRawError::Missing`] if the descriptor's node is no longer one
/// udev knows, or the pad has no raw node beside it (its driver isn't
/// `hid-steam`, or hasn't made the node yet); [`SteamRawError::Ambiguous`]
/// if more than one could be its.
pub fn steam_raw_node(event: BorrowedFd<'_>) -> Result<SteamRaw, SteamRawError> {
    let missing = |why: &str| {
        SteamRawError::Missing(io::Error::new(io::ErrorKind::NotFound, why.to_string()))
    };
    let number = rustix::fs::fstat(event)
        .map_err(|error| SteamRawError::Missing(error.into()))?
        .st_rdev;
    let device = udev::Device::from_devnum(udev::DeviceType::Character, number)
        .map_err(SteamRawError::Missing)?;
    let hid = hid_of(&device)
        .ok_or_else(|| missing("the pad hangs from no HID device, so it has no raw node"))?;
    let mut beside = steam_raw_beside(&hid).into_iter();
    let raw = match (beside.next(), beside.next()) {
        (Some(raw), None) => raw,
        (None, _) => {
            return Err(missing(&format!(
                "no raw node of {STEAM_DRIVER} is beside {}",
                hid.syspath().display()
            )));
        }
        (Some(_), Some(_)) => {
            return Err(SteamRawError::Ambiguous {
                hid: hid.syspath().to_path_buf(),
            });
        }
    };
    let node = raw
        .devnode()
        .ok_or_else(|| missing("the raw node has no device file"))?;
    Ok(SteamRaw {
        syspath: device.syspath().to_path_buf(),
        node: node.to_path_buf(),
    })
}

/// Whether the event node `event` is open on has gone, its input device
/// unregistered: the kernel then has the descriptor hang up, and go on
/// saying so for as long as it is open. Asked without waiting.
#[must_use]
pub fn event_node_gone(event: BorrowedFd<'_>) -> bool {
    let mut fds = [PollFd::new(&event, PollFlags::IN)];
    let now = Timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    poll(&mut fds, Some(&now)).is_ok_and(|_| {
        fds[0]
            .revents()
            .intersects(PollFlags::HUP | PollFlags::ERR | PollFlags::NVAL)
    })
}

/// The pads a Steam Controller's raw node, which udev has just finished
/// with, completes: each event node of a Steam Controller beside it that
/// udev has finished with too. Such a pad is [found](found) only once
/// its raw node is there, and the driver makes that after the pad's input
/// devices, so its arrival can come here, after its own.
fn steam_pads_completed_by(raw: &udev::Device) -> Vec<Found> {
    let Some(client) = raw.parent().filter(is_steam_client) else {
        return Vec::new();
    };
    let (Some(parent), Some(phys)) = (client.parent(), client.property_value("HID_PHYS")) else {
        return Vec::new();
    };
    let listed = udev::Enumerator::new().and_then(|mut nodes| {
        nodes.match_parent(&parent)?;
        nodes.match_subsystem("input")?;
        nodes.match_sysname("event*")?;
        Ok(nodes.scan_devices()?.collect::<Vec<_>>())
    });
    let Ok(listed) = listed else {
        return Vec::new();
    };
    listed
        .into_iter()
        .filter(udev::Device::is_initialized)
        .filter(|node| {
            hid_of(node).is_some_and(|hid| {
                hid.syspath() != client.syspath() && hid.property_value("HID_PHYS") == Some(phys)
            })
        })
        .filter_map(|node| found(&node))
        .filter(|found| found.identifiers.is_steam_controller())
        .collect()
}

/// The pad a udev event device is, read without opening it. A Steam
/// Controller pad, which is read through its raw node, is a pad once that
/// is there and udev has finished with it, so that it can be opened at
/// once as any other pad's nodes can.
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
    if identifiers.is_steam_controller()
        && hid_of(device).is_none_or(|hid| steam_raw_beside(&hid).is_empty())
    {
        return None;
    }
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

/// Opens the event node `node` with `options`, as the node at `syspath`
/// in sysfs. A node's name is handed on: one opened a while after it was
/// found may be another pad's by then, which is refused. Opened first and
/// then looked up, it is the one at `syspath` if that is still there with
/// the opened file's number, a number being one device's for as long as
/// it is there.
///
/// # Errors
///
/// Fails if the node can't be opened, or is no longer the one at
/// `syspath`.
pub fn open_node(node: &Path, syspath: &Path, options: &OpenOptions) -> io::Result<File> {
    let file = options.open(node)?;
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
    /// and raw nodes, the last of which a Steam Controller needs to be a
    /// pad; each filtered by udev before it is sent.
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
            .match_subsystem("hidraw")?
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
        udev::EventType::Add | udev::EventType::Change
            if device.subsystem().and_then(|found| found.to_str()) == Some("hidraw") =>
        {
            for found in steam_pads_completed_by(device) {
                change(Change::RawNodeChanged(found));
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
        let options = OpenOptions::new().read(true).write(true).clone();
        open_node(null, Path::new("/sys/devices/virtual/mem/null"), &options).unwrap();
        let refused =
            open_node(null, Path::new("/sys/devices/virtual/mem/zero"), &options).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::NotFound, "{refused}");
        let gone = Path::new("/sys/devices/virtual/mem/gone");
        assert!(open_node(null, gone, &options).is_err());
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

    /// Why a Steam Controller's raw node wasn't found is said in a
    /// person's words, with the error underneath where there is one.
    #[test]
    fn why_a_raw_node_wasn_t_found_is_said() {
        use std::error::Error as _;
        let missing = SteamRawError::Missing(io::Error::other("gone"));
        assert_eq!(missing.to_string(), "its raw node isn't there: gone");
        assert!(missing.source().is_some());
        let ambiguous = SteamRawError::Ambiguous {
            hid: PathBuf::from("/sys/devices/somewhere"),
        };
        assert_eq!(
            ambiguous.to_string(),
            "more than one raw node of hid-steam could be that of /sys/devices/somewhere, and which is can't be told"
        );
        assert!(ambiguous.source().is_none());
    }

    /// A node that is no pad's has no raw node beside it, and one that
    /// is still there hasn't gone.
    #[test]
    fn a_node_that_is_no_pad_s_has_no_raw_node() {
        use std::os::fd::AsFd as _;
        let null = File::open("/dev/null").unwrap();
        assert!(matches!(
            steam_raw_node(null.as_fd()),
            Err(SteamRawError::Missing(_))
        ));
        assert!(!event_node_gone(null.as_fd()));
    }
}
