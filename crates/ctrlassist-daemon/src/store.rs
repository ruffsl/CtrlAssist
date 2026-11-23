//! The known pads on disk: `pads.toml` in the daemon's state directory.
//!
//! The file is the daemon's own, rewritten whole and durably on each
//! change. One that can't be read (damaged, or written by a later
//! release) is left alone: the daemon starts with no remembered pads and
//! doesn't write, so nothing is lost by running an older release once.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::book::Known;
use crate::durable;

/// The schema this release writes and reads.
const SCHEMA: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Stored {
    schema: u32,
    #[serde(default, rename = "pad", skip_serializing_if = "Vec::is_empty")]
    pads: Vec<StoredPad>,
}

#[derive(Serialize, Deserialize)]
struct StoredPad {
    id: String,
    name: String,
    /// Vendor and product, as `054c:0ce6`.
    model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rumble: Option<bool>,
}

fn model(text: &str) -> Option<(u16, u16)> {
    let (vendor, product) = text.split_once(':')?;
    Some((
        u16::from_str_radix(vendor, 16).ok()?,
        u16::from_str_radix(product, 16).ok()?,
    ))
}

/// Where the known pads are kept.
#[derive(Clone, Debug)]
pub(crate) struct Store {
    path: PathBuf,
    /// Cleared when the file on disk couldn't be understood.
    writable: bool,
}

impl Store {
    /// Opens the store in `directory` and reads what it holds.
    pub(crate) fn open(directory: &Path) -> (Self, Vec<Known>) {
        let path = directory.join("pads.toml");
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => {
                let missing = error.kind() == io::ErrorKind::NotFound;
                if !missing {
                    warn!(path = %path.display(), %error, "the known pads can't be read; starting without them, and leaving the file as it is");
                }
                return (
                    Self {
                        path,
                        writable: missing,
                    },
                    Vec::new(),
                );
            }
        };
        match toml::from_str::<Stored>(&text) {
            Ok(stored) if stored.schema == SCHEMA => {
                let known = stored
                    .pads
                    .into_iter()
                    .filter_map(|pad| {
                        let (vendor, product) = model(&pad.model)?;
                        Some(Known {
                            id: pad.id,
                            name: pad.name,
                            vendor,
                            product,
                            rumble: pad.rumble,
                        })
                    })
                    .collect();
                (
                    Self {
                        path,
                        writable: true,
                    },
                    known,
                )
            }
            outcome => {
                let reason = match outcome {
                    Ok(stored) => format!(
                        "it is schema {}, and this release reads {SCHEMA}",
                        stored.schema
                    ),
                    Err(error) => error.to_string(),
                };
                warn!(path = %path.display(), reason, "the known pads can't be understood; starting without them, and leaving the file as it is");
                (
                    Self {
                        path,
                        writable: false,
                    },
                    Vec::new(),
                )
            }
        }
    }

    /// Writes `known` in place of what the file held, durably: a crash
    /// leaves the old file or the new one, never part of either.
    pub(crate) fn save(&self, known: &[Known]) -> io::Result<()> {
        if !self.writable {
            return Ok(());
        }
        let stored = Stored {
            schema: SCHEMA,
            pads: known
                .iter()
                .map(|known| StoredPad {
                    id: known.id.clone(),
                    name: known.name.clone(),
                    model: format!("{:04x}:{:04x}", known.vendor, known.product),
                    rumble: known.rumble,
                })
                .collect(),
        };
        let text = toml::to_string(&stored).map_err(io::Error::other)?;
        durable::write(&self.path, &text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(id: &str, rumble: Option<bool>) -> Known {
        Known {
            id: id.to_string(),
            name: "A \"pad\" = [odd]\nname".to_string(),
            vendor: 0x054c,
            product: 0x0ce6,
            rumble,
        }
    }

    #[test]
    fn what_is_saved_is_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("made").join("on-demand");
        let (store, known) = Store::open(&state);
        assert!(known.is_empty());
        let pads = vec![pad("address:aa", Some(true)), pad("serial:1:2:x", None)];
        store.save(&pads).unwrap();
        assert_eq!(Store::open(&state).1, pads);
        assert!(!state.join("pads.toml.partial").exists());
        let text = fs::read_to_string(state.join("pads.toml")).unwrap();
        assert!(text.contains("schema = 1"), "{text}");
        assert!(text.contains("model = \"054c:0ce6\""), "{text}");

        store.save(&[]).unwrap();
        assert!(Store::open(&state).1.is_empty());
    }

    #[test]
    fn a_file_that_is_not_understood_is_left_as_it_is() {
        for (case, text) in [
            (
                "a later schema",
                "schema = 2\n[[pad]]\nid = \"x\"\nfuture = true\n",
            ),
            ("damage", "schema = 1\n[[pad"),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("pads.toml");
            fs::write(&path, text).unwrap();
            let (store, known) = Store::open(dir.path());
            assert!(known.is_empty(), "{case}");
            store.save(&[pad("address:aa", None)]).unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), text, "{case}");
        }
    }

    #[test]
    fn a_file_that_cannot_be_read_is_left_as_it_is() {
        // A directory where the file should be: reading it fails, and so
        // would writing over it.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("pads.toml")).unwrap();
        let (store, known) = Store::open(dir.path());
        assert!(known.is_empty());
        store.save(&[pad("address:aa", None)]).unwrap();
        assert!(dir.path().join("pads.toml").is_dir());
    }

    #[test]
    fn a_pad_whose_model_is_damaged_is_skipped_and_the_rest_kept() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("pads.toml"),
            "schema = 1\n[[pad]]\nid = \"a\"\nname = \"A\"\nmodel = \"zz\"\n[[pad]]\nid = \"b\"\nname = \"B\"\nmodel = \"0001:0002\"\n",
        )
        .unwrap();
        let known = Store::open(dir.path()).1;
        assert_eq!(known.len(), 1);
        assert_eq!(
            (known[0].id.as_str(), known[0].vendor, known[0].product),
            ("b", 1, 2)
        );
    }

    #[test]
    fn a_save_that_cannot_be_written_says_so() {
        // Something sits where the new file is written before it takes
        // the old one's place.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("pads.toml.partial")).unwrap();
        let (store, _) = Store::open(dir.path());
        assert!(store.save(&[]).is_err());
        assert!(!dir.path().join("pads.toml").exists());
    }
}
