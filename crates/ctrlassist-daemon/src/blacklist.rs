//! Steam's controller blacklist, for where pads are only grabbed.
//!
//! A grab silences a pad's event node, but Steam reads PlayStation pads
//! some other way and still sees them. Steam can be told to leave a
//! model of pad alone: `controller_blacklist` in its `config.vdf`, a
//! list of `vendor/product` pairs it reads when it starts. So without
//! the hiding service, a setup's pads are added there while it runs.
//!
//! The file is Steam's, not the daemon's. Only the one line is touched,
//! only entries the daemon added are ever removed, and each is recorded
//! in the daemon's state directory before Steam's file is written, so
//! that a daemon that died is cleaned up after by the next.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::durable;

/// The key of the list, as Steam writes it.
const KEY: &str = "\"controller_blacklist\"";

/// The schema this release writes and reads.
const SCHEMA: u32 = 1;

/// What the daemon has added to Steam's files, as kept on disk.
#[derive(Serialize, Deserialize, Default)]
struct Record {
    schema: u32,
    /// The entries added to each file, such as `054c/0ce6`, by the
    /// file's path.
    #[serde(default)]
    added: BTreeMap<PathBuf, BTreeSet<String>>,
}

/// A model as Steam's list writes it.
pub(crate) fn entry(vendor: u16, product: u16) -> String {
    format!("{vendor:04x}/{product:04x}")
}

/// The list in a line of `config.vdf`: the second quoted string.
fn listed(line: &str) -> Option<Vec<&str>> {
    let value = line.split('"').nth(3)?;
    Some(
        value
            .split(',')
            .map(str::trim)
            .filter(|entry| !entry.is_empty())
            .collect(),
    )
}

/// `text` with `add` in Steam's list and `remove` out of it, if that
/// changes it, and which of `add` weren't there before, which are the
/// daemon's to take out again. Nothing if the file isn't shaped as
/// expected: it is then left alone. Everything but the one line is kept
/// as it was.
fn edited(
    text: &str,
    add: &BTreeSet<String>,
    remove: &BTreeSet<String>,
) -> Option<(Option<String>, BTreeSet<String>)> {
    let ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.split(ending).map(str::to_string).collect();
    let at = lines
        .iter()
        .position(|line| line.trim_start().starts_with(KEY));
    let before: Vec<String> = match at {
        Some(at) => listed(&lines[at])?
            .into_iter()
            .map(str::to_string)
            .collect(),
        None => Vec::new(),
    };
    let mut now: Vec<String> = before
        .iter()
        .filter(|entry| !remove.contains(*entry))
        .cloned()
        .collect();
    let mut ours = BTreeSet::new();
    for entry in add {
        if !now.contains(entry) {
            now.push(entry.clone());
            ours.insert(entry.clone());
        }
    }
    if now == before {
        return Some((None, ours));
    }
    let line = format!("\t{KEY}\t\t\"{}\"", now.join(","));
    match at {
        Some(at) => lines[at] = line,
        None => {
            // Just inside the outermost block, where Steam keeps it.
            let store = lines
                .iter()
                .position(|line| line.trim() == "\"InstallConfigStore\"")?;
            let brace = store + 1;
            (lines.get(brace)?.trim() == "{").then_some(())?;
            lines.insert(brace + 1, line);
        }
    }
    Some((Some(lines.join(ending)), ours))
}

/// Steam's lists, and what the daemon has in them.
#[derive(Debug)]
pub(crate) struct Blacklist {
    /// Steam's `config.vdf` files: one per way Steam is installed.
    configs: Vec<PathBuf>,
    record: PathBuf,
    /// Each setup's entries.
    wanted: BTreeMap<String, BTreeSet<String>>,
    /// The entries the daemon added to each file, and so may remove from
    /// it: an entry may be the user's own in one file and not another.
    added: BTreeMap<PathBuf, BTreeSet<String>>,
}

