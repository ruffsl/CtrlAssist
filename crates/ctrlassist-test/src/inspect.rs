//! Looking at pads the way a test needs to: what exists, what a user can
//! open, and whether anything still arrives on a node.

use std::fs::OpenOptions;
use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::OFlags;

/// The kinds of device node a pad can have.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeKind {
    /// `/dev/input/eventN`
    Event,
    /// `/dev/input/jsN`
    Js,
    /// `/dev/hidrawN`
    Hidraw,
}

/// A pad as udev knows it.
#[derive(Clone, Debug)]
pub struct Pad {
    /// The kernel's name for it.
    pub name: String,
    /// The address it reports, or empty.
    pub uniq: String,
    /// Its vendor ID, as the kernel prints it: four hex digits.
    pub vendor: String,
    /// Its product ID, as the kernel prints it: four hex digits.
    pub product: String,
    /// Its HID device's name, if it has one.
    pub hid: Option<String>,
    /// Its device nodes.
    pub nodes: Vec<(NodeKind, PathBuf)>,
}

impl Pad {
    /// The pad's node of one kind, if it has one.
    #[must_use]
    pub fn node(&self, kind: NodeKind) -> Option<&Path> {
        self.nodes
            .iter()
            .find(|(node_kind, _)| *node_kind == kind)
            .map(|(_, node)| node.as_path())
    }
}

fn attribute(device: &udev::Device, name: &str) -> String {
    device
        .attribute_value(name)
        .map(|value| value.to_string_lossy().trim().to_string())
        .unwrap_or_default()
}

fn in_subsystem(device: &udev::Device, wanted: &str) -> bool {
    device.subsystem().and_then(|subsystem| subsystem.to_str()) == Some(wanted)
}

/// Every pad udev classifies as a joystick, with its nodes.
///
/// # Errors
///
/// Fails if udev can't be queried.
pub fn pads() -> io::Result<Vec<Pad>> {
    let mut joysticks = udev::Enumerator::new()?;
    joysticks.match_subsystem("input")?;
    joysticks.match_property("ID_INPUT_JOYSTICK", "1")?;
    let mut found = Vec::new();
    for device in joysticks.scan_devices()? {
        let Some(node) = device.devnode() else {
            continue;
        };
        if !node.to_string_lossy().contains("/event") {
            continue;
        }
        // The input device (inputN) is the event node's parent; the HID
        // device, when there is one, owns the hidraw siblings.
        let Some(input) = device.parent() else {
            continue;
        };
        let hid = std::iter::successors(Some(input.clone()), udev::Device::parent)
            .find(|ancestor| in_subsystem(ancestor, "hid"));

        let mut nodes = vec![(NodeKind::Event, node.to_path_buf())];
        let mut children = udev::Enumerator::new()?;
        children.match_parent(hid.as_ref().unwrap_or(&input))?;
        for child in children.scan_devices()? {
            let Some(child_node) = child.devnode() else {
                continue;
            };
            let text = child_node.to_string_lossy();
            let ours = child
                .parent()
                .is_some_and(|parent| parent.syspath() == input.syspath());
            if text.contains("/js") && ours {
                nodes.push((NodeKind::Js, child_node.to_path_buf()));
            } else if text.contains("hidraw") {
                nodes.push((NodeKind::Hidraw, child_node.to_path_buf()));
            }
        }
        found.push(Pad {
            name: attribute(&input, "name"),
            uniq: attribute(&input, "uniq"),
            vendor: attribute(&input, "id/vendor"),
            product: attribute(&input, "id/product"),
            hid: hid.map(|hid| hid.sysname().to_string_lossy().into_owned()),
            nodes,
        });
    }
    Ok(found)
}

/// Whether this process can open a node, for reading or for reading and
/// writing.
#[must_use]
pub fn can_open(path: &Path, write: bool) -> bool {
    OpenOptions::new()
        .read(true)
        .write(write)
        .custom_flags(OFlags::NONBLOCK.bits() as i32)
        .open(path)
        .is_ok()
}

