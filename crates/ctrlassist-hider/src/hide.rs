//! Hiding a pad and showing it again.
//!
//! A static udev rule, shipped with the package, hides the nodes of any
//! pad named by a marker file in [`MARKERS`]: they go to the service's
//! group alone and lose seat access. Hiding a pad is creating its markers,
//! then asking udev to re-bind its driver, which cuts off whatever had
//! the pad open and makes its nodes again, hidden as they are made.
//! Showing it again is deleting the markers and asking for the same.
//!
//! The service writes nothing else: no rule, no record. A marker is made
//! before the re-bind it takes effect at, so whatever a service that died
//! left hidden is named by a marker, and undoing it needs nothing more.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::os::fd::{BorrowedFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use ctrlassist_hider::Refusal;
use rustix::event::{PollFd, PollFlags, poll};
use rustix::fs::{Access, OFlags};
use rustix::io::Errno;
use rustix::rand::{GetRandomFlags, getrandom};
use rustix::time::{ClockId, clock_gettime};
use tracing::{debug, error, warn};

use crate::pad::{self, Device, Pad};

/// Where the markers are, as the shipped rule names it. The unit's
/// runtime directory, kept when the service stops.
const MARKERS: &str = "/run/ctrlassist/hide";

/// Whether `marker` names a file directly in the marker directory, as the
/// rule tests every marker.
fn is_flat(marker: &str) -> bool {
    !marker.is_empty() && marker != "." && marker != ".." && !marker.contains(['/', '\0'])
}

fn marker_path(marker: &str) -> PathBuf {
    Path::new(MARKERS).join(marker)
}

/// Creates a marker: a file directly in the marker directory.
pub(crate) fn create(marker: &str) -> io::Result<()> {
    if !is_flat(marker) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a marker is a file in the marker directory, not a path",
        ));
    }
    fs::write(marker_path(marker), b"")
}

/// Deletes a marker. One already gone is no failure.
pub(crate) fn delete(marker: &str) {
    if !is_flat(marker) {
        return;
    }
    match fs::remove_file(marker_path(marker)) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            warn!(marker, %error, "couldn't delete a marker; the pad it names stays hidden at its next event");
        }
    }
}

/// Deletes everything in the marker directory, whatever made it.
pub(crate) fn clear() {
    let Ok(entries) = fs::read_dir(MARKERS) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let removed = if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        if let Err(error) = removed {
            warn!(marker = %path.display(), %error, "couldn't delete a marker");
        }
    }
}

/// A random UUID, as a synthetic event carries to say it is this one.
fn uuid() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom(&mut bytes, GetRandomFlags::empty())?;
    let mut text = String::new();
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        let _ = write!(text, "{byte:02x}");
    }
    Ok(text)
}

/// The time on `CLOCK_MONOTONIC` in microseconds, the clock udev stamps
/// a device it has finished with by.
fn now() -> u64 {
    let now = clock_gettime(ClockId::Monotonic);
    u64::try_from(now.tv_sec).unwrap_or(0) * 1_000_000
        + u64::try_from(now.tv_nsec).unwrap_or(0) / 1_000
}

/// Whether the service may ask for `device` to be re-bound: the shipped
/// rule gives the group the `uevent` file of a pad's device as it first
/// sees the pad. Where the group can't write it, the rule isn't
/// installed, or didn't take the device for a pad's; or the device has
/// gone.
pub(crate) fn may_request(device: &Device) -> Result<(), Refusal> {
    let uevent = device.path.join("uevent");
    rustix::fs::access(&uevent, Access::WRITE_OK).map_err(|error| {
        if !device.is_there() {
            return Refusal::Absent;
        }
        error!(
            file = %uevent.display(),
            %error,
            "the shipped rule hasn't given the service this pad's uevent file: it isn't installed, or didn't take the device for a pad's"
        );
        Refusal::Failed
    })
}

/// Asks udev to re-bind the driver of the device at `parent`: a change
/// event on it, marked with the service's own argument, which the shipped
/// rule acts on. The rule gives the service's group the `uevent` file of
/// pads' devices only. Returns the event's UUID.
pub(crate) fn request(parent: &Path) -> io::Result<String> {
    let uuid = uuid()?;
    fs::write(
        parent.join("uevent"),
        format!("change {uuid} CTRLASSIST=rebind"),
    )?;
    Ok(uuid)
}