impl Blacklist {
    /// Opens the lists, and takes out whatever an earlier daemon added
    /// and left: no setup runs yet.
    pub(crate) fn open(configs: Vec<PathBuf>, state: &Path) -> Self {
        let record = state.join("steam-blacklist.toml");
        let left = fs::read_to_string(&record)
            .ok()
            .and_then(|text| toml::from_str::<Record>(&text).ok())
            .filter(|record| record.schema == SCHEMA)
            .map(|record| record.added)
            .unwrap_or_default();
        let mut blacklist = Self {
            configs,
            record,
            wanted: BTreeMap::new(),
            added: left,
        };
        if !blacklist.added.is_empty() {
            info!(entries = ?blacklist.added, "entries an earlier daemon left in Steam's controller blacklist are taken out");
            blacklist.apply();
        }
        blacklist
    }

    /// No lists: for a daemon that leaves Steam's files alone.
    pub(crate) fn none() -> Self {
        Self {
            configs: Vec::new(),
            record: PathBuf::new(),
            wanted: BTreeMap::new(),
            added: BTreeMap::new(),
        }
    }

    /// Notes `entries` as the ones the daemon added to `config`.
    fn note(&mut self, config: &Path, entries: BTreeSet<String>) {
        if entries.is_empty() {
            self.added.remove(config);
        } else {
            self.added.insert(config.to_path_buf(), entries);
        }
    }

    /// Sets which models `setup` wants Steam to leave alone; none when
    /// it has stopped.
    pub(crate) fn set(&mut self, setup: &str, models: BTreeSet<String>) {
        if models.is_empty() {
            if self.wanted.remove(setup).is_none() {
                return;
            }
        } else {
            self.wanted.insert(setup.to_string(), models);
        }
        self.apply();
    }

    /// Takes out everything the daemon added, as it stops.
    pub(crate) fn clear(&mut self) {
        self.wanted.clear();
        self.apply();
    }

    fn save(&self) {
        let record = Record {
            schema: SCHEMA,
            added: self.added.clone(),
        };
        let saved = toml::to_string(&record)
            .map_err(std::io::Error::other)
            .and_then(|text| durable::write(&self.record, &text));
        if let Err(error) = saved {
            warn!(path = %self.record.display(), %error, "what was added to Steam's controller blacklist couldn't be recorded");
        }
    }

    /// Makes Steam's files hold what the setups want, of the daemon's
    /// own entries.
    fn apply(&mut self) {
        if self.configs.is_empty() {
            return;
        }
        let want: BTreeSet<String> = self.wanted.values().flatten().cloned().collect();
        for config in self.configs.clone() {
            let added = self.added.get(&config).cloned().unwrap_or_default();
            let remove: BTreeSet<String> = added.difference(&want).cloned().collect();
            let text = match fs::read_to_string(&config) {
                Ok(text) => text,
                // Gone, and what was added with it.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.note(&config, BTreeSet::new());
                    continue;
                }
                // What was added stays the daemon's, to take out once the
                // file can be read.
                Err(error) => {
                    warn!(path = %config.display(), %error, "Steam's controller blacklist couldn't be read");
                    continue;
                }
            };
            // A file not shaped as Steam's is left alone, and what was
            // added to it stays the daemon's.
            let Some((text, ours)) = edited(&text, &want, &remove) else {
                continue;
            };
            let kept: BTreeSet<String> = added.difference(&remove).cloned().collect();
            let Some(text) = text else {
                self.note(&config, kept);
                continue;
            };
            // Recorded as added before Steam's file is touched, so that a
            // daemon that dies between the two is cleaned up after.
            self.note(&config, added.union(&ours).cloned().collect());
            self.save();
            match durable::write(&config, &text) {
                Ok(()) => {
                    info!(path = %config.display(), added = ?ours, removed = ?remove, "Steam's controller blacklist was changed; Steam takes it up when it next starts");
                    self.note(&config, kept.union(&ours).cloned().collect());
                }
                // As it was: what was to be taken out is still there.
                Err(error) => {
                    warn!(path = %config.display(), %error, "Steam's controller blacklist couldn't be written");
                    self.note(&config, added);
                }
            }
        }
        self.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEAM: &str = "\"InstallConfigStore\"\n{\n\t\"controller_blacklist\"\t\t\"\"\n\t\"Software\"\n\t{\n\t\t\"Valve\"\n\t\t{\n\t\t}\n\t}\n}\n";