/// Whether this process may open a node just made, once udev is done
/// with it: for reading, or for reading and writing. It is asked of the
/// kernel without opening the node, which for some nodes is an act: a
/// Steam Controller's raw node opened takes its input devices away.
///
/// udev gives a node to the seat's user as it handles the node's coming,
/// after the node is made, and `udevadm settle` alone can return before
/// udev has even heard of it. So this waits, woken by udev's own events,
/// until udev has the node's device in its database, which it has once
/// it has heard of it, and then until udev's queue is empty, which it is
/// once it has done what its rules say, the seat's access among it; and
/// only then asks. A node a machine gives nobody is answered as soon as
/// udev is done with it. The wait gives up after ten seconds, only so
/// that a test whose udev never answers ends.
#[must_use]
pub fn may_open_once_udev_is_done(path: &Path, write: bool) -> bool {
    // Listening first, so that nothing between the look and the wait is
    // missed.
    let monitor = udev::MonitorBuilder::new().and_then(udev::MonitorBuilder::listen);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if is_in_udev_database(path) {
            break;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        let wait = Timespec::try_from(left).expect("ten seconds fit in a timespec");
        match &monitor {
            Ok(monitor) => {
                let mut fds = [PollFd::new(monitor, PollFlags::IN)];
                let _ = poll(&mut fds, Some(&wait));
                // What the events say isn't read: the look after them is.
                for _ in monitor.iter() {}
            }
            Err(_) => {
                let _ = poll(&mut [], Some(&wait));
            }
        }
    }
    let _ = std::process::Command::new("udevadm").arg("settle").status();
    let mode = if write {
        rustix::fs::Access::READ_OK | rustix::fs::Access::WRITE_OK
    } else {
        rustix::fs::Access::READ_OK
    };
    rustix::fs::access(path, mode).is_ok()
}

/// Whether udev has the device of the node at `path` in its database.
fn is_in_udev_database(path: &Path) -> bool {
    let Ok(stat) = rustix::fs::stat(path) else {
        return false;
    };
    udev::Device::from_devnum(udev::DeviceType::Character, stat.st_rdev)
        .is_ok_and(|device| device.is_initialized())
}

/// What became of a reader that watched a node for a while.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Reads {
    /// How many reads returned data.
    pub count: usize,
    /// Whether the node failed under the reader, as when its device is
    /// taken away.
    pub cut_off: bool,
}

/// Opens a node and counts the reads that return data within a window,
/// calling `each_second` with the running count so a caller can show
/// progress. Works on any node kind: it counts reads, not events.
///
/// # Errors
///
/// Fails if the node can't be opened or polled.
pub fn count_reads(
    node: &Path,
    window: Duration,
    each_second: &mut impl FnMut(u64, usize),
) -> io::Result<Reads> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(OFlags::NONBLOCK.bits() as i32)
        .open(node)?;
    let started = Instant::now();
    let deadline = started + window;
    let mut buffer = [0u8; 4096];
    let mut reads = Reads {
        count: 0,
        cut_off: false,
    };
    let mut reported = 0;
    loop {
        let now = Instant::now();
        let seconds = now.duration_since(started).as_secs();
        if seconds > reported {
            reported = seconds;
            each_second(seconds, reads.count);
        }
        if now >= deadline {
            return Ok(reads);
        }
        // Wake at least once a second so progress is reported while idle.
        let wait = deadline.duration_since(now).min(Duration::from_secs(1));
        let timeout = Timespec {
            tv_sec: wait.as_secs() as i64,
            tv_nsec: i64::from(wait.subsec_nanos()),
        };
        let mut fds = [PollFd::new(&file, PollFlags::IN)];
        if poll(&mut fds, Some(&timeout))? == 0 {
            continue;
        }
        match file.read(&mut buffer) {
            Ok(0) => {
                reads.cut_off = true;
                return Ok(reads);
            }
            Ok(_) => reads.count += 1,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => {
                reads.cut_off = true;
                return Ok(reads);
            }
        }
    }
}
