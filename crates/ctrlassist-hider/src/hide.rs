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
use rustix::fs::OFlags;
use rustix::rand::{GetRandomFlags, getrandom};

use crate::pad::{self, Pad};

/// Where the markers are, as the shipped rule names it. The unit's
/// runtime directory, kept when the service stops.
const MARKERS: &str = "/run/ctrlassist/hide";

fn marker_path(marker: &str) -> PathBuf {
    Path::new(MARKERS).join(marker)
}

/// Creates a marker. One that names a port has a directory for its port,
/// and a file for its interface in it.
pub(crate) fn create(marker: &str) -> io::Result<()> {
    let path = marker_path(marker);
    if let Some(parent) = path.parent().filter(|parent| *parent != Path::new(MARKERS)) {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, b"")
}

/// Deletes a marker, and its port's directory once that is empty.
pub(crate) fn delete(marker: &str) {
    let path = marker_path(marker);
    let _ = fs::remove_file(&path);
    if let Some(parent) = path.parent().filter(|parent| *parent != Path::new(MARKERS)) {
        let _ = fs::remove_dir(parent);
    }
}

/// Every marker there is, by name.
fn markers() -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = fs::read_dir(MARKERS) else {
        return found;
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            for inner in fs::read_dir(entry.path()).into_iter().flatten().flatten() {
                if let Some(inner) = inner.file_name().to_str() {
                    found.push(format!("{name}/{inner}"));
                }
            }
        } else {
            found.push(name);
        }
    }
    found
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

/// Has udev re-bind the driver of the device at `parent`, and waits until
/// it has finished with the event nodes that makes. Gives up if `client`
/// hangs up meanwhile.
///
/// It waits on udev's own events, in the order udev sends them: the change
/// it was asked for, broadcast once the rule has unbound and probed the
/// driver; then the arrival of each event node the probe made, broadcast
/// once the rule has hidden it or not. A probe that bound no driver, or
/// made no event node, is a failure found at the first.
pub(crate) fn rebind(parent: &Path, client: Option<BorrowedFd<'_>>) -> Result<(), Refusal> {
    // Listening from before the request, so nothing it causes is missed.
    let monitor = udev::MonitorBuilder::new()
        .and_then(|monitor| monitor.match_subsystem("hid"))
        .and_then(|monitor| monitor.match_subsystem("usb"))
        .and_then(|monitor| monitor.match_subsystem("input"))
        .and_then(udev::MonitorBuilder::listen)
        .map_err(|_| Refusal::Failed)?;
    let uuid = request(parent).map_err(|_| Refusal::Failed)?;
    let mut pending: Option<BTreeSet<PathBuf>> = None;
    loop {
        let mut fds = vec![PollFd::new(&monitor, PollFlags::IN)];
        if let Some(client) = client {
            fds.push(PollFd::from_borrowed_fd(client, PollFlags::RDHUP));
        }
        if poll(&mut fds, None).is_err() {
            continue;
        }
        if fds.get(1).is_some_and(|fd| !fd.revents().is_empty()) {
            return Err(Refusal::Failed);
        }
        drop(fds);
        for event in monitor.iter() {
            match &mut pending {
                None => {
                    let ours = event.event_type() == udev::EventType::Change
                        && event.syspath() == parent
                        && event
                            .property_value("SYNTH_UUID")
                            .and_then(|value| value.to_str())
                            == Some(uuid.as_str());
                    if !ours {
                        continue;
                    }
                    let made = pad::event_devices_under(parent);
                    if made.is_empty() || !pad::has_driver(parent) {
                        return Err(Refusal::Failed);
                    }
                    pending = Some(made);
                }
                Some(made) => {
                    if event.event_type() == udev::EventType::Add {
                        made.remove(event.syspath());
                        if made.is_empty() {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}

/// Opens a pad's node for a client, for reading and writing.
pub(crate) fn open(node: &Path) -> Result<OwnedFd, Refusal> {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags((OFlags::NONBLOCK | OFlags::CLOEXEC).bits() as i32)
        .open(node)
        .map(OwnedFd::from)
        .map_err(|_| Refusal::Failed)
}

/// Shows again each pad the rule hides that nothing should: one that no
/// pad in `held` is and that no marker in `markers` names, as a pad
/// sharing an address with one that was hidden, or plugged into its port
/// while it was away. Doesn't wait for udev.
pub(crate) fn sweep(group: u32, held: &[&Path], markers: &BTreeSet<String>) {
    for pad in pad::hidden(group) {
        let named = pad.markers.intersection(markers).next().is_some();
        if !named && !held.contains(&pad.parent.as_path()) {
            let _ = request(&pad.parent);
        }
    }
}

/// Undoes whatever an earlier service left hidden: run when no connection
/// can still own anything. Every marker goes, then every pad the rule
/// still hides is shown again.
pub(crate) fn reconcile(group: u32) {
    for marker in markers() {
        delete(&marker);
    }
    sweep(group, &[], &BTreeSet::new());
}

/// Cuts off whatever has a pad open, once its markers are made: the
/// re-bind, then a descriptor on its event node as the re-bind made it.
/// `client` hanging up stops the wait.
pub(crate) fn cut_off(pad: &Pad, client: BorrowedFd<'_>) -> Result<OwnedFd, Refusal> {
    rebind(&pad.parent, Some(client))?;
    let node = pad::event_node(&pad.parent, pad.ordinal).ok_or(Refusal::Absent)?;
    open(&node)
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
    fn only_kernel_shaped_names_name_a_marker() {
        assert!(pad::is_address("0c:27:56:4f:bd:14"));
        for hostile in [
            "",
            "0C:27:56:4F:BD:14",
            "0c:27:56:4f:bd:1",
            "0c:27:56:4f:bd:14 ",
            "0c:27:56:4f:bd\",R",
            "0c-27-56-4f-bd-14",
            "../../../etc/passwd",
        ] {
            assert!(!pad::is_address(hostile), "{hostile:?}");
        }
        assert!(pad::is_serial("7EED821FFB17"));
        for hostile in ["", "a b", "a\"", "a\nb", "a,b", "a/b", "é", &"x".repeat(65)] {
            assert!(!pad::is_serial(hostile), "{hostile:?}");
        }
        assert!(pad::is_uniq("0c:27:56:4f:bd:14") && pad::is_uniq("7EED821FFB17"));
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
        for port in [
            "usb-0000:00:14.0-2/input3",
            "usb-0000:00:14.0-1.4.2/input0",
            "usb-dummy_hcd.0-1/input0",
        ] {
            assert!(pad::is_port(port), "{port:?}");
        }
        for hostile in [
            "",
            "usb-0000:00:14.0-2",
            "usb-0000:00:14.0-2/input",
            "usb-0000:00:14.0-2/input3/x",
            "usb-../input0",
            "../../etc/input0",
            "usb-a b/input0",
            "02:ca:7e:00:00:01",
            "usb-x/input1234",
        ] {
            assert!(!pad::is_port(hostile), "{hostile:?}");
        }
    }
}