    fn set(entries: &[&str]) -> BTreeSet<String> {
        entries.iter().map(ToString::to_string).collect()
    }

    fn list(path: &Path) -> String {
        let text = fs::read_to_string(path).unwrap();
        let line = text.lines().find(|line| line.contains(KEY)).unwrap();
        listed(line).unwrap().join(",")
    }

    #[test]
    fn only_the_one_line_changes_and_only_when_it_must() {
        let (text, ours) = edited(STEAM, &set(&["054c/0ce6"]), &set(&[])).unwrap();
        let text = text.unwrap();
        assert_eq!(
            text,
            STEAM.replace("\t\t\"\"", "\t\t\"054c/0ce6\""),
            "every other byte is as it was"
        );
        assert_eq!(ours, set(&["054c/0ce6"]));
        // Already there, by the user's own doing: nothing to write, and
        // not the daemon's to take out.
        let unchanged = Some((None, set(&[])));
        assert_eq!(edited(&text, &set(&["054c/0ce6"]), &set(&[])), unchanged);
        // Beside the user's own entry, which stays first and stays.
        let users = STEAM.replace("\t\t\"\"", "\t\t\"28de/1142\"");
        let (text, ours) = edited(&users, &set(&["045e/02dd", "054c/0ce6"]), &set(&[])).unwrap();
        let text = text.unwrap();
        assert!(text.contains("\"28de/1142,045e/02dd,054c/0ce6\""), "{text}");
        assert_eq!(ours.len(), 2);
        let (text, _) = edited(&text, &set(&[]), &set(&["045e/02dd", "054c/0ce6"])).unwrap();
        let text = text.unwrap();
        assert_eq!(text, users);
        assert_eq!(edited(&text, &set(&[]), &set(&["054c/0ce6"])), unchanged);
    }

    #[test]
    fn a_file_without_the_line_gets_it_where_steam_keeps_it_and_keeps_its_line_endings() {
        let bare = "\"InstallConfigStore\"\r\n{\r\n\t\"Software\"\r\n\t{\r\n\t}\r\n}\r\n";
        let (text, _) = edited(bare, &set(&["054c/0ce6"]), &set(&[])).unwrap();
        assert_eq!(
            text.unwrap(),
            "\"InstallConfigStore\"\r\n{\r\n\t\"controller_blacklist\"\t\t\"054c/0ce6\"\r\n\t\"Software\"\r\n\t{\r\n\t}\r\n}\r\n"
        );
        // A file that isn't shaped as Steam's is left alone.
        for odd in [
            "",
            "{}",
            "\"Other\"\n{\n}\n",
            "\"InstallConfigStore\"\n\"x\"\n",
            "\t\"controller_blacklist\"\n",
        ] {
            assert_eq!(
                edited(odd, &set(&["054c/0ce6"]), &set(&[])),
                None,
                "{odd:?}"
            );
        }
        assert_eq!(entry(0x54c, 0xce6), "054c/0ce6");
    }

