//! The diagnostic bundle the About pane's Diagnostics button puts on
//! the clipboard, for pasting into a bug report: JSON, so it can be
//! read by a person and by a script.
//!
//! It holds what a report usually has to be asked for: the releases of
//! the parts, the desktop session, facts of the kernel and of input on
//! this machine, the known pads, the running setups and the profiles'
//! text. A pad's address is masked throughout: its vendor half stays,
//! which tells a maker's pad from a clone, and the half that names the
//! one unit is replaced, with a number so two pads of one maker can
//! still be told apart. No file path goes in: one under the home
//! directory names the user, so the daemon's section says only whether
//! the daemon named the directory it keeps the user's files in.

use ctrlassist::{DaemonState, Pad, Profile, Setup};
use serde_json::{Value, json};

/// Bumped when a field is renamed or removed, so a reader goes by the
/// version and doesn't guess from what is there.
pub const SCHEMA_VERSION: u32 = 1;

/// One of CtrlAssist's parts, as its About row has it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Part {
    /// The row's identifier: `daemon`, `hider`, `tray`, `gui`, `cli`.
    pub name: String,
    /// Its release, when known.
    pub version: String,
    /// Whether it is there.
    pub running: bool,
    /// What its chip says: a release, or how it is.
    pub chip: String,
}

/// What the bundle is made from: plain data, so it is made the same in
/// a test as in the window.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    /// The About pane's rows.
    pub parts: Vec<Part>,
    /// The daemon's own state, if it is there.
    pub daemon: Option<DaemonState>,
    /// The known pads.
    pub pads: Vec<Pad>,
    /// The setups that exist.
    pub setups: Vec<Setup>,
    /// The kept profiles, with their text.
    pub profiles: Vec<Profile>,
    /// Facts of this machine.
    pub machine: Machine,
}

/// Facts of the machine the window runs on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Machine {
    /// The kernel's release.
    pub kernel: Option<String>,
    /// The distribution's id and version, from `os-release`.
    pub os_id: Option<String>,
    /// See `os_id`.
    pub os_version: Option<String>,
    /// `XDG_CURRENT_DESKTOP`, lowercased.
    pub desktop: Option<String>,
    /// `XDG_SESSION_TYPE`, lowercased.
    pub session_type: Option<String>,
    /// Whether this user can open `/dev/uinput` for writing, which
    /// every virtual pad needs.
    pub uinput: bool,
    /// The same for `/dev/uhid`, which a virtual DualSense needs.
    pub uhid: bool,
    /// Whether a Steam client is running.
    pub steam: bool,
}

impl Machine {
    /// Reads them from this machine.
    #[must_use]
    pub fn read() -> Self {
        let writable = |node: &str| std::fs::OpenOptions::new().write(true).open(node).is_ok();
        let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let env = |key: &str| {
            std::env::var(key)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| value.to_lowercase())
        };
        Self {
            kernel: std::fs::read_to_string("/proc/sys/kernel/osrelease")
                .ok()
                .map(|release| release.trim().to_string())
                .filter(|release| !release.is_empty()),
            os_id: os_release_field(&os, "ID"),
            os_version: os_release_field(&os, "VERSION_ID"),
            desktop: env("XDG_CURRENT_DESKTOP"),
            session_type: env("XDG_SESSION_TYPE"),
            uinput: writable("/dev/uinput"),
            uhid: writable("/dev/uhid"),
            steam: steam_runs(),
        }
    }
}

/// A field of `os-release`, without its quotes.
fn os_release_field(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
        .map(|value| value.trim().trim_matches('"').to_string())
        .filter(|value| !value.is_empty())
}

/// Whether some process is named `steam`.
fn steam_runs() -> bool {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            std::fs::read_to_string(entry.path().join("comm"))
                .is_ok_and(|comm| comm.trim() == "steam")
        })
}

