//! Hiding a pad and putting it back.
//!
//! Hiding is two steps. A runtime udev rule denies the pad's nodes to
//! everyone; then the pad's driver is unbound and bound again, which cuts
//! off whatever already had the pad open and recreates its nodes already
//! denied. Restoring removes the rule and re-binds once more, so the
//! nodes come back as they were and programs see the pad arrive.
//!
//! Everything written before the rule is recorded on disk first, so that
//! a service that dies can be cleaned up after by the next one.

use std::fmt::Write as _;
use std::fs;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use ctrlassist_hider::Refusal;
use rustix::fs::OFlags;

use crate::pad::{self, Identity, Pad, Rebind};

/// Where udev reads runtime rules from. Emptied at boot.
const RULES: &str = "/run/udev/rules.d";
/// Where the service records what it has hidden. Emptied at boot.
const RECORDS: &str = "/run/ctrlassist/hidden";

/// A node's mode and owner before hiding, for a pad with no driver to
/// re-bind: removing the rule doesn't put a node's mode back.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Saved {
    path: PathBuf,
    mode: u32,
    uid: u32,
    gid: u32,
}

/// A pad the service has hidden.
///
/// It holds what the pad is, never where it is: a pad that reconnects
/// while hidden comes back under new kernel names, so its nodes and its
/// driver binding are looked up each time they are needed.
#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) token: u32,
    pub(crate) identity: Identity,
    saved: Vec<Saved>,
}

fn rule_path(token: u32) -> PathBuf {
    // Sorted after systemd's rule that grants seat access (70) and before
    // the one that applies it (73); earlier, a returning pad is granted
    // access again.
    Path::new(RULES).join(format!("72-ctrlassist-{token}.rules"))
}

fn record_path(token: u32) -> PathBuf {
    Path::new(RECORDS).join(token.to_string())
}

/// The rule that denies one pad's nodes. Built only from numbers the
/// service parsed and strings `pad` has already checked.
fn rule_text(identity: &Identity) -> String {
    const DENY: &str = r#"MODE="0000", GROUP="root", TAG-="uaccess""#;
    let mut input = format!(
        r#"SUBSYSTEM=="input", ATTRS{{id/vendor}}=="{:04x}", ATTRS{{id/product}}=="{:04x}""#,
        identity.vendor, identity.product
    );
    if let Some(uniq) = &identity.uniq {
        let _ = write!(input, r#", ATTRS{{uniq}}=="{uniq}""#);
    } else if let Some(serial) = &identity.serial {
        let _ = write!(input, r#", ENV{{ID_SERIAL_SHORT}}=="{serial}""#);
    }
    let mut rule = format!(
        "# Written by the CtrlAssist hiding service; removed when the pad is restored.\n{input}, {DENY}\n"
    );
    if identity.hid {
        // A hidraw node's parent is the HID device, named for its bus,
        // vendor and product; its address is a property of that parent.
        let mut hidraw = format!(
            r#"SUBSYSTEM=="hidraw", KERNELS=="{:04X}:{:04X}:{:04X}.*""#,
            identity.bus, identity.vendor, identity.product
        );
        if let Some(uniq) = &identity.uniq {
            let _ = write!(
                hidraw,
                r#", IMPORT{{parent}}="HID_UNIQ", ENV{{HID_UNIQ}}=="{uniq}""#
            );
        }
        let _ = writeln!(rule, "{hidraw}, {DENY}");
    }
    rule
}

fn udevadm(arguments: &[&str]) -> io::Result<()> {
    let status = Command::new("udevadm").args(arguments).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "udevadm {arguments:?} failed: {status}"
        )))
    }
}

/// Makes udev re-read its rules now, not at its next periodic check.
fn reload() -> io::Result<()> {
    udevadm(&["control", "--reload"])
}

/// Unbinds and binds the pad's driver, then waits for udev to finish with
/// the nodes that creates.
fn rebind(target: &Rebind) -> io::Result<()> {
    let driver = Path::new("/sys/bus")
        .join(target.bus)
        .join("drivers")
        .join(&target.driver);
    // Unbinding a device that is already unbound fails; that is fine, the
    // bind is what must succeed.
    let _ = fs::write(driver.join("unbind"), &target.device);
    fs::write(driver.join("bind"), &target.device)?;
    udevadm(&["settle"])
}

