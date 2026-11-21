//! Files written so that a crash leaves the old one or the new one,
//! never part of either.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;

fn directory(path: &Path) -> io::Result<&Path> {
    path.parent()
        .ok_or_else(|| io::Error::other("the file has no directory"))
}

/// Writes `text` as the file at `path`, in place of whatever was there,
/// making its directory if need be.
pub(crate) fn write(path: &Path, text: &str) -> io::Result<()> {
    let directory = directory(path)?;
    fs::create_dir_all(directory)?;
    let mut partial = path.as_os_str().to_owned();
    partial.push(".partial");
    let partial = Path::new(&partial);
    let mut file = File::create(partial)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    fs::rename(partial, path)?;
    // The rename is durable once the directory is.
    File::open(directory)?.sync_all()
}

/// Removes the file at `path`.
pub(crate) fn remove(path: &Path) -> io::Result<()> {
    fs::remove_file(path)?;
    File::open(directory(path)?)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_written_whole_replaced_whole_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("made/on-demand/file.toml");
        write(&path, "one").unwrap();
        write(&path, "two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        let beside: Vec<_> = fs::read_dir(path.parent().unwrap()).unwrap().collect();
        assert_eq!(beside.len(), 1, "nothing partial is left: {beside:?}");
        remove(&path).unwrap();
        assert!(!path.exists());
        assert!(remove(&path).is_err());
    }

    #[test]
    fn a_write_that_cannot_be_made_says_so_and_leaves_what_was_there() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.toml");
        write(&path, "kept").unwrap();
        // Something sits where the new file is written before it takes
        // the old one's place.
        fs::create_dir(dir.path().join("file.toml.partial")).unwrap();
        assert!(write(&path, "lost").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "kept");
        assert!(write(Path::new("/"), "x").is_err());
        assert!(remove(Path::new("/")).is_err());
    }
}
