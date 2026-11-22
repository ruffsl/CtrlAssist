//! The profiles the daemon keeps: one TOML file each, named for the
//! profile, in the `profiles` directory of its configuration directory.
//!
//! The files are the daemon's own. Clients write profiles through it, so
//! every one it wrote was found to be right first. A file that arrived
//! some other way, or was written for a later release, is still listed,
//! with what is wrong with it, and is never changed or removed unless a
//! client asks.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use ctrlassist_protocol::error::{ErrorCode, WireError};
use ctrlassist_protocol::payload;
use ctrlassist_protocol::profile::{Profile, is_name};
use tracing::warn;

use crate::durable;

/// The most a profile's text may be. The largest sensible profile is a
/// few kilobytes; this stops a client filling the disk or the bus.
const MOST_BYTES: usize = 64 * 1024;

/// The kept profiles, by name.
#[derive(Debug)]
pub(crate) struct Profiles {
    directory: PathBuf,
    kept: BTreeMap<String, String>,
}

impl Profiles {
    /// Reads the profiles in `directory`: every `<name>.toml` whose name
    /// a profile may have. Anything else there is left alone.
    pub(crate) fn open(directory: &Path) -> Self {
        let mut kept = BTreeMap::new();
        for entry in fs::read_dir(directory).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|file| file.to_str())
                .and_then(|file| file.strip_suffix(".toml"))
                .filter(|name| is_name(name));
            let Some(name) = name else { continue };
            match fs::read_to_string(&path) {
                Ok(text) => {
                    kept.insert(name.to_string(), text);
                }
                Err(error) => {
                    warn!(path = %path.display(), %error, "a profile can't be read; leaving it out");
                }
            }
        }
        Self {
            directory: directory.to_path_buf(),
            kept,
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(format!("{name}.toml"))
    }

    /// Reads the directory again, for files changed some other way than
    /// through the daemon. Says which profiles are new or changed, and
    /// which are gone, each in name order.
    pub(crate) fn reload(&mut self) -> (Vec<String>, Vec<String>) {
        let read = Self::open(&self.directory).kept;
        let changed = read
            .iter()
            .filter(|(name, text)| self.kept.get(*name) != Some(*text))
            .map(|(name, _)| name.clone())
            .collect();
        let removed = self
            .kept
            .keys()
            .filter(|name| !read.contains_key(*name))
            .cloned()
            .collect();
        self.kept = read;
        (changed, removed)
    }

    /// Every profile's name.
    pub(crate) fn names(&self) -> impl Iterator<Item = &str> {
        self.kept.keys().map(String::as_str)
    }

    /// A profile's published state.
    pub(crate) fn profile(&self, name: &str) -> Option<payload::Profile> {
        let toml = self.kept.get(name)?;
        Some(payload::Profile {
            name: name.to_string(),
            toml: toml.clone(),
            problem: Profile::check(toml).err().map(Into::into),
        })
    }

    /// What is wrong with `toml` as a profile, if anything.
    pub(crate) fn problem(toml: &str) -> Option<payload::Problem> {
        if toml.len() > MOST_BYTES {
            return Some(payload::Problem {
                at: String::new(),
                message: format!("a profile is at most {MOST_BYTES} bytes"),
            });
        }
        Profile::check(toml).err().map(Into::into)
    }

    /// Keeps `toml` as the profile `name`, durably, in place of any it
    /// had. A profile that is wrong is refused, and what was kept stays.
    pub(crate) fn put(&mut self, name: &str, toml: &str) -> Result<(), WireError> {
        if !is_name(name) {
            return Err(WireError::bad_request(format!(
                "a profile's name is 1 to 32 lowercase letters, digits and hyphens, which `{}` isn't",
                name.escape_debug()
            )));
        }
        if let Some(problem) = Self::problem(toml) {
            return Err(WireError::refusal(
                ErrorCode::InvalidProfile,
                problem.to_string(),
            ));
        }
        durable::write(&self.path(name), toml).map_err(|error| {
            WireError::internal(format!("the profile couldn't be written: {error}"))
        })?;
        self.kept.insert(name.to_string(), toml.to_string());
        Ok(())
    }

    /// Stops keeping the profile `name`.
    pub(crate) fn delete(&mut self, name: &str) -> Result<(), WireError> {
        if !self.kept.contains_key(name) {
            return Err(WireError::refusal(
                ErrorCode::UnknownProfile,
                format!("no profile is called {}", name.escape_debug()),
            ));
        }
        durable::remove(&self.path(name)).map_err(|error| {
            WireError::internal(format!("the profile couldn't be removed: {error}"))
        })?;
        self.kept.remove(name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RIGHT: &str =
        "mode = \"priority\"\nprimary = \"0c:27:56:4f:bd:14\"\nassist = \"model:054c:09cc\"\n";

    #[test]
    fn a_profile_put_is_kept_across_a_restart_until_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let directory = dir.path().join("profiles");
        let mut profiles = Profiles::open(&directory);
        assert_eq!(profiles.names().count(), 0);
        profiles.put("couch", RIGHT).unwrap();
        assert_eq!(
            fs::read_to_string(directory.join("couch.toml")).unwrap(),
            RIGHT
        );

        let mut profiles = Profiles::open(&directory);
        assert_eq!(profiles.names().collect::<Vec<_>>(), ["couch"]);
        let kept = profiles.profile("couch").unwrap();
        assert_eq!((kept.name.as_str(), kept.toml.as_str()), ("couch", RIGHT));
        assert_eq!(kept.problem, None);
        assert_eq!(profiles.profile("desk"), None);

        // Put again, it is replaced.
        let other = RIGHT.replace("priority", "average");
        profiles.put("couch", &other).unwrap();
        assert_eq!(profiles.profile("couch").unwrap().toml, other);

        profiles.delete("couch").unwrap();
        assert!(!directory.join("couch.toml").exists());
        assert_eq!(
            profiles.delete("couch").unwrap_err().code,
            ErrorCode::UnknownProfile
        );
        assert_eq!(Profiles::open(&directory).names().count(), 0);
    }

    #[test]
    fn a_profile_that_is_wrong_is_refused_and_what_was_kept_stays() {
        let dir = tempfile::tempdir().unwrap();
        let mut profiles = Profiles::open(dir.path());
        profiles.put("couch", RIGHT).unwrap();

        let wrong = profiles.put("couch", "mode = \"priority\"").unwrap_err();
        assert_eq!(wrong.code, ErrorCode::InvalidProfile);
        assert!(wrong.message.starts_with("mode: "), "{}", wrong.message);
        let long = format!("{RIGHT}# {}\n", "x".repeat(MOST_BYTES));
        let refused = profiles.put("couch", &long).unwrap_err();
        assert_eq!(refused.code, ErrorCode::InvalidProfile);
        assert!(refused.message.contains("at most 65536 bytes"));
        assert_eq!(profiles.profile("couch").unwrap().toml, RIGHT);

        for name in ["", "Couch", "../couch", "a/b", "couch.toml"] {
            let refused = profiles.put(name, RIGHT).unwrap_err();
            assert_eq!(refused.code, ErrorCode::BadRequest, "{name}");
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_file_that_arrived_some_other_way_is_listed_with_what_is_wrong() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("edited.toml"), "mode = \"sideways\"\n").unwrap();
        fs::write(
            dir.path().join("later.toml"),
            format!("schema = 9\n{RIGHT}"),
        )
        .unwrap();
        // Not profiles: another kind of file, a name no profile may have,
        // a directory, and text that isn't text.
        fs::write(dir.path().join("notes.txt"), "x").unwrap();
        fs::write(dir.path().join("Not A Name.toml"), RIGHT).unwrap();
        fs::create_dir(dir.path().join("folder.toml")).unwrap();
        fs::write(dir.path().join("binary.toml"), [0xff, 0xfe]).unwrap();

        let mut profiles = Profiles::open(dir.path());
        assert_eq!(profiles.names().collect::<Vec<_>>(), ["edited", "later"]);
        let edited = profiles.profile("edited").unwrap().problem.unwrap();
        assert_eq!(edited.at, "line 1");
        let later = profiles.profile("later").unwrap().problem.unwrap();
        assert_eq!(later.at, "schema");

        // It can be put right, or deleted, like any other.
        profiles.put("edited", RIGHT).unwrap();
        assert_eq!(profiles.profile("edited").unwrap().problem, None);
        profiles.delete("later").unwrap();
        assert!(dir.path().join("notes.txt").exists());
        assert!(dir.path().join("Not A Name.toml").exists());
    }

    #[test]
    fn a_profile_that_cannot_be_written_or_removed_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut profiles = Profiles::open(dir.path());
        fs::create_dir(dir.path().join("couch.toml.partial")).unwrap();
        let failed = profiles.put("couch", RIGHT).unwrap_err();
        assert_eq!(failed.code, ErrorCode::Internal);
        assert_eq!(profiles.names().count(), 0);

        profiles.put("desk", RIGHT).unwrap();
        fs::remove_file(dir.path().join("desk.toml")).unwrap();
        let failed = profiles.delete("desk").unwrap_err();
        assert_eq!(failed.code, ErrorCode::Internal);
        assert_eq!(profiles.names().collect::<Vec<_>>(), ["desk"]);
    }
}