/// Makes udev apply the current rules to nodes that already exist.
fn retrigger(nodes: &[PathBuf]) -> io::Result<()> {
    let mut arguments = vec!["trigger", "--action=change", "--settle"];
    let names: Vec<String> = nodes
        .iter()
        .map(|node| format!("--name-match={}", node.display()))
        .collect();
    arguments.extend(names.iter().map(String::as_str));
    udevadm(&arguments)
}

fn open_event(identity: &Identity) -> Result<OwnedFd, Refusal> {
    let nodes = pad::nodes(identity);
    let event = nodes.first().ok_or(Refusal::Absent)?;
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags((OFlags::NONBLOCK | OFlags::CLOEXEC).bits() as i32)
        .open(event)
        .map(OwnedFd::from)
        .map_err(|_| Refusal::Failed)
}

/// What a later service needs to undo an entry, one fact per line.
fn record(identity: &Identity, saved: &[Saved]) -> String {
    let mut record = format!(
        "pad {:04x} {:04x} {:04x} {} {} {}\n",
        identity.bus,
        identity.vendor,
        identity.product,
        u8::from(identity.hid),
        identity.uniq.as_deref().unwrap_or("-"),
        identity.serial.as_deref().unwrap_or("-"),
    );
    for saved in saved {
        let _ = writeln!(
            record,
            "node {:o} {} {} {}",
            saved.mode,
            saved.uid,
            saved.gid,
            saved.path.display()
        );
    }
    record
}

/// A record's contents, if every field passes the checks that were
/// applied before it was written. A record is read as warily as a device.
fn parse_record(text: &str) -> Option<(Identity, Vec<Saved>)> {
    let mut identity = None;
    let mut saved = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split(' ').collect();
        match fields.as_slice() {
            ["pad", bus, vendor, product, hid @ ("0" | "1"), uniq, serial] => {
                let optional = |text: &str, ok: fn(&str) -> bool| match text {
                    "-" => Some(None),
                    text => ok(text).then(|| Some(text.to_string())),
                };
                identity = Some(Identity {
                    bus: u16::from_str_radix(bus, 16).ok()?,
                    vendor: u16::from_str_radix(vendor, 16).ok()?,
                    product: u16::from_str_radix(product, 16).ok()?,
                    uniq: optional(uniq, pad::is_address)?,
                    serial: optional(serial, pad::is_serial)?,
                    hid: *hid == "1",
                });
            }
            ["node", mode, uid, gid, path] => saved.push(Saved {
                path: recorded_node(path)?,
                mode: u32::from_str_radix(mode, 8).ok()? & 0o7777,
                uid: uid.parse().ok()?,
                gid: gid.parse().ok()?,
            }),
            _ => return None,
        }
    }
    Some((identity?, saved))
}

/// A record line's node path, if it is one this service could have written.
fn recorded_node(text: &str) -> Option<PathBuf> {
    let name = text
        .strip_prefix("/dev/input/")
        .or_else(|| text.strip_prefix("/dev/"))?;
    let ok = !name.is_empty()
        && name.len() <= 32
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric());
    ok.then(|| PathBuf::from(text))
}

/// A node's kind: its name without its number, such as `event` or `js`.
fn kind(node: &Path) -> &str {
    node.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("")
        .trim_end_matches(|character: char| character.is_ascii_digit())
}

impl Entry {
    /// Opens the pad again after it has reconnected.
    pub(crate) fn reopen(&self) -> Result<OwnedFd, Refusal> {
        open_event(&self.identity)
    }

    /// Puts the pad back. Carries on past failures: each step that can
    /// still be undone is.
    pub(crate) fn restore(self) {
        let _ = fs::remove_file(rule_path(self.token));
        let _ = reload();
        undo(&self.identity, &self.saved);
        let _ = fs::remove_file(record_path(self.token));
    }
}

/// Brings a pad's nodes back once its rule is gone. A pad that isn't
/// connected needs nothing: it returns to rules that no longer name it.
fn undo(identity: &Identity, saved: &[Saved]) {
    let nodes = pad::nodes(identity);
    if nodes.is_empty() {
        return;
    }
    if let Some(target) = pad::rebind(identity) {
        let _ = rebind(&target);
        return;
    }
    let _ = retrigger(&nodes);
    for node in &nodes {
        // The node as it was; or, for a pad that has reconnected under new
        // names, as its node of the same kind was.
        let was = saved
            .iter()
            .find(|saved| saved.path == *node)
            .or_else(|| saved.iter().find(|saved| kind(&saved.path) == kind(node)));
        if let Some(was) = was {
            let _ = fs::set_permissions(node, fs::Permissions::from_mode(was.mode));
            let _ = std::os::unix::fs::chown(node, Some(was.uid), Some(was.gid));
        }
    }
}