/// The bundle, as JSON laid out to be read, with addresses masked.
#[must_use]
pub fn bundle(inputs: &Inputs) -> String {
    let value = render(inputs);
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    mask_addresses(&text)
}

fn render(inputs: &Inputs) -> Value {
    let parts: serde_json::Map<String, Value> = inputs
        .parts
        .iter()
        .map(|part| {
            (
                part.name.clone(),
                json!({ "version": part.version, "running": part.running, "says": part.chip }),
            )
        })
        .collect();
    let machine = &inputs.machine;
    json!({
        "schema_version": SCHEMA_VERSION,
        "window": {
            "version": env!("CARGO_PKG_VERSION"),
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "target": env!("CTRLASSIST_BUILD_TARGET"),
        },
        "parts": parts,
        "daemon": inputs.daemon.as_ref().map(daemon_section),
        "machine": {
            "kernel": machine.kernel,
            "os_id": machine.os_id,
            "os_version": machine.os_version,
            "desktop": machine.desktop,
            "session_type": machine.session_type,
            "uinput_writable": machine.uinput,
            "uhid_writable": machine.uhid,
            "steam_running": machine.steam,
        },
        "pads": inputs.pads,
        "setups": inputs.setups,
        "profiles": inputs.profiles,
    })
}

/// The daemon's published state, but for the directory it keeps the
/// user's files in, which names the home directory and so usually the
/// user: whether it named one is all that goes in. Field by field, so a
/// field added to the state doesn't reach the bundle unread.
fn daemon_section(daemon: &DaemonState) -> Value {
    json!({
        "version": daemon.version,
        "realtime": daemon.realtime,
        "hiding": daemon.hiding,
        "config": !daemon.config.is_empty(),
        "hider_version": daemon.hider_version,
    })
}

/// How many bytes an address is as text: six pairs of hex digits with
/// colons between.
const ADDRESS: usize = 17;