/// What a re-bind is to leave a pad's nodes as.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Leave {
    /// Hidden as the service's group's, with its `ordinal`th joystick
    /// node made, for the pad to be opened again.
    Hidden { group: u32, ordinal: usize },
    /// As the rule makes them, hidden or not.
    AsTheRuleSays,
}

/// Where a re-bind has got to, read from sysfs and udev's record.
#[derive(Debug, PartialEq, Eq)]
enum Progress {
    Waiting,
    Done,
    Failed(Refusal),
}

/// What a re-bind came to.
#[derive(Debug)]
pub(crate) struct Rebound {
    pub(crate) outcome: Result<(), Refusal>,
    /// Whether udev was asked for it. A wait given up before udev's
    /// answer leaves the request in udev's queue, and the device's next
    /// re-bind, such as the one that shows the pad again, is run after it.
    pub(crate) asked: bool,
}

impl Rebound {
    fn not_asked(refusal: Refusal) -> Self {
        Self {
            outcome: Err(refusal),
            asked: false,
        }
    }

    fn asked(outcome: Result<(), Refusal>) -> Self {
        Self {
            outcome,
            asked: true,
        }
    }
}

/// Where the re-bind of `device` has got to. `before` is its inputs
/// before it was asked for, `since` when it was asked for, `answered`
/// whether udev has said it acted on the request, and `overran` whether
/// the monitor has lost events since, which the answer may have been.
///
/// Done needs the answer: until it comes, what sysfs and udev's record
/// say may be the doing of an earlier re-bind still in udev's queue. Only
/// after an overrun may state alone say done, and that rests on the order
/// udev runs events in: one only once every earlier event of the same
/// device, its parents and its children has run. The nodes an earlier
/// re-bind makes are the device's children, their events queued after
/// this request, so udev finishes none of them before this request has
/// run, whose unbind removes them. Nodes finished since the request are
/// then this re-bind's, but for one case: inputs the kernel removed and
/// made again between `before` being read and the request being written,
/// by an earlier re-bind run in that moment, a driver making its inputs
/// anew, or someone else re-binding the pad, which would be taken for
/// this re-bind.
///
/// The checks of state that fail come before the answer is needed, to
/// fail early: the inputs still there or no driver once udev has
/// answered, a node made not hidden. A failure seen with the device gone
/// is its unplugging, whichever read saw it go.
fn progress(
    device: &Device,
    before: &[PathBuf],
    since: u64,
    leave: Leave,
    answered: bool,
    overran: bool,
) -> Progress {
    match looked(device, before, since, leave, answered, overran) {
        Progress::Failed(_) if !device.is_there() => {
            debug!(device = %device.path.display(), "the pad was unplugged as its driver was re-bound");
            Progress::Failed(Refusal::Absent)
        }
        progress => progress,
    }
}

/// [`progress`] as read once.
fn looked(
    device: &Device,
    before: &[PathBuf],
    since: u64,
    leave: Leave,
    answered: bool,
    overran: bool,
) -> Progress {
    let parent = device.path.as_path();
    let shown = parent.display();
    if !device.is_there() {
        return Progress::Failed(Refusal::Absent);
    }
    // The rule unbinds the driver and probes again before udev says it
    // has acted on the request, so after that the inputs that were there
    // are gone, and a driver is bound if any will be.
    if before.iter().any(|input| input.exists()) {
        if answered {
            error!(
                device = %shown,
                "udev acted on the request to re-bind the pad's driver without re-binding it: the shipped rule isn't installed, or isn't the one shipped"
            );
            return Progress::Failed(Refusal::Failed);
        }
        return Progress::Waiting;
    }
    if !pad::has_driver(parent) {
        if answered {
            warn!(device = %shown, "no driver took the pad again after its driver was re-bound");
            return Progress::Failed(Refusal::Failed);
        }
        return Progress::Waiting;
    }
    // Bound again: whatever the probe made is there, and udev has each
    // node to finish with.
    let nodes = pad::nodes_under(parent);
    if !nodes.iter().all(|node| pad::finished_since(node, since)) {
        return Progress::Waiting;
    }
    if let Leave::Hidden { group, ordinal } = leave {
        for node in &nodes {
            if let Some(wrong) = pad::unhidden_node(node, group) {
                error!(
                    device = %shown,
                    node = %node.devnode().unwrap_or(node.syspath()).display(),
                    wrong,
                    "a node of the pad wasn't hidden when made again: another rule undoes the shipped one, or it isn't installed"
                );
                return Progress::Failed(Refusal::Failed);
            }
        }
        // A driver may make its pad's input later than its probe, as one
        // for a wireless receiver does once the pad reports in.
        if pad::event_node(parent, ordinal).is_none() {
            return Progress::Waiting;
        }
    }
    if !answered && !overran {
        return Progress::Waiting;
    }
    Progress::Done
}

