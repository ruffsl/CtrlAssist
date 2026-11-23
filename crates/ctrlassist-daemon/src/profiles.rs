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
use ctrlassist_protocol::profile::{self, Profile, is_name};
use tracing::warn;

use crate::durable;

/// The most a profile's text may be. The largest sensible profile is a
/// few kilobytes; this stops a client filling the disk or the bus.
const MOST_BYTES: usize = 64 * 1024;

/// The text of the profile at `path`, if it is text and no larger than a
/// profile may be: a larger one isn't read whole, nor kept.
fn read(path: &Path) -> std::io::Result<String> {
    use std::io::Read as _;
    let mut text = String::new();
    let most = u64::try_from(MOST_BYTES).expect("the most a profile may be fits in a u64");
    fs::File::open(path)?
        .take(most + 1)
        .read_to_string(&mut text)?;
    if text.len() > MOST_BYTES {
        return Err(std::io::Error::other(format!(
            "a profile is at most {MOST_BYTES} bytes"
        )));
    }
    Ok(text)
}

/// The kept profiles, by name.
#[derive(Clone, Debug)]
pub(crate) struct Profiles {
    directory: PathBuf,
    kept: BTreeMap<String, String>,
}

impl Profiles {
    /// Reads the profiles in `directory`: every `<name>.toml` whose name
    /// a profile may have. Anything else there is left alone.
    pub(crate) fn open(directory: &Path) -> Self {
        let mut kept = BTreeMap::new();
        let mut unread = Vec::new();
        for entry in fs::read_dir(directory).into_iter().flatten().flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .and_then(|file| file.to_str())
                .and_then(|file| file.strip_suffix(".toml"))
                .filter(|name| is_name(name));
            let Some(name) = name else { continue };
            match read(&path) {
                Ok(text) => {
                    kept.insert(name.to_string(), text);
                }
                Err(error) => {
                    warn!(path = %path.display(), %error, "a profile can't be read; leaving it out, and the file as it is");
                    unread.push(name.to_string());
                }
            }
        }
        let mut profiles = Self {
            directory: directory.to_path_buf(),
            kept,
        };
        profiles.ensure_default(unread.iter().any(|name| name == profile::DEFAULT));
        profiles
    }

    /// Makes the default profile where there is none: it is always
    /// kept. One that can't be written is kept until the daemon stops,
    /// as is one whose file is there and can't be read, which is left as
    /// it is.
    fn ensure_default(&mut self, unread: bool) {
        if self.kept.contains_key(profile::DEFAULT) {
            return;
        }
        if !unread
            && let Err(error) = durable::write(&self.path(profile::DEFAULT), profile::DEFAULT_TOML)
        {
            warn!(%error, "the default profile couldn't be written; it is kept until the daemon stops");
        }
        self.kept.insert(
            profile::DEFAULT.to_string(),
            profile::DEFAULT_TOML.to_string(),
        );
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(format!("{name}.toml"))
    }

    /// Reads the directory again, for files changed some other way than
    /// through the daemon. Says which profiles are new or changed, and
    /// which are gone, each in name order.
    pub(crate) fn reload(&mut self) -> (Vec<String>, Vec<String>) {
        // A default profile removed by hand is made again.
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
        let unset = Profile::unchosen(toml);
        Some(payload::Profile {
            name: name.to_string(),
            toml: toml.clone(),
            problem: Self::wrong(toml),
            unset,
        })
    }

    /// What keeps `toml` from being a profile that runs, if anything.
    pub(crate) fn problem(toml: &str) -> Option<payload::Problem> {
        Self::too_large(toml).or_else(|| Profile::check(toml).err().map(Into::into))
    }

    /// Whether `toml` is larger than a profile may be.
    fn too_large(toml: &str) -> Option<payload::Problem> {
        (toml.len() > MOST_BYTES).then(|| payload::Problem {
            at: String::new(),
            message: format!("a profile is at most {MOST_BYTES} bytes"),
        })
    }

    /// What keeps `toml` from being kept, if anything. A preset whose
    /// pads are still to choose isn't wrong: it is kept, and can't run
    /// until they are chosen. One too large is wrong whatever it is.
    fn wrong(toml: &str) -> Option<payload::Problem> {
        Self::too_large(toml)
            .or_else(|| Self::problem(toml).filter(|_| Profile::unchosen(toml).is_empty()))
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
        if let Some(problem) = Self::wrong(toml) {
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

    /// Stops keeping the profile `name`. The default profile is always
    /// kept.
    pub(crate) fn delete(&mut self, name: &str) -> Result<(), WireError> {
        if name == profile::DEFAULT {
            return Err(WireError::refusal(
                ErrorCode::ProfileReserved,
                "the default profile is always kept; it can be changed, not deleted",
            ));
        }
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
        assert_eq!(profiles.names().collect::<Vec<_>>(), ["default"]);
        profiles.put("couch", RIGHT).unwrap();
        assert_eq!(
            fs::read_to_string(directory.join("couch.toml")).unwrap(),
            RIGHT
        );

        let mut profiles = Profiles::open(&directory);
        assert_eq!(profiles.names().collect::<Vec<_>>(), ["couch", "default"]);
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
        assert_eq!(Profiles::open(&directory).names().count(), 1);
    }

    /// The default profile is always kept: made where there is none,
    /// with its pads still to choose, changed like any other, never
    /// deleted, and made again if its file is removed.
    #[test]
    fn the_default_profile_is_always_kept() {
        let dir = tempfile::tempdir().unwrap();
        let mut profiles = Profiles::open(dir.path());
        let made = profiles.profile("default").unwrap();
        assert_eq!(made.toml, profile::DEFAULT_TOML);
        assert_eq!(
            (made.problem, made.unset),
            (None, vec!["primary".to_string(), "assist".to_string()])
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("default.toml")).unwrap(),
            profile::DEFAULT_TOML
        );
        assert_eq!(
            profiles.delete("default").unwrap_err().code,
            ErrorCode::ProfileReserved
        );

        // One pad chosen is still not a profile that runs, and is kept.
        let one = "mode = \"toggle\"\nprimary = \"model:054c:09cc\"\n";
        profiles.put("default", one).unwrap();
        assert_eq!(profiles.profile("default").unwrap().unset, ["assist"]);
        profiles.put("default", RIGHT).unwrap();
        let whole = profiles.profile("default").unwrap();
        assert!(whole.unset.is_empty() && whole.problem.is_none());
        // What a person made of it survives a restart.
        assert_eq!(
            Profiles::open(dir.path()).profile("default").unwrap().toml,
            RIGHT
        );

        fs::remove_file(dir.path().join("default.toml")).unwrap();
        let (changed, removed) = profiles.reload();
        assert_eq!(
            (changed, removed),
            (vec!["default".to_string()], Vec::new())
        );
        assert_eq!(
            profiles.profile("default").unwrap().toml,
            profile::DEFAULT_TOML
        );
    }

    #[test]
    fn a_profile_that_is_wrong_is_refused_and_what_was_kept_stays() {
        let dir = tempfile::tempdir().unwrap();
        let mut profiles = Profiles::open(dir.path());
        profiles.put("couch", RIGHT).unwrap();

        let wrong = profiles
            .put("couch", "mode = \"priority\"\npad = \"model:054c:09cc\"\n")
            .unwrap_err();
        assert_eq!(wrong.code, ErrorCode::InvalidProfile);
        assert!(wrong.message.starts_with("pad: "), "{}", wrong.message);
        let long = format!("{RIGHT}# {}\n", "x".repeat(MOST_BYTES));
        let refused = profiles.put("couch", &long).unwrap_err();
        assert_eq!(refused.code, ErrorCode::InvalidProfile);
        assert!(refused.message.contains("at most 65536 bytes"));
        assert_eq!(profiles.profile("couch").unwrap().toml, RIGHT);

        for name in ["", "Couch", "../couch", "a/b", "couch.toml"] {
            let refused = profiles.put(name, RIGHT).unwrap_err();
            assert_eq!(refused.code, ErrorCode::BadRequest, "{name}");
        }
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
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
        assert_eq!(
            profiles.names().collect::<Vec<_>>(),
            ["default", "edited", "later"]
        );
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

    /// A default profile whose file can't be read is the user's, left as
    /// it is; the default is kept all the same, until the daemon stops.
    #[test]
    fn a_default_profile_that_cannot_be_read_is_left_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("default.toml");
        let unreadable = [b'#', 0xff, 0xfe, b'\n'];
        fs::write(&path, unreadable).unwrap();
        let mut profiles = Profiles::open(dir.path());
        assert_eq!(
            profiles.profile("default").unwrap().toml,
            profile::DEFAULT_TOML
        );
        assert_eq!(fs::read(&path).unwrap(), unreadable);
        profiles.reload();
        assert_eq!(fs::read(&path).unwrap(), unreadable, "nor on a reload");
        // Put by a client, it is written as any other.
        profiles.put("default", RIGHT).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), RIGHT);
    }

    /// No profile is larger than the most a profile may be, however it
    /// came: a preset whose pads are still to choose included, and a file
    /// that arrived some other way.
    #[test]
    fn no_profile_is_larger_than_a_profile_may_be() {
        let dir = tempfile::tempdir().unwrap();
        let mut profiles = Profiles::open(dir.path());
        let padded = format!("{}# {}\n", profile::DEFAULT_TOML, "x".repeat(MOST_BYTES));
        assert!(!Profile::unchosen(&padded).is_empty(), "pads to choose");
        let refused = profiles.put("couch", &padded).unwrap_err();
        assert_eq!(refused.code, ErrorCode::InvalidProfile);
        assert!(refused.message.contains("at most 65536 bytes"));
        assert!(profiles.profile("couch").is_none());

        fs::write(dir.path().join("large.toml"), &padded).unwrap();
        fs::write(dir.path().join("edge.toml"), "#".repeat(MOST_BYTES)).unwrap();
        let profiles = Profiles::open(dir.path());
        assert_eq!(
            profiles.names().collect::<Vec<_>>(),
            ["default", "edge"],
            "left out, and the file as it is"
        );
        assert!(dir.path().join("large.toml").exists());
    }

    #[test]
    fn a_profile_that_cannot_be_written_or_removed_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let mut profiles = Profiles::open(dir.path());
        fs::create_dir(dir.path().join("couch.toml.partial")).unwrap();
        let failed = profiles.put("couch", RIGHT).unwrap_err();
        assert_eq!(failed.code, ErrorCode::Internal);
        assert_eq!(profiles.names().count(), 1);

        profiles.put("desk", RIGHT).unwrap();
        fs::remove_file(dir.path().join("desk.toml")).unwrap();
        let failed = profiles.delete("desk").unwrap_err();
        assert_eq!(failed.code, ErrorCode::Internal);
        assert_eq!(profiles.names().collect::<Vec<_>>(), ["default", "desk"]);
    }
}
