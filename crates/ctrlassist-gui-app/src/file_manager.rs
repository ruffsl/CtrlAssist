//! "Reveal in file manager" helper. Calls
//! `org.freedesktop.FileManager1.ShowItems` so the user's chosen
//! file manager opens the parent directory and pre-selects the
//! given path. Falls back to opening the parent directory via
//! `webbrowser::open` when no FileManager1 implementation is
//! reachable on the user bus.
//!
//! Spec: <https://www.freedesktop.org/wiki/Specifications/file-manager-interface/>.
//! Implementations include Dolphin, Nautilus, Nemo, Thunar, Caja, PCManFM-Qt.

use anyhow::{Context, Result};
use std::path::Path;
use tracing::{debug, warn};
use zbus::Connection;

const SERVICE: &str = "org.freedesktop.FileManager1";
const PATH: &str = "/org/freedesktop/FileManager1";
const INTERFACE: &str = "org.freedesktop.FileManager1";

/// Reveal `path` in the user's file manager. The parent directory is
/// opened and the entry at `path` is pre-selected. Logs a warning and
/// returns `Err` on failure; callers typically log and move on
/// (mirroring the rest of the GUI's "best-effort, never panic" launch
/// affordances).
pub(crate) async fn reveal(connection: &Connection, path: &Path) -> Result<()> {
    let uri = path_to_uri(path)?;
    debug!(uri = %uri, "FileManager1.ShowItems");

    let result = connection
        .call_method(
            Some(SERVICE),
            PATH,
            Some(INTERFACE),
            "ShowItems",
            &(vec![uri.as_str()], ""),
        )
        .await;

    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            warn!(error = %e, "FileManager1.ShowItems failed; falling back to xdg-open on parent");
            let parent = path.parent().unwrap_or(path);
            let parent_uri = path_to_uri(parent)?;
            webbrowser::open(&parent_uri)
                .with_context(|| format!("xdg-open fallback failed for {parent_uri}"))?;
            Ok(())
        }
    }
}

/// Format a `file://` URI from an absolute filesystem path. Refuses
/// non-UTF-8 paths (xdg URIs require UTF-8) and non-absolute paths.
fn path_to_uri(path: &Path) -> Result<String> {
    let path_str = path
        .to_str()
        .with_context(|| format!("path is not valid UTF-8: {path:?}"))?;
    if !path.is_absolute() {
        anyhow::bail!("path must be absolute: {path_str}");
    }
    let encoded: String = path_str
        .bytes()
        .map(|b| match b {
            b'/' | b'-' | b'_' | b'.' | b'~' | b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    Ok(format!("file://{encoded}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn ascii_path_uri() {
        let p = PathBuf::from("/home/user/.config/ctrlassist/config.toml");
        assert_eq!(
            path_to_uri(&p).unwrap(),
            "file:///home/user/.config/ctrlassist/config.toml"
        );
    }

    #[test]
    fn percent_encodes_spaces() {
        let p = PathBuf::from("/home/user/My Files/x.toml");
        assert_eq!(
            path_to_uri(&p).unwrap(),
            "file:///home/user/My%20Files/x.toml"
        );
    }

    #[test]
    fn percent_encodes_reserved_uri_bytes() {
        let path = PathBuf::from("/tmp/a%?#.toml");
        assert_eq!(path_to_uri(&path).unwrap(), "file:///tmp/a%25%3F%23.toml");
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_utf8_paths() {
        use std::os::unix::ffi::OsStringExt;

        let path = PathBuf::from(std::ffi::OsString::from_vec(vec![b'/', 0xff]));
        assert!(path_to_uri(&path).is_err());
    }

    #[test]
    fn rejects_relative() {
        assert!(path_to_uri(Path::new("relative/path")).is_err());
    }
}