/// Listens to udev's events for pads' devices.
fn listen() -> io::Result<udev::MonitorSocket> {
    udev::MonitorBuilder::new()
        .and_then(|monitor| monitor.match_subsystem("hid"))
        .and_then(|monitor| monitor.match_subsystem("usb"))
        .and_then(|monitor| monitor.match_subsystem("input"))
        .and_then(|monitor| monitor.match_subsystem("hidraw"))
        .and_then(udev::MonitorBuilder::listen)
}

/// A re-bind udev has been asked for, watched until its answer.
struct Watch {
    /// Listening since before the request.
    monitor: udev::MonitorSocket,
    device: Device,
    uuid: String,
    before: Vec<PathBuf>,
    since: u64,
    answered: bool,
    overran: bool,
}

impl Watch {
    /// Reads every event udev has sent, each only a reason to look at
    /// state again, but for the request's own answer.
    fn drain(&mut self) {
        loop {
            if let Some(event) = self.monitor.iter().next() {
                self.answered |= event.event_type() == udev::EventType::Change
                    && event.syspath() == self.device.path
                    && event
                        .property_value("SYNTH_UUID")
                        .and_then(|value| value.to_str())
                        == Some(self.uuid.as_str());
                continue;
            }
            // udev's library says why it had nothing: once after the
            // kernel dropped events for a socket not read in time, it is
            // ENOBUFS, and the answer may have been among them. What is
            // queued after is still read.
            if io::Error::last_os_error().raw_os_error() == Some(libc::ENOBUFS) {
                if !self.overran {
                    warn!(
                        device = %self.device.path.display(),
                        "udev's events overran the service's monitor; the pad's re-bind is judged on state alone"
                    );
                }
                self.overran = true;
                continue;
            }
            return;
        }
    }

    fn progress(&self, leave: Leave) -> Progress {
        progress(
            &self.device,
            &self.before,
            self.since,
            leave,
            self.answered,
            self.overran,
        )
    }
}

