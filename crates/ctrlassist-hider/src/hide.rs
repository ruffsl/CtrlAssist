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
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use rustix::fs::{Access, OFlags};
use rustix::io::Errno;
use rustix::rand::{GetRandomFlags, getrandom};
use rustix::time::{ClockId, clock_gettime};
use tracing::{debug, error, warn};

use crate::pad::{self, Pad};

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
fn clear() {
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

/// Whether the service may ask for the device at `parent` to be re-bound:
/// the shipped rule gives the group the `uevent` file of a pad's device
/// as it first sees the pad. Where the group can't write it, the rule
/// isn't installed, or didn't take the device for a pad's.
pub(crate) fn may_request(parent: &Path) -> Result<(), Refusal> {
    let uevent = parent.join("uevent");
    rustix::fs::access(&uevent, Access::WRITE_OK).map_err(|error| {
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
    /// Failed, and whether the pad went and came back all the same.
    Failed(Refusal, bool),
}

/// Who asked for a re-bind, and so what else ends the wait for it.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Asker<'a> {
    /// Nobody waits for it now: it ends on state alone.
    Nobody,
    /// A client whose hanging up ends it.
    HangsUp(BorrowedFd<'a>),
    /// A client whose hanging up ends it, and so does anything it sends
    /// meanwhile, as `cancel` is: that is looked for before the re-bind
    /// is asked as well, and then it isn't.
    Cancels(BorrowedFd<'a>),
}

impl<'a> Asker<'a> {
    /// The client to watch, and what of it ends the wait.
    fn watch(self) -> Option<(BorrowedFd<'a>, PollFlags)> {
        match self {
            Self::Nobody => None,
            Self::HangsUp(client) => Some((client, PollFlags::RDHUP)),
            Self::Cancels(client) => Some((client, PollFlags::IN | PollFlags::RDHUP)),
        }
    }

    /// Whether the client has hung up, or sent what cancels, already.
    fn has_cancelled(self) -> bool {
        let Self::Cancels(client) = self else {
            return false;
        };
        let mut fds = [PollFd::from_borrowed_fd(
            client,
            PollFlags::IN | PollFlags::RDHUP,
        )];
        // Looked at as it is now, not waited on.
        let now = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        matches!(poll(&mut fds, Some(&now)), Ok(ready) if ready > 0)
    }
}

/// What a re-bind came to.
#[derive(Debug)]
pub(crate) struct Rebound {
    pub(crate) outcome: Result<(), Refusal>,
    /// Whether udev was asked for it.
    pub(crate) asked: bool,
    /// Whether the pad goes and comes back for it, or is still to: it was
    /// asked, and wasn't seen to end with the pad unplugged, the rule not
    /// acting, or no driver taking the pad.
    pub(crate) went: bool,
}

impl Rebound {
    fn not_asked(refusal: Refusal) -> Self {
        Self {
            outcome: Err(refusal),
            asked: false,
            went: false,
        }
    }
}

/// Where the re-bind of the device at `parent` has got to. `before` is
/// its inputs before it was asked for, `since` when it was asked for, and
/// `answered` whether udev has said it acted on the request.
fn progress(
    parent: &Path,
    before: &[PathBuf],
    since: u64,
    leave: Leave,
    answered: bool,
) -> Progress {
    let device = parent.display();
    if !parent.exists() {
        debug!(%device, "the pad was unplugged as its driver was re-bound");
        return Progress::Failed(Refusal::Absent, false);
    }
    // The rule unbinds the driver and probes again before udev says it
    // has acted on the request, so after that the inputs that were there
    // are gone, and a driver is bound if any will be.
    if before.iter().any(|input| input.exists()) {
        if answered {
            error!(
                %device,
                "udev acted on the request to re-bind the pad's driver without re-binding it: the shipped rule isn't installed, or isn't the one shipped"
            );
            return Progress::Failed(Refusal::Failed, false);
        }
        return Progress::Waiting;
    }
    if !pad::has_driver(parent) {
        if answered {
            warn!(%device, "no driver took the pad again after its driver was re-bound");
            return Progress::Failed(Refusal::Failed, false);
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
                    %device,
                    node = %node.devnode().unwrap_or(node.syspath()).display(),
                    wrong,
                    "a node of the pad wasn't hidden when made again: another rule undoes the shipped one, or it isn't installed"
                );
                return Progress::Failed(Refusal::Failed, true);
            }
        }
        // A driver may make its pad's input later than its probe, as one
        // for a wireless receiver does once the pad reports in.
        if pad::event_node(parent, ordinal).is_none() {
            return Progress::Waiting;
        }
    }
    Progress::Done
}

/// Has udev re-bind the driver of the device at `parent`, and waits until
/// it has finished with the nodes that makes. Gives up if the pad is
/// unplugged, or if `asker` goes or cancels meanwhile; one that has
/// cancelled already has nothing re-bound.
///
/// It waits on state, not on particular events: at every event udev sends
/// for a pad's devices, and after the monitor's socket overran and lost
/// some, it reads afresh what sysfs and udev's record say. The re-bind is
/// done when the inputs there before are gone, a driver is bound, and
/// udev has finished with every node under the device since it was asked,
/// each hidden if it is to be. It fails at once when the rule's effect is
/// missing: udev has acted on the request and the driver is as it was, no
/// driver took the pad again, or a node came back not hidden.
pub(crate) fn rebind(parent: &Path, leave: Leave, asker: Asker<'_>) -> Rebound {
    if let Err(refusal) = may_request(parent) {
        return Rebound::not_asked(refusal);
    }
    let before = pad::inputs_under(parent);
    let device = parent.display();
    // Listening from before the request, so nothing it causes is missed.
    let monitor = udev::MonitorBuilder::new()
        .and_then(|monitor| monitor.match_subsystem("hid"))
        .and_then(|monitor| monitor.match_subsystem("usb"))
        .and_then(|monitor| monitor.match_subsystem("input"))
        .and_then(|monitor| monitor.match_subsystem("hidraw"))
        .and_then(udev::MonitorBuilder::listen);
    let monitor = match monitor {
        Ok(monitor) => monitor,
        Err(error) => {
            warn!(%error, "couldn't listen to udev");
            return Rebound::not_asked(Refusal::Failed);
        }
    };
    if asker.has_cancelled() {
        debug!(%device, "the client cancelled before the pad's driver was re-bound");
        return Rebound::not_asked(Refusal::Cancelled);
    }
    let since = now();
    let uuid = match request(parent) {
        Ok(uuid) => uuid,
        Err(error) => {
            warn!(%device, %error, "couldn't ask udev to re-bind the pad's driver");
            return Rebound::not_asked(Refusal::Failed);
        }
    };
    // Asked: from here on the pad goes and comes back for it, unless the
    // re-bind is seen to end otherwise.
    let ended = |refusal| Rebound {
        outcome: Err(refusal),
        asked: true,
        went: true,
    };
    let mut answered = false;
    loop {
        let mut fds = vec![PollFd::new(&monitor, PollFlags::IN)];
        if let Some((client, flags)) = asker.watch() {
            fds.push(PollFd::from_borrowed_fd(client, flags));
        }
        match poll(&mut fds, None) {
            Ok(_) | Err(Errno::INTR) => {}
            Err(error) => {
                warn!(%device, %error, "couldn't wait on udev for the pad's driver to be re-bound");
                return ended(Refusal::Failed);
            }
        }
        if fds.get(1).is_some_and(|fd| !fd.revents().is_empty()) {
            return match asker {
                Asker::Cancels(_) => {
                    debug!(%device, "the client cancelled, or hung up, as the pad's driver was re-bound");
                    ended(Refusal::Cancelled)
                }
                Asker::HangsUp(_) | Asker::Nobody => {
                    debug!(%device, "the client hung up as the pad's driver was re-bound");
                    ended(Refusal::Failed)
                }
            };
        }
        drop(fds);
        // Each event is only a reason to look again. One lost to an
        // overrun ends the iteration early, and is a reason too.
        for event in monitor.iter() {
            answered |= event.event_type() == udev::EventType::Change
                && event.syspath() == parent
                && event
                    .property_value("SYNTH_UUID")
                    .and_then(|value| value.to_str())
                    == Some(uuid.as_str());
        }
        match progress(parent, &before, since, leave, answered) {
            Progress::Waiting => {}
            Progress::Done => {
                return Rebound {
                    outcome: Ok(()),
                    asked: true,
                    went: true,
                };
            }
            Progress::Failed(refusal, went) => {
                return Rebound {
                    outcome: Err(refusal),
                    asked: true,
                    went,
                };
            }
        }
    }
}

/// Opens a pad's node for a client, for reading and writing, if it is
/// still a node under `parent`: a node found by its path could have gone,
/// and its number been given to another device's, before it was opened.
/// With `hidden`, the node must also be as the rule hides a node for that
/// group.
pub(crate) fn open_under(
    node: &Path,
    parent: &Path,
    hidden: Option<u32>,
) -> Result<OwnedFd, Refusal> {
    let fd: OwnedFd = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags((OFlags::NONBLOCK | OFlags::CLOEXEC).bits() as i32)
        .open(node)
        .map_err(|error| {
            warn!(node = %node.display(), %error, "couldn't open a pad's node");
            Refusal::Failed
        })?
        .into();
    let stat = rustix::fs::fstat(&fd).map_err(|error| {
        warn!(node = %node.display(), %error, "couldn't look at a pad's node once open");
        Refusal::Failed
    })?;
    let opened = pad::device_of(udev::DeviceType::Character, stat.st_rdev)
        .filter(|device| device.syspath().starts_with(parent));
    let Some(opened) = opened else {
        warn!(
            node = %node.display(),
            device = %parent.display(),
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
        if let Some(wrong) = pad::unhidden(ownership, tags, group) {
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

/// Shows again each pad the rule hides that nothing should: one that no
/// pad in `held` is and that no marker in `markers` names, as a pad
/// sharing an address with one that was hidden, or plugged into its port
/// while it was away. Doesn't wait for udev.
pub(crate) fn sweep(group: u32, held: &[&Path], markers: &BTreeSet<String>) {
    for pad in pad::hidden(group) {
        let named = pad.markers.intersection(markers).next().is_some();
        if !named
            && !held.contains(&pad.parent.as_path())
            && let Err(error) = request(&pad.parent)
        {
            warn!(
                device = %pad.parent.display(),
                %error,
                "couldn't ask for a pad no marker names to be shown again; it stays hidden until it is plugged in again"
            );
        }
    }
}

/// Undoes whatever an earlier service left hidden: run when no connection
/// can still own anything. Every marker goes, then every pad the rule
/// still hides is shown again.
pub(crate) fn reconcile(group: u32) {
    clear();
    sweep(group, &[], &BTreeSet::new());
}

/// Cuts off whatever has a pad open, once its markers are made: the
/// re-bind, then a descriptor on its event node as the re-bind made it,
/// checked to be hidden. `client` hanging up, or sending anything, stops
/// the wait. Returns too what the re-bind came to.
pub(crate) fn cut_off(
    pad: &Pad,
    group: u32,
    client: BorrowedFd<'_>,
) -> (Result<OwnedFd, Refusal>, Rebound) {
    let leave = Leave::Hidden {
        group,
        ordinal: pad.ordinal,
    };
    let rebound = rebind(&pad.parent, leave, Asker::Cancels(client));
    let opened = rebound.outcome.and_then(|()| {
        let node = pad::event_node(&pad.parent, pad.ordinal).ok_or(Refusal::Absent)?;
        open_under(&node, &pad.parent, Some(group))
    });
    (opened, rebound)
}

#[cfg(test)]
mod tests {
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
            "raw-3-1.4.2:1.3",
            "x..",
        ] {
            assert!(is_flat(marker), "{marker}");
        }
        for hostile in ["", ".", "..", "port-usb-x/input0", "../x", "/etc", "a\0b"] {
            assert!(!is_flat(hostile), "{hostile:?}");
            assert!(create(hostile).is_err(), "{hostile:?}");
        }
    }

    #[test]
    fn udev_s_clock_is_the_monotonic_one_in_microseconds() {
        let first = now();
        let second = now();
        assert!(first > 0 && second >= first);
    }

    #[test]
    fn a_re_bind_that_the_rule_never_acted_on_fails_once_udev_has_answered() {
        // A device that isn't there: the pad was unplugged.
        let gone = Path::new("/sys/devices/virtual/ctrlassist-test-no-such-device");
        let leave = Leave::AsTheRuleSays;
        // Neither of these makes the pad go and come back.
        assert_eq!(
            progress(gone, &[], 0, leave, false),
            Progress::Failed(Refusal::Absent, false)
        );
        // A device there whose inputs are still those from before: waiting
        // until udev has answered, then failed. Any directory stands in.
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let before = [here.join("src")];
        assert_eq!(progress(here, &before, 0, leave, false), Progress::Waiting);
        assert_eq!(
            progress(here, &before, 0, leave, true),
            Progress::Failed(Refusal::Failed, false)
        );
    }

    /// A client that has sent something, as a `cancel`, or hung up, has
    /// cancelled; one that has sent nothing hasn't, and one that may only
    /// hang up never cancels.
    #[test]
    fn a_client_has_cancelled_once_it_sends_anything_or_hangs_up() {
        use std::os::fd::AsFd;

        use rustix::net::{AddressFamily, SocketFlags, SocketType, socketpair};
        let pair = || {
            socketpair(
                AddressFamily::UNIX,
                SocketType::SEQPACKET,
                SocketFlags::CLOEXEC,
                None,
            )
            .unwrap()
        };
        let (service, client) = pair();
        let cancels = Asker::Cancels(service.as_fd());
        assert!(!cancels.has_cancelled());
        rustix::net::send(&client, &[7, 0, 0, 0, 0], rustix::net::SendFlags::empty()).unwrap();
        assert!(cancels.has_cancelled());
        assert!(!Asker::HangsUp(service.as_fd()).has_cancelled());
        assert!(!Asker::Nobody.has_cancelled());
        let (service, client) = pair();
        drop(client);
        assert!(Asker::Cancels(service.as_fd()).has_cancelled());
    }

    /// A re-bind that is never asked is never one the pad goes and comes
    /// back for: here the device's `uevent` file isn't the group's.
    #[test]
    fn a_re_bind_never_asked_is_not_counted() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let rebound = rebind(here, Leave::AsTheRuleSays, Asker::Nobody);
        assert_eq!(rebound.outcome, Err(Refusal::Failed));
        assert!(!rebound.went);
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