/// Hides a pad. Returns its entry, a descriptor on its event node, and
/// whether whatever had it open was cut off.
pub(crate) fn hide(token: u32, pad: Pad) -> Result<(Entry, OwnedFd, bool), Refusal> {
    let Pad {
        identity,
        rebind: target,
    } = pad;
    let nodes = pad::nodes(&identity);
    if nodes.is_empty() {
        return Err(Refusal::Absent);
    }
    // Only a pad with no driver to re-bind keeps its nodes, so only its
    // nodes need their modes remembered.
    let saved = if target.is_some() {
        Vec::new()
    } else {
        nodes
            .iter()
            .filter_map(|path| {
                let metadata = fs::metadata(path).ok()?;
                Some(Saved {
                    path: path.clone(),
                    mode: metadata.mode() & 0o7777,
                    uid: metadata.uid(),
                    gid: metadata.gid(),
                })
            })
            .collect()
    };
    let entry = Entry {
        token,
        identity,
        saved,
    };

    let attempt = || -> io::Result<bool> {
        fs::create_dir_all(RECORDS)?;
        fs::create_dir_all(RULES)?;
        // The record first: from here on a crash leaves enough to undo.
        fs::write(record_path(token), record(&entry.identity, &entry.saved))?;
        fs::write(rule_path(token), rule_text(&entry.identity))?;
        reload()?;
        match &target {
            Some(target) => rebind(target).map(|()| true),
            None => retrigger(&nodes).map(|()| false),
        }
    };
    match attempt() {
        Ok(dislodged) => match open_event(&entry.identity) {
            Ok(event) => Ok((entry, event, dislodged)),
            Err(refusal) => {
                entry.restore();
                Err(refusal)
            }
        },
        Err(_) => {
            entry.restore();
            Err(Refusal::Failed)
        }
    }
}

