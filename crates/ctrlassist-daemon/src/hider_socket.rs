//! Hearing the hiding service's socket made, or made to take
//! connections, so that the daemon looks for the service again then, if it
//! isn't known to be there, and not only when it next needs it: a pad grabbed for want of the service
//! is hidden once it is back.
//!
//! systemd makes a socket unit's socket by binding it, which makes its
//! file, then listening on it, and then touching the file, so that
//! whoever waits for it to take connections hears so: a connection made
//! between the bind and the listen is refused, and the touch follows.
//! The service run by hand with `--listen` does the same. So the socket's
//! directory is watched for the socket's file being made, moved there or
//! touched. The directory may not be there yet, as before the package is
//! installed, or be made again: then the nearest directory above it that
//! is there is watched for the next one down being made, and once it is,
//! the watch moves down to it.
//!
//! The watch has a thread of its own, which sleeps until the kernel or
//! the daemon has something for it.

use std::io;
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};
use std::thread::JoinHandle;

use rustix::event::{EventfdFlags, PollFd, PollFlags, eventfd, poll};
use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, warn};

use crate::hotplug::Seen;

/// Where the socket is watched for.
#[derive(Debug)]
struct Watched {
    inotify: OwnedFd,
    socket: PathBuf,
    /// The directory watched, and its watch: the socket's own, or the
    /// nearest above it that is there. None where no directory could be
    /// watched, and then nothing is heard.
    dir: Option<(i32, PathBuf)>,
}

/// What one read of the watch came to.
#[derive(Debug, Default)]
struct Heard {
    /// The socket may take connections where it didn't.
    made: bool,
    /// The watch is to move: to the directory below, made, or above, as
    /// the one watched went; or afresh, as events were lost.
    move_watch: bool,
}

impl Watched {
    /// Watches for the socket at `socket` being made. A relative path is
    /// taken from the daemon's working directory, as connecting to it
    /// takes it: a bare file name has an empty directory, which can't be
    /// watched.
    fn new(socket: &Path) -> io::Result<Self> {
        let socket = std::path::absolute(socket).unwrap_or_else(|_| socket.to_path_buf());
        let mut watched = Self {
            inotify: inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?,
            socket,
            dir: None,
        };
        watched.place();
        Ok(watched)
    }

    /// Watches the socket's directory, or the nearest directory above it
    /// that is there. Returns whether the socket is there now: made
    /// before the watch was, it is heard of no other way.
    fn place(&mut self) -> bool {
        if let Some((watch, _)) = self.dir.take() {
            // Gone already where its directory went.
            let _ = inotify::remove_watch(&self.inotify, watch);
        }
        let own = self.socket.parent();
        let mut dir = own;
        while let Some(at) = dir {
            let mut flags = WatchFlags::CREATE
                | WatchFlags::MOVED_TO
                | WatchFlags::DELETE_SELF
                | WatchFlags::MOVE_SELF
                | WatchFlags::ONLYDIR;
            if Some(at) == own {
                flags |= WatchFlags::ATTRIB;
            }
            match inotify::add_watch(&self.inotify, at, flags) {
                Ok(watch) => {
                    debug!(dir = %at.display(), "watching for the hiding service's socket");
                    self.dir = Some((watch, at.to_path_buf()));
                    break;
                }
                // Not there yet: the one above is watched for it.
                Err(rustix::io::Errno::NOENT | rustix::io::Errno::NOTDIR) => {}
                // There, and not to be watched, as for want of permission
                // or of watches left: a watch above would wait for it to
                // be made, which it never is.
                Err(error) => {
                    warn!(dir = %at.display(), %error, "the hiding service's socket's directory can't be watched; the daemon won't notice the service appearing, and looks for it only when it is next needed");
                    return self.socket.symlink_metadata().is_ok();
                }
            }
            dir = at.parent();
        }
        if self.dir.is_none() {
            warn!(socket = %self.socket.display(), "nothing above the hiding service's socket can be watched; it is looked for only when it is next needed");
        }
        self.socket.symlink_metadata().is_ok()
    }

    /// The name, in the directory watched, of what is to be made there
    /// next: the socket itself, in its own directory; or the next
    /// directory down towards it.
    fn next_down(&self, watched: &Path) -> Option<&[u8]> {
        let below = self.socket.strip_prefix(watched).ok()?;
        match below.components().next()? {
            Component::Normal(name) => Some(name.as_bytes()),
            _ => None,
        }
    }