/// What a wait polls a client's socket for: its hanging up alone, so that
/// whatever it sends meanwhile, an empty message among it, stays queued,
/// to be read in its turn once the wait is over.
fn watched(client: BorrowedFd<'_>) -> PollFd<'_> {
    PollFd::from_borrowed_fd(client, PollFlags::RDHUP)
}

/// Whether what a poll found of a client is its hanging up, whole or
/// sending alone.
fn hung_up(revents: PollFlags) -> bool {
    revents.intersects(PollFlags::HUP | PollFlags::RDHUP | PollFlags::ERR)
}

/// Has udev re-bind the driver of `device`, and waits until udev has
/// answered and finished with the nodes that makes. Gives up if the pad
/// is unplugged, or if `client`, the one that asked, hangs up meanwhile.
///
/// It waits on state, read afresh at every event udev sends for a pad's
/// devices, and on the request's own answer: see [`progress`]. An earlier
/// re-bind of the device still in udev's queue runs before this one and
/// is never taken for it.
pub(crate) fn rebind(device: &Device, leave: Leave, client: Option<BorrowedFd<'_>>) -> Rebound {
    let parent = device.path.as_path();
    let shown = parent.display();
    if let Err(refusal) = may_request(device) {
        return Rebound::not_asked(refusal);
    }
    // Listening from before the request, so nothing it causes is missed.
    let monitor = match listen() {
        Ok(monitor) => monitor,
        Err(error) => {
            warn!(%error, "couldn't listen to udev");
            return Rebound::not_asked(Refusal::Failed);
        }
    };
    // The inputs this re-bind is to remove, or an earlier one still in
    // udev's queue: either way they are gone once this one has run.
    let before = pad::inputs_under(parent);
    let since = now();
    let uuid = match request(parent) {
        Ok(uuid) => uuid,
        Err(error) => {
            if !device.is_there() {
                return Rebound::not_asked(Refusal::Absent);
            }
            warn!(device = %shown, %error, "couldn't ask udev to re-bind the pad's driver");
            return Rebound::not_asked(Refusal::Failed);
        }
    };
    let mut watch = Watch {
        monitor,
        device: device.clone(),
        uuid,
        before,
        since,
        answered: false,
        overran: false,
    };
    loop {
        let mut fds = vec![PollFd::new(&watch.monitor, PollFlags::IN)];
        if let Some(client) = client {
            fds.push(watched(client));
        }
        match poll(&mut fds, None) {
            Ok(_) | Err(Errno::INTR) => {}
            Err(error) => {
                drop(fds);
                warn!(device = %shown, %error, "couldn't wait on udev for the pad's driver to be re-bound");
                return Rebound::asked(Err(Refusal::Failed));
            }
        }
        let gone = fds.get(1).is_some_and(|fd| hung_up(fd.revents()));
        drop(fds);
        // udev's side first: a re-bind seen to end in the same wake as the
        // client's hanging up is told as it ended.
        watch.drain();
        match watch.progress(leave) {
            Progress::Waiting => {}
            Progress::Done => return Rebound::asked(Ok(())),
            Progress::Failed(refusal) => return Rebound::asked(Err(refusal)),
        }
        if gone {
            debug!(device = %shown, "the client hung up as the pad's driver was re-bound");
            return Rebound::asked(Err(Refusal::Failed));
        }
    }
}

/// Opens a pad's node for a client, for reading and writing, if it is
/// still a node of `parent`, the enumeration of the device that was
/// checked: a node found by its path could have gone, and its number been
/// given to another device's, before it was opened, and on USB another
/// pad can have taken the port meanwhile. With `hidden`, the node must
/// also be as the rule hides a node for that group.
pub(crate) fn open_under(
    node: &Path,
    parent: &Device,
    hidden: Option<u32>,
) -> Result<OwnedFd, Refusal> {
    let gone = |refusal| {
        if parent.is_there() {
            refusal
        } else {
            Refusal::Absent
        }
    };
    let fd: OwnedFd = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags((OFlags::NONBLOCK | OFlags::CLOEXEC).bits() as i32)
        .open(node)
        .map_err(|error| {
            warn!(node = %node.display(), %error, "couldn't open a pad's node");
            gone(Refusal::Failed)
        })?
        .into();
    let stat = rustix::fs::fstat(&fd).map_err(|error| {
        warn!(node = %node.display(), %error, "couldn't look at a pad's node once open");
        gone(Refusal::Failed)
    })?;
    // Under the device's place, and that place still this enumeration of
    // it: looked at once the node is open, so a device replaced before or
    // after the opening is seen either way.
    let opened = pad::device_of(udev::DeviceType::Character, stat.st_rdev)
        .filter(|device| device.syspath().starts_with(&parent.path) && parent.is_there());
    let Some(opened) = opened else {
        warn!(
            node = %node.display(),
            device = %parent.path.display(),
            "a pad's node was another device's by the time it was opened"
        );
        return Err(Refusal::Absent);
    };
    if let Some(group) = hidden {
        let ownership = pad::Ownership {
            uid: stat.st_uid,
            gid: stat.st_gid,
            mode: stat.st_mode,
        };
        let tags = opened
            .property_value("CURRENT_TAGS")
            .and_then(|tags| tags.to_str());
        // The ACL too, read on the descriptor opened: seat access is given
        // by one, and a node with an entry beyond its mode is open to
        // someone its mode doesn't say.
        let wrong = pad::unhidden(ownership, tags, group)
            .or_else(|| pad::acl_wrong(|buffer| rustix::fs::fgetxattr(&fd, pad::ACL, buffer)));
        if let Some(wrong) = wrong {
            error!(
                node = %node.display(),
                wrong,
                "a hidden pad's node isn't hidden: another rule undoes the shipped one, or it isn't installed"
            );
            return Err(Refusal::Failed);
        }
    }
    Ok(fd)
}

/// The device of each pad, and of each raw node, the rule hides that
/// nothing should, with the markers that would name it: one that no pad
/// in `held` is and that no marker in `markers` names, as a pad sharing
/// an address with one that was hidden, or plugged into its port while
/// it was away. A pad's event nodes hidden are known by their group; a
/// raw node by what the rule sets on a node it hides.
pub(crate) fn unnamed(
    group: u32,
    held: &[&Path],
    markers: &BTreeSet<String>,
) -> Vec<(Device, BTreeSet<String>)> {
    let mut found: Vec<(Device, BTreeSet<String>)> = pad::hidden(group)
        .into_iter()
        .map(|pad| (pad.parent, pad.markers))
        .collect();
    for (device, names) in pad::hidden_raw() {
        match found
            .iter_mut()
            .find(|(other, _)| other.path == device.path)
        {
            Some((_, markers)) => markers.extend(names),
            None => found.push((device, names)),
        }
    }
    found
        .into_iter()
        .filter(|(device, names)| {
            !held.contains(&device.path.as_path()) && names.is_disjoint(markers)
        })
        .collect()
}

/// Asks for each pad the rule hides that nothing should, as [`unnamed`]
/// finds them, to be shown again by re-binding its driver, which makes
/// every node afresh as the stock rules make it. A raw node's own change
/// event isn't the service's to send, its `uevent` file being root's; its
/// HID device's is, for a pad's.
///
/// A device with a keyboard or a pointer among its inputs is left as it
/// is, until it is plugged in again: a hide refuses one, the re-bind
/// cutting that off too.
///
/// Waits for none of them: udev runs a device's events in order, so a
/// later re-bind of the device, such as a hide's, runs after this one and
/// is judged on its own answer.
pub(crate) fn sweep(group: u32, held: &[&Path], markers: &BTreeSet<String>) {
    for (device, _) in unnamed(group, held, markers) {
        if pad::shares_with_keyboard_or_pointer(&device.path) {
            warn!(
                device = %device.path.display(),
                "left hidden a pad no marker names, which has a keyboard or a pointer among its inputs: re-binding its driver would cut that off; it is shown again when plugged in again"
            );
            continue;
        }
        match request(&device.path) {
            Ok(_) => {
                debug!(device = %device.path.display(), "asked for a pad no marker names to be shown again");
            }
            Err(error) => warn!(
                device = %device.path.display(),
                %error,
                "couldn't ask for a pad no marker names to be shown again; it stays hidden until it is plugged in again"
            ),
        }
    }
}

/// Deletes every marker, then shows again every pad the rule still hides,
/// as [`sweep`] does: undoes what an earlier service left hidden, run
/// when no connection can own anything.
pub(crate) fn reconcile(group: u32) {
    clear();
    sweep(group, &[], &BTreeSet::new());
}

/// Cuts off whatever has a pad open, once its markers are made: the
/// re-bind, then a descriptor on its event node as the re-bind made it,
/// checked to be hidden. `client` hanging up stops the wait. Returns too
/// what the re-bind came to.
pub(crate) fn cut_off(
    pad: &Pad,
    group: u32,
    client: BorrowedFd<'_>,
) -> (Result<OwnedFd, Refusal>, Rebound) {
    let leave = Leave::Hidden {
        group,
        ordinal: pad.ordinal,
    };
    let rebound = rebind(&pad.parent, leave, Some(client));
    let opened = rebound.outcome.and_then(|()| {
        let node = pad::event_node(&pad.parent.path, pad.ordinal).ok_or(Refusal::Absent)?;
        open_under(&node, &pad.parent, Some(group))
    });
    (opened, rebound)
}

#[cfg(test)]
mod tests {
    use ctrlassist_hider::Request;
    use rustix::event::Timespec;

    use super::*;

    #[test]
    fn a_uuid_is_shaped_as_the_kernel_reads_one() {
        let uuid = uuid().expect("the kernel's random source answers");
        let groups: Vec<usize> = uuid.split('-').map(str::len).collect();
        assert_eq!(groups, [8, 4, 4, 4, 12], "{uuid}");
        assert!(
            uuid.bytes()
                .all(|byte| byte == b'-' || byte.is_ascii_hexdigit())
        );
        assert_ne!(
            uuid,
            super::uuid().expect("the kernel's random source answers")
        );
    }

    #[test]
    fn a_marker_is_a_file_in_the_marker_directory_and_never_a_path() {
        for marker in [
            "addr-0a:1b:2c:3d:4e:14",
            "port-1-2:1.0",
            "raw-3-1.4.2:1.3-0003:0000054C:00000CE6",
            "x..",
        ] {
            assert!(is_flat(marker), "{marker}");
        }
        for hostile in ["", ".", "..", "port-usb-x/input0", "../x", "/etc", "a\0b"] {
            assert!(!is_flat(hostile), "{hostile:?}");
            assert!(create(hostile).is_err(), "{hostile:?}");
        }
    }

    /// [`now`] is the monotonic clock in microseconds, as the kernel says
    /// it in seconds either side. That udev stamps a device by that clock
    /// is from its source, and not checked here.
    #[test]
    fn now_is_the_monotonic_clock_in_microseconds() {
        let before = clock_gettime(ClockId::Monotonic).tv_sec;
        let now = now();
        let after = clock_gettime(ClockId::Monotonic).tv_sec;
        let seconds = i64::try_from(now / 1_000_000).unwrap();
        assert!(
            (before..=after).contains(&seconds),
            "{before} {now} {after}"
        );
    }

    /// A device every machine has, with no driver and no `uevent` file the
    /// service may write, standing in for a pad's.
    fn stand_in() -> Device {
        Device::at(Path::new("/sys/devices/virtual/mem/null")).expect("/dev/null has a device")
    }

    #[test]
    fn a_re_bind_that_the_rule_never_acted_on_fails_once_udev_has_answered() {
        // A device that isn't there: the pad was unplugged.
        let gone = Device {
            path: PathBuf::from("/sys/devices/virtual/ctrlassist-test-no-such-device"),
            enumeration: None,
        };
        let leave = Leave::AsTheRuleSays;
        assert_eq!(
            progress(&gone, &[], 0, leave, false, false),
            Progress::Failed(Refusal::Absent)
        );
        // A device there whose inputs are still those from before: waiting
        // until udev has answered, then failed. Any directory stands in for
        // an input.
        let here = stand_in();
        let before = [PathBuf::from(env!("CARGO_MANIFEST_DIR"))];
        assert_eq!(
            progress(&here, &before, 0, leave, false, false),
            Progress::Waiting
        );
        assert_eq!(
            progress(&here, &before, 0, leave, true, false),
            Progress::Failed(Refusal::Failed)
        );
    }

    /// A device is the enumeration of it that was checked: one stamped
    /// otherwise at the same place is another plugged into the same port.
    #[test]
    fn a_device_at_the_same_place_enumerated_again_is_not_there() {
        let here = stand_in();
        assert!(here.is_there());
        let other = Device {
            enumeration: Some(1),
            ..here
        };
        assert!(!other.is_there());
    }

    fn pair() -> (OwnedFd, OwnedFd) {
        use rustix::net::{AddressFamily, SocketFlags, SocketType, socketpair};
        socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap()
    }

    fn sends(client: &OwnedFd, request: Request) {
        rustix::net::send(client, &request.encode(), rustix::net::SendFlags::empty()).unwrap();
    }

    /// Whether a client has hung up, looked at as a wait looks at it, now.
    fn has_hung_up(client: &OwnedFd) -> bool {
        use std::os::fd::AsFd;

        let mut fds = [watched(client.as_fd())];
        let now = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        poll(&mut fds, Some(&now)).unwrap() > 0 && hung_up(fds[0].revents())
    }

    /// What a wait on a client's behalf polls for sees only a hang-up,
    /// whole or of its sending alone. What it sends meanwhile, a request,
    /// an empty message or what isn't a request at all, stays queued, for
    /// `serve` to read in its turn. The wait itself, in [`rebind`], needs
    /// a device the service may re-bind, which the virtual machine check
    /// has.
    #[test]
    fn only_a_hang_up_is_seen_as_one_and_what_is_sent_meanwhile_stays_queued() {
        use std::os::fd::AsFd;

        let (service, client) = pair();
        assert!(!has_hung_up(&service));
        sends(&client, Request::Version);
        rustix::net::send(&client, &[], rustix::net::SendFlags::empty()).unwrap();
        rustix::net::send(
            &client,
            &[7, 0, 0, 0, 0, 0],
            rustix::net::SendFlags::empty(),
        )
        .unwrap();
        assert!(!has_hung_up(&service));
        let mut buffer = [0u8; 8];
        let (length, _) = ctrlassist_hider::receive(service.as_fd(), &mut buffer).unwrap();
        assert_eq!(Request::decode(&buffer[..length]), Some(Request::Version));
        let (length, _) = ctrlassist_hider::receive(service.as_fd(), &mut buffer).unwrap();
        assert_eq!(length, 0, "the empty message, in its turn");

        // Stopping sending is hanging up, as the daemon gives a hide up.
        rustix::net::shutdown(&client, rustix::net::Shutdown::Write).unwrap();
        assert!(has_hung_up(&service));
        let (service, client) = pair();
        drop(client);
        assert!(has_hung_up(&service));
    }

    /// A re-bind the service may not ask is never asked: here the device
    /// is gone, and, for a user that isn't root, as the service never is,
    /// the device's `uevent` file isn't the group's. Root may write any
    /// device's, so as root that half would ask a re-bind of the stand-in.
    #[test]
    fn a_re_bind_the_service_may_not_ask_is_not_asked() {
        let gone = Device {
            path: PathBuf::from("/sys/devices/virtual/ctrlassist-test-no-such-device"),
            enumeration: None,
        };
        let rebound = rebind(&gone, Leave::AsTheRuleSays, None);
        assert_eq!(rebound.outcome, Err(Refusal::Absent));
        assert!(!rebound.asked);
        if !rustix::process::geteuid().is_root() {
            let rebound = rebind(&stand_in(), Leave::AsTheRuleSays, None);
            assert_eq!(rebound.outcome, Err(Refusal::Failed));
            assert!(!rebound.asked);
        }
    }

    #[test]
    fn only_kernel_shaped_names_name_a_marker() {
        assert!(pad::is_address("0a:1b:2c:3d:4e:14"));
        for hostile in [
            "",
            "0A:1B:2C:3D:4E:14",
            "0a:1b:2c:3d:4e:1",
            "0a:1b:2c:3d:4e:14 ",
            "0a:1b:2c:3d:4e\",R",
            "0a-1b-2c-3d-4e-14",
            "../../../etc/passwd",
        ] {
            assert!(!pad::is_address(hostile), "{hostile:?}");
        }
        assert!(pad::is_serial("7EED821FFB17"));
        for hostile in ["", "a b", "a\"", "a\nb", "a,b", "a/b", "é", &"x".repeat(65)] {
            assert!(!pad::is_serial(hostile), "{hostile:?}");
        }
        assert!(pad::is_uniq("0a:1b:2c:3d:4e:14") && pad::is_uniq("7EED821FFB17"));
        for hostile in ["", "a/b", "..", "a b"] {
            assert!(!pad::is_uniq(hostile), "{hostile:?}");
        }
        assert!(pad::is_hid_name("0003:054C:0CE6.0017"));
        assert!(pad::is_hid_name("0005:054C:05C4.001B"));
        // Past 65,535 HID devices since boot the instance has five digits.
        assert!(pad::is_hid_name("0003:054C:0CE6.10000"));
        for hostile in [
            "",
            "..",
            "../x",
            "a/b",
            "0003:054c:0ce6.0017",
            "0003:054C:0CE6.001",
            "0003:054C:0CE6.0017/..",
            "0003.054C:0CE6:0017",
            "0003:054C:0CE6:0017.0017",
            "x y",
        ] {
            assert!(!pad::is_hid_name(hostile), "{hostile:?}");
        }
        assert!(pad::is_hid_id("0003:0000054C:00000CE6"));
        for hostile in [
            "",
            "0003:054C:0CE6",
            "0003:0000054c:00000ce6",
            "0003:0000054C:00000CE6/x",
        ] {
            assert!(!pad::is_hid_id(hostile), "{hostile:?}");
        }
    }
}