/// Undoes whatever an earlier service left hidden. Run when no connection
/// can still own anything. A record that doesn't parse still has its rule
/// removed: the rule is named by the record's own number.
pub(crate) fn reconcile() {
    let Ok(records) = fs::read_dir(RECORDS) else {
        return;
    };
    let mut pending = Vec::new();
    for record in records.flatten() {
        let Some(token) = record
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let _ = fs::remove_file(rule_path(token));
        let parsed = parse_record(&fs::read_to_string(record.path()).unwrap_or_default());
        pending.push((record.path(), parsed));
    }
    if pending.is_empty() {
        return;
    }
    let _ = reload();
    for (record, parsed) in pending {
        if let Some((identity, saved)) = parsed {
            undo(&identity, &saved);
        }
        let _ = fs::remove_file(record);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> Identity {
        Identity {
            bus: 3,
            vendor: 0x054c,
            product: 0x0ce6,
            uniq: Some("0c:27:56:4f:bd:14".to_string()),
            serial: None,
            hid: true,
        }
    }

    #[test]
    fn a_rule_names_a_pad_by_numbers_and_its_checked_address() {
        let rule = rule_text(&identity());
        let lines: Vec<&str> = rule.lines().collect();
        assert_eq!(
            lines[1],
            r#"SUBSYSTEM=="input", ATTRS{id/vendor}=="054c", ATTRS{id/product}=="0ce6", ATTRS{uniq}=="0c:27:56:4f:bd:14", MODE="0000", GROUP="root", TAG-="uaccess""#
        );
        assert_eq!(
            lines[2],
            r#"SUBSYSTEM=="hidraw", KERNELS=="0003:054C:0CE6.*", IMPORT{parent}="HID_UNIQ", ENV{HID_UNIQ}=="0c:27:56:4f:bd:14", MODE="0000", GROUP="root", TAG-="uaccess""#
        );
    }

    #[test]
    fn a_pad_without_an_address_is_named_by_its_serial_or_its_model_alone() {
        let serial = Identity {
            uniq: None,
            serial: Some("7EED821FFB17".to_string()),
            hid: false,
            ..identity()
        };
        let rule = rule_text(&serial);
        assert!(
            rule.contains(r#"ENV{ID_SERIAL_SHORT}=="7EED821FFB17""#),
            "{rule}"
        );
        assert!(
            !rule.contains("hidraw"),
            "a pad with no HID device has no hidraw rule: {rule}"
        );

        let bare = Identity {
            serial: None,
            ..serial
        };
        assert!(!rule_text(&bare).contains("ENV{"));
    }

    #[test]
    fn only_a_well_formed_address_or_serial_is_accepted() {
        assert!(pad::is_address("0c:27:56:4f:bd:14"));
        for hostile in [
            "",
            "0C:27:56:4F:BD:14",
            "0c:27:56:4f:bd:1",
            "0c:27:56:4f:bd:14 ",
            "0c:27:56:4f:bd\",R",
            "0c-27-56-4f-bd-14",
        ] {
            assert!(!pad::is_address(hostile), "{hostile:?}");
        }
        assert!(pad::is_serial("7EED821FFB17"));
        for hostile in ["", "a b", "a\"", "a\nb", "a,b", "é", &"x".repeat(65)] {
            assert!(!pad::is_serial(hostile), "{hostile:?}");
        }
    }

    #[test]
    fn only_kernel_shaped_names_reach_a_sysfs_path() {
        assert!(pad::is_hid_name("0003:054C:0CE6.0017"));
        assert!(pad::is_hid_name("0005:054C:05C4.001B"));
        for interface in ["1-3:1.0", "3-1.4.2:1.0", "12-10:2.11"] {
            assert!(pad::is_usb_interface(interface), "{interface:?}");
        }
        assert!(pad::is_driver("hid-generic"));
        assert!(pad::is_driver("playstation"));
        for hostile in [
            "",
            "..",
            "../x",
            "a/b",
            "0003:054c:0ce6.0017",
            "0003:054C:0CE6.001",
            "0003:054C:0CE6.00170",
            "0003.054C:0CE6:0017",
            "x y",
        ] {
            assert!(!pad::is_hid_name(hostile), "{hostile:?}");
        }
        for hostile in [
            "",
            "..",
            ":",
            "../x",
            "a/b",
            "1-3:1.0/..",
            "usb1",
            "1-3",
            "1-3:",
            ":1.0",
            "1--3:1.0",
            "1-3:1..0",
            "1-3:1.0:1.0",
        ] {
            assert!(!pad::is_usb_interface(hostile), "{hostile:?}");
        }
        for hostile in ["", "../x", "a/b", "Playstation", "a b"] {
            assert!(!pad::is_driver(hostile), "{hostile:?}");
        }
    }

    #[test]
    fn a_record_reads_back_as_it_was_written_and_not_at_all_if_tampered_with() {
        let saved = vec![Saved {
            path: PathBuf::from("/dev/input/event30"),
            mode: 0o660,
            uid: 0,
            gid: 174,
        }];
        let text = record(&identity(), &saved);
        assert_eq!(parse_record(&text), Some((identity(), saved)));

        let bare = Identity {
            uniq: None,
            serial: Some("7EED821FFB17".to_string()),
            hid: false,
            ..identity()
        };
        assert_eq!(parse_record(&record(&bare, &[])), Some((bare, Vec::new())));

        for tampered in [
            "",
            "node 660 0 174 /dev/input/event30\n",
            "pad 0003 054c 0ce6 1 not-an-address -\n",
            "pad 0003 054c 0ce6 1 - a\"b\n",
            "pad 0003 054c 0ce6 2 - -\n",
            "pad 0003 054c 0ce6 1 - -\nnode 660 0 174 /etc/shadow\n",
            "pad 0003 054c 0ce6 1 - -\nrun /bin/sh\n",
        ] {
            assert_eq!(parse_record(tampered), None, "{tampered:?}");
        }
    }

    #[test]
    fn a_node_s_kind_is_its_name_without_its_number() {
        assert_eq!(kind(Path::new("/dev/input/event30")), "event");
        assert_eq!(kind(Path::new("/dev/input/js1")), "js");
        assert_eq!(kind(Path::new("/dev/hidraw16")), "hidraw");
    }

    #[test]
    fn a_recorded_node_must_be_a_plain_device_name() {
        assert_eq!(
            recorded_node("/dev/input/event30"),
            Some(PathBuf::from("/dev/input/event30"))
        );
        assert_eq!(
            recorded_node("/dev/hidraw16"),
            Some(PathBuf::from("/dev/hidraw16"))
        );
        for hostile in [
            "/etc/shadow",
            "/dev/input/../sda",
            "/dev/input/",
            "/dev/",
            "event30",
            "/dev/input/a b",
        ] {
            assert_eq!(recorded_node(hostile), None, "{hostile:?}");
        }
    }
}