/// Whether `bytes` starts with an address.
fn is_address(bytes: &[u8]) -> bool {
    bytes.len() >= ADDRESS
        && bytes[..ADDRESS].iter().enumerate().all(|(at, byte)| {
            if at % 3 == 2 {
                *byte == b':'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

/// Whether `before` ends in a pair of hex digits and a colon, the pair
/// standing by itself: the tail of a run of pairs.
fn follows_a_pair(before: &[u8]) -> bool {
    let [.., first, second, b':'] = before else {
        return false;
    };
    first.is_ascii_hexdigit()
        && second.is_ascii_hexdigit()
        && before
            .len()
            .checked_sub(4)
            .and_then(|at| before.get(at))
            .is_none_or(|lead| !lead.is_ascii_alphanumeric())
}

/// Masks every address in `text`: `0a:1b:2c:3d:4e:14` becomes
/// `0a:1b:2c:xx:xx:xx#1`, the number being the address's place among
/// those met, so the same address reads the same everywhere and two of
/// one maker's don't read alike.
#[must_use]
pub fn mask_addresses(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut seen: Vec<String> = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while at < bytes.len() {
        // Not the tail of a longer run of pairs, nor the head of one. A
        // colon before it is fine after a word, as in `address:…`, and
        // not after another pair.
        let starts = !follows_a_pair(&bytes[..at])
            && bytes[..at]
                .last()
                .is_none_or(|before| !before.is_ascii_hexdigit());
        let ends = bytes
            .get(at + ADDRESS)
            .is_none_or(|next| !(next.is_ascii_hexdigit() || *next == b':'));
        if starts && ends && is_address(&bytes[at..]) {
            // All ASCII, by `is_address`, so the slice is on boundaries.
            let address = text[at..at + ADDRESS].to_ascii_lowercase();
            let number = match seen.iter().position(|known| *known == address) {
                Some(place) => place + 1,
                None => {
                    seen.push(address.clone());
                    seen.len()
                }
            };
            out.push_str(&address[..9]);
            out.push_str("xx:xx:xx#");
            out.push_str(&number.to_string());
            at += ADDRESS;
            continue;
        }
        // One whole character, whatever its width.
        let width = text[at..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&text[at..at + width]);
        at += width;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_keeps_its_vendor_half_and_a_number_tells_units_apart() {
        let text = "primary = \"0a:1b:2c:3d:4e:14\", assist = \"0A:1B:2C:AA:BB:CC\", again address:0a:1b:2c:3d:4e:14.";
        assert_eq!(
            mask_addresses(text),
            "primary = \"0a:1b:2c:xx:xx:xx#1\", assist = \"0a:1b:2c:xx:xx:xx#2\", again address:0a:1b:2c:xx:xx:xx#1."
        );
    }

    #[test]
    fn what_only_looks_like_part_of_an_address_is_left_alone() {
        for text in [
            "model:054c:09cc",
            "ctrlassist/alive/daemon",
            // Seven pairs is something else.
            "00:11:22:33:44:55:66",
            "at 12:34:56 on the day",
            "naïve — 0a:1b:2c",
            "0123456789ab:cd:ef:01:23:45",
            "",
        ] {
            assert_eq!(mask_addresses(text), text);
        }
    }

    #[test]
    fn an_os_release_field_is_read_without_its_quotes() {
        let text = "NAME=NixOS\nID=nixos\nVERSION_ID=\"26.05\"\nIDX=other\n";
        assert_eq!(os_release_field(text, "ID").as_deref(), Some("nixos"));
        assert_eq!(
            os_release_field(text, "VERSION_ID").as_deref(),
            Some("26.05")
        );
        assert_eq!(os_release_field(text, "BUILD_ID"), None);
    }

    #[test]
    fn the_daemon_section_says_whether_it_named_its_directory_and_not_where() {
        let inputs = Inputs {
            daemon: Some(DaemonState {
                version: "0.5.0".into(),
                realtime: true,
                hiding: ctrlassist::Hiding::Service,
                config: "/home/someone/.config/ctrlassist".into(),
                hider_version: Some("0.5.0".into()),
            }),
            ..Inputs::default()
        };
        let text = bundle(&inputs);
        assert!(!text.contains("/home"), "{text}");
        assert!(!text.contains("someone"), "{text}");
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            value["daemon"],
            json!({
                "version": "0.5.0",
                "realtime": true,
                "hiding": "service",
                "config": true,
                "hider_version": "0.5.0",
            })
        );
    }

    #[test]
    fn the_bundle_holds_each_section_and_no_whole_address() {
        let inputs = Inputs {
            parts: vec![Part {
                name: "daemon".into(),
                version: "0.5.0".into(),
                running: true,
                chip: "v0.5.0".into(),
            }],
            profiles: vec![Profile {
                name: "couch".into(),
                toml: "mode = \"priority\"\nprimary = \"0a:1b:2c:3d:4e:14\"\n".into(),
                unset: Vec::new(),
                problem: None,
            }],
            machine: Machine {
                kernel: Some("7.3.4".into()),
                uinput: true,
                ..Machine::default()
            },
            ..Inputs::default()
        };
        let text = bundle(&inputs);
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["schema_version"], SCHEMA_VERSION);
        assert_eq!(value["parts"]["daemon"]["says"], "v0.5.0");
        assert_eq!(value["daemon"], Value::Null);
        assert_eq!(value["machine"]["kernel"], "7.3.4");
        assert_eq!(value["machine"]["uinput_writable"], true);
        assert_eq!(value["machine"]["os_id"], Value::Null);
        assert!(value["pads"].as_array().unwrap().is_empty());
        let toml = value["profiles"][0]["toml"].as_str().unwrap();
        assert!(toml.contains("0a:1b:2c:xx:xx:xx#1"), "{toml}");
        assert!(!text.contains("3d:4e:14"));
    }
}