    /// What `event` says.
    fn heard(&self, event: &inotify::Event<'_>, heard: &mut Heard) {
        let events = event.events();
        if events.contains(ReadFlags::QUEUE_OVERFLOW) {
            // Anything may have been lost: the socket made among it.
            heard.made = true;
            heard.move_watch = true;
            return;
        }
        let Some((watch, dir)) = &self.dir else {
            return;
        };
        // What a watch given up still said.
        if event.wd() != *watch {
            return;
        }
        if events.intersects(ReadFlags::DELETE_SELF | ReadFlags::MOVE_SELF | ReadFlags::IGNORED) {
            heard.move_watch = true;
            return;
        }
        let Some(name) = event.file_name() else {
            return;
        };
        if self.next_down(dir) != Some(name.to_bytes()) {
            return;
        }
        let own = self.socket.parent() == Some(dir.as_path());
        if own && events.intersects(ReadFlags::CREATE | ReadFlags::MOVED_TO | ReadFlags::ATTRIB) {
            heard.made = true;
        } else if !own && events.intersects(ReadFlags::CREATE | ReadFlags::MOVED_TO) {
            heard.move_watch = true;
        }
    }

    /// Reads what the kernel has queued, and moves the watch if that
    /// says to. Returns whether the socket may take connections where it
    /// didn't: its file made, moved into place or touched, as systemd
    /// touches it once it listens; found there as the watch moved down to
    /// its directory; or events lost.
    fn read(&mut self) -> io::Result<bool> {
        let mut heard = Heard::default();
        let mut buffer = [MaybeUninit::uninit(); 4096];
        let mut events = inotify::Reader::new(&self.inotify, &mut buffer);
        loop {
            match events.next() {
                Ok(event) => self.heard(&event, &mut heard),
                Err(rustix::io::Errno::WOULDBLOCK) => break,
                Err(rustix::io::Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
        }
        if heard.move_watch {
            heard.made |= self.place();
        }
        Ok(heard.made)
    }
}

/// The running watch on where the hiding service's socket is made.
#[must_use = "dropping the watch stops it, and the hiding service's socket made is no longer heard"]
#[derive(Debug)]
pub(crate) struct SocketWatch {
    wake: OwnedFd,
    thread: Option<JoinHandle<()>>,
}

impl SocketWatch {
    /// Starts watching for the socket at `socket` being made, or made to
    /// take connections, and says each time to `seen`. The watch is in
    /// place once this returns.
    pub(crate) fn start(socket: &Path, seen: UnboundedSender<Seen>) -> io::Result<Self> {
        let mut watched = Watched::new(socket)?;
        let wake = eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?;
        let stop = wake.try_clone()?;
        let thread = std::thread::Builder::new()
            .name("hider-socket".into())
            .spawn(move || {
                loop {
                    let mut fds = [
                        PollFd::new(&watched.inotify, PollFlags::IN),
                        PollFd::new(&stop, PollFlags::IN),
                    ];
                    match poll(&mut fds, None) {
                        Ok(_) => {}
                        Err(rustix::io::Errno::INTR) => continue,
                        // Any other stays, and waiting again would spin.
                        Err(error) => {
                            warn!(%error, "the hiding service's socket can't be waited on any more; it is looked for only when it is next needed");
                            return;
                        }
                    }
                    if !fds[1].revents().is_empty() {
                        return;
                    }
                    match watched.read() {
                        Ok(false) => {}
                        Ok(true) => {
                            if seen.send(Seen::HiderSocket).is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            warn!(%error, "the hiding service's socket can't be watched any more; it is looked for only when it is next needed");
                            return;
                        }
                    }
                }
            })?;
        Ok(Self {
            wake,
            thread: Some(thread),
        })
    }
}

impl Drop for SocketWatch {
    fn drop(&mut self) {
        let _ = rustix::io::write(self.wake.as_fd(), &1u64.to_ne_bytes());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use rustix::event::Timespec;
    use rustix::fs::{AtFlags, CWD, Timestamps, UTIME_NOW, utimensat};
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketType, bind, connect, listen, socket};
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;

    /// Long enough for an event that came to have been heard; how long
    /// only bounds how long a failing test takes to fail.
    const SETTLE: Timespec = Timespec {
        tv_sec: 0,
        tv_nsec: 200_000_000,
    };

    fn a_dir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("ca")
            .tempdir_in("/tmp")
            .unwrap()
    }

    /// Whether the watch hears the socket made, within a moment.
    fn hears(watched: &mut Watched) -> bool {
        loop {
            let mut fds = [PollFd::new(&watched.inotify, PollFlags::IN)];
            if poll(&mut fds, Some(&SETTLE)).unwrap() == 0 {
                return false;
            }
            if watched.read().unwrap() {
                return true;
            }
        }
    }

    /// A socket bound at `path` and not yet listened on, as systemd's is
    /// between its bind and its listen.
    fn bound(path: &Path) -> OwnedFd {
        let listener = socket(AddressFamily::UNIX, SocketType::SEQPACKET, None).unwrap();
        bind(&listener, &SocketAddrUnix::new(path).unwrap()).unwrap();
        listener
    }

    fn connects(path: &Path) -> bool {
        let client = socket(AddressFamily::UNIX, SocketType::SEQPACKET, None).unwrap();
        connect(&client, &SocketAddrUnix::new(path).unwrap()).is_ok()
    }

    /// Touches `path`, as systemd does once its socket listens.
    fn touch(path: &Path) {
        let now = Timespec {
            tv_sec: 0,
            tv_nsec: UTIME_NOW,
        };
        let times = Timestamps {
            last_access: now,
            last_modification: now,
        };
        utimensat(CWD, path, &times, AtFlags::empty()).unwrap();
    }

    /// The socket made is heard; a connection between its bind and its
    /// listen is refused, and the touch after the listen is heard too,
    /// after which a connection is taken. Nothing else in the directory
    /// is heard.
    #[test]
    fn a_socket_made_and_then_listened_on_is_heard_each_time() {
        let dir = a_dir();
        let path = dir.path().join("hider.sock");
        let mut watched = Watched::new(&path).unwrap();
        assert!(!hears(&mut watched), "nothing yet");
        std::fs::write(dir.path().join("other"), "").unwrap();
        touch(&dir.path().join("other"));
        assert!(!hears(&mut watched), "another file is nothing");
        let listener = bound(&path);
        assert!(hears(&mut watched), "made");
        assert!(!connects(&path), "not listening yet");
        listen(&listener, 1).unwrap();
        touch(&path);
        assert!(hears(&mut watched), "touched once it listens");
        assert!(connects(&path));
    }

    /// With the socket's directory not there yet, the directory above is
    /// watched until it is made; and made again, it is watched afresh.
    #[test]
    fn a_directory_made_later_or_made_again_is_watched_once_it_is() {
        let dir = a_dir();
        let own = dir.path().join("ctrlassist");
        let path = own.join("hider.sock");
        let mut watched = Watched::new(&path).unwrap();
        std::fs::create_dir(dir.path().join("elsewhere")).unwrap();
        assert!(!hears(&mut watched), "another directory is nothing");
        std::fs::create_dir(&own).unwrap();
        assert!(!hears(&mut watched), "the directory alone is nothing");
        let listener = bound(&path);
        assert!(hears(&mut watched), "made in it");

        drop(listener);
        std::fs::remove_dir_all(&own).unwrap();
        assert!(!hears(&mut watched), "gone is nothing");
        std::fs::create_dir(&own).unwrap();
        let _listener = bound(&path);
        assert!(hears(&mut watched), "made in it again");
    }

    /// A socket made before its directory's watch is in place, as one
    /// made just as the directory is, is found as the watch moves to it.
    #[test]
    fn a_socket_made_with_its_directory_is_found_there() {
        let dir = a_dir();
        let own = dir.path().join("ctrlassist");
        let path = own.join("hider.sock");
        let mut watched = Watched::new(&path).unwrap();
        std::fs::create_dir(&own).unwrap();
        let _listener = bound(&path);
        assert!(hears(&mut watched));
    }

    /// The running watch says each socket made to the daemon, and stops
    /// as it is dropped.
    #[tokio::test]
    async fn the_running_watch_says_so_and_stops_when_dropped() {
        // A bound on each wait, past which the test fails rather than
        // hangs: what passes never waits for it.
        const BOUND: std::time::Duration = std::time::Duration::from_secs(10);
        let dir = a_dir();
        let path = dir.path().join("hider.sock");
        let (seen, mut said) = unbounded_channel();
        let watch = SocketWatch::start(&path, seen).unwrap();
        let _listener = bound(&path);
        let heard = tokio::time::timeout(BOUND, said.recv())
            .await
            .expect("the socket made is said");
        assert!(matches!(heard, Some(Seen::HiderSocket)));
        drop(watch);
        let after = tokio::time::timeout(BOUND, said.recv())
            .await
            .expect("the watch stops as it is dropped");
        assert!(after.is_none(), "its sender gone with it");
    }

    /// A socket given by a bare file name is in the daemon's working
    /// directory, which is watched for it, as connecting to it finds it
    /// there.
    #[test]
    fn a_socket_given_relative_is_watched_for_where_it_is_connected_to() {
        let watched = Watched::new(Path::new("hider.sock")).unwrap();
        let here = std::env::current_dir().unwrap();
        assert_eq!(watched.socket, here.join("hider.sock"));
        assert_eq!(watched.dir.map(|(_, dir)| dir), Some(here));
    }

    /// A socket's directory that is there and can't be watched, here for
    /// want of permission to read it, isn't waited for from above, where
    /// it would never be made: nothing is watched.
    #[test]
    fn a_directory_there_that_can_t_be_watched_is_not_waited_for_from_above() {
        use std::os::unix::fs::PermissionsExt;

        let dir = a_dir();
        let own = dir.path().join("ctrlassist");
        std::fs::create_dir(&own).unwrap();
        std::fs::set_permissions(&own, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Read all the same by a user who may read anything.
        if std::fs::read_dir(&own).is_ok() {
            return;
        }
        let watched = Watched::new(&own.join("hider.sock")).unwrap();
        // Readable again, for the directory to be removed with the test.
        std::fs::set_permissions(&own, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(watched.dir.is_none(), "{:?}", watched.dir);
    }
}
