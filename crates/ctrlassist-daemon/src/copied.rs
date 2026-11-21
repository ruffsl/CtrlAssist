//! What has been copied from a connected pad, for a virtual pad to pass
//! as one: `identities/dualsense.toml` in the daemon's state directory.
//!
//! A DualSense's report descriptor and firmware report are read from a
//! real one, once, and kept here, so the identity is offered from then
//! on whether or not that pad is connected. Nothing of them ships with
//! CtrlAssist. A file that can't be read is left alone, and the identity
//! isn't offered until a pad is copied again.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use ctrlassist_device::DualSenseData;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::durable;

/// The schema this release writes and reads.
const SCHEMA: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Stored {
    schema: u32,
    /// The report descriptor, as hex digits.
    descriptor: String,
    /// The firmware report, as hex digits.
    firmware: String,
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

fn from_hex(text: &str) -> Option<Vec<u8>> {
    if !text.is_ascii() || !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

fn parse(text: &str) -> Option<DualSenseData> {
    let stored: Stored = toml::from_str(text).ok()?;
    let data = DualSenseData {
        descriptor: from_hex(&stored.descriptor)?,
        firmware: from_hex(&stored.firmware)?,
    };
    (stored.schema == SCHEMA && data.is_whole()).then_some(data)
}

/// What has been copied, as kept on disk.
#[derive(Debug)]
pub(crate) struct Copied {
    path: PathBuf,
    dualsense: Option<DualSenseData>,
}

impl Copied {
    /// Reads what an earlier run copied, if it left anything whole.
    pub(crate) fn open(state: &Path) -> Self {
        let path = state.join("identities").join("dualsense.toml");
        let dualsense = match fs::read_to_string(&path) {
            Ok(text) => {
                let data = parse(&text);
                if data.is_none() {
                    warn!(path = %path.display(), "the copied DualSense can't be read; the identity isn't offered until one is copied again");
                }
                data
            }
            Err(_) => None,
        };
        Self { path, dualsense }
    }

    /// The data a virtual DualSense is made from, once one was copied.
    pub(crate) fn dualsense(&self) -> Option<&DualSenseData> {
        self.dualsense.as_ref()
    }

    /// Keeps a connected DualSense's data, for this run and later ones.
    pub(crate) fn keep(&mut self, data: DualSenseData) {
        let stored = Stored {
            schema: SCHEMA,
            descriptor: to_hex(&data.descriptor),
            firmware: to_hex(&data.firmware),
        };
        let written = toml::to_string(&stored)
            .map_err(std::io::Error::other)
            .and_then(|text| {
                if let Some(directory) = self.path.parent() {
                    fs::create_dir_all(directory)?;
                }
                durable::write(&self.path, &text)
            });
        match written {
            Ok(()) => info!(path = %self.path.display(), "a connected DualSense was copied"),
            Err(error) => {
                warn!(path = %self.path.display(), %error, "the copied DualSense couldn't be saved; it is kept until the daemon stops");
            }
        }
        self.dualsense = Some(data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> DualSenseData {
        let mut firmware = vec![0u8; 64];
        firmware[0] = 0x20;
        firmware[44] = 0x15;
        DualSenseData {
            descriptor: vec![0x06, 0x00, 0xff, 0xc0],
            firmware,
        }
    }

    #[test]
    fn what_is_copied_is_there_for_a_later_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut copied = Copied::open(dir.path());
        assert!(copied.dualsense().is_none(), "nothing was ever copied");
        copied.keep(data());
        assert_eq!(copied.dualsense(), Some(&data()));
        assert_eq!(Copied::open(dir.path()).dualsense(), Some(&data()));
    }

    #[test]
    fn a_file_that_is_not_whole_offers_nothing_and_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("identities/dualsense.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        for wrong in [
            "not toml at all [",
            "schema = 2\ndescriptor = \"06\"\nfirmware = \"20\"\n",
            "schema = 1\ndescriptor = \"0g\"\nfirmware = \"20\"\n",
            "schema = 1\ndescriptor = \"06\"\nfirmware = \"2\"\n",
            "schema = 1\ndescriptor = \"06\"\nfirmware = \"20\"\n",
            "schema = 1\ndescriptor = \"é6\"\nfirmware = \"20\"\n",
        ] {
            fs::write(&path, wrong).unwrap();
            assert!(Copied::open(dir.path()).dualsense().is_none(), "{wrong}");
            assert_eq!(fs::read_to_string(&path).unwrap(), wrong);
        }
    }

    #[test]
    fn what_cannot_be_saved_is_kept_for_this_run() {
        let dir = tempfile::tempdir().unwrap();
        // Where the directory should go there is a file.
        fs::write(dir.path().join("identities"), "").unwrap();
        let mut copied = Copied::open(dir.path());
        copied.keep(data());
        assert_eq!(copied.dualsense(), Some(&data()));
    }
}