    #[test]
    fn entries_follow_the_setups_and_only_the_daemon_s_own_are_taken_out() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.vdf");
        fs::write(&config, STEAM.replace("\t\t\"\"", "\t\t\"28de/1142\"")).unwrap();
        let missing = dir.path().join("flatpak/config.vdf");
        let mut blacklist = Blacklist::open(vec![config.clone(), missing], dir.path());
        blacklist.set("couch", set(&["054c/0ce6", "28de/1142"]));
        blacklist.set("desk", set(&["054c/0ce6", "045e/02dd"]));
        assert_eq!(list(&config), "28de/1142,054c/0ce6,045e/02dd");
        // One setup stops: what the other still wants stays.
        blacklist.set("desk", set(&[]));
        assert_eq!(list(&config), "28de/1142,054c/0ce6");
        blacklist.set("desk", set(&[]));
        // The last stops: the user's own entry, which a setup also
        // wanted, was never the daemon's and stays.
        blacklist.set("couch", set(&[]));
        assert_eq!(list(&config), "28de/1142");
        blacklist.set("couch", set(&["054c/0ce6"]));
        blacklist.clear();
        assert_eq!(list(&config), "28de/1142");
    }

    #[test]
    fn what_a_daemon_that_died_left_is_taken_out_by_the_next() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.vdf");
        fs::write(&config, STEAM).unwrap();
        let mut died = Blacklist::open(vec![config.clone()], dir.path());
        died.set("couch", set(&["054c/0ce6"]));
        assert_eq!(list(&config), "054c/0ce6");
        drop(died);
        // The user adds one of their own meanwhile.
        let text = fs::read_to_string(&config).unwrap();
        fs::write(&config, text.replace("054c/0ce6", "054c/0ce6,28de/1142")).unwrap();
        let next = Blacklist::open(vec![config.clone()], dir.path());
        assert_eq!(list(&config), "28de/1142");
        assert!(next.added.is_empty());
        let record = fs::read_to_string(dir.path().join("steam-blacklist.toml")).unwrap();
        assert_eq!(
            toml::from_str::<Record>(&record).unwrap().added,
            BTreeMap::new()
        );
        // A record that can't be read is nothing to act on.
        fs::write(
            dir.path().join("steam-blacklist.toml"),
            "schema = 9\nadded = [\"28de/1142\"]\n",
        )
        .unwrap();
        let _ = Blacklist::open(vec![config.clone()], dir.path());
        assert_eq!(list(&config), "28de/1142");
        // Without Steam there is nothing to do, and nothing goes wrong.
        let mut gone = Blacklist::open(vec![dir.path().join("nowhere/config.vdf")], dir.path());
        gone.set("couch", set(&["054c/0ce6"]));
        gone.clear();
        // A daemon given no lists leaves no record either.
        let mut none = Blacklist::none();
        none.set("couch", set(&["054c/0ce6"]));
        none.clear();
        assert!(none.added.is_empty());
    }

    /// Steam may be installed twice, as a distribution installs it and
    /// as a Flatpak, and an entry the user put in one file is theirs in
    /// that file even where the daemon added it to the other.
    #[test]
    fn an_entry_is_the_daemon_s_only_in_the_files_it_added_it_to() {
        let dir = tempfile::tempdir().unwrap();
        let native = dir.path().join("native/config.vdf");
        let flatpak = dir.path().join("flatpak/config.vdf");
        fs::create_dir_all(native.parent().unwrap()).unwrap();
        fs::create_dir_all(flatpak.parent().unwrap()).unwrap();
        fs::write(&native, STEAM.replace("\t\t\"\"", "\t\t\"28de/1142\"")).unwrap();
        fs::write(&flatpak, STEAM).unwrap();
        let mut blacklist = Blacklist::open(vec![native.clone(), flatpak.clone()], dir.path());
        blacklist.set("couch", set(&["28de/1142"]));
        assert_eq!(
            (list(&native), list(&flatpak)),
            ("28de/1142".into(), "28de/1142".into())
        );
        // A daemon that dies here leaves a record that says the same.
        let next = Blacklist::open(vec![native.clone(), flatpak.clone()], dir.path());
        assert_eq!(
            (list(&native), list(&flatpak)),
            ("28de/1142".into(), String::new())
        );
        drop(next);
        blacklist.set("couch", set(&["28de/1142"]));
        blacklist.set("couch", set(&[]));
        assert_eq!(
            (list(&native), list(&flatpak)),
            ("28de/1142".into(), String::new())
        );
    }

    /// An entry that can't be taken out now stays the daemon's, and is
    /// taken out once it can be.
    #[test]
    fn an_entry_that_cannot_be_taken_out_yet_is_taken_out_later() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("config.vdf");
        fs::write(&config, STEAM).unwrap();
        let mut blacklist = Blacklist::open(vec![config.clone()], dir.path());
        blacklist.set("couch", set(&["054c/0ce6"]));
        // Something sits where the new file is written before it takes
        // the old one's place.
        let partial = dir.path().join("config.vdf.partial");
        fs::create_dir(&partial).unwrap();
        blacklist.set("couch", set(&[]));
        assert_eq!(list(&config), "054c/0ce6");
        fs::remove_dir(&partial).unwrap();
        blacklist.clear();
        assert_eq!(list(&config), "");
    }
}
