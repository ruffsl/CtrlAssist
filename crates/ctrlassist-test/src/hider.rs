//! A stand-in for the hiding service.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use ctrlassist_device::PadId;
use ctrlassist_hider::{Refusal, Reply, Request, Token, receive, send};
use rustix::net::{
    AddressFamily, Shutdown, SocketAddrUnix, SocketType, accept, bind, connect, listen, shutdown,
    socket,
};

/// What the stand-in was asked, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asked {
    /// To hide the pad known by this id.
    Hide(String),
    /// To open a hidden pad again after it reconnected.
    Reopen(String),
    /// To restore a pad.
    Restore(String),
    /// For a pad's raw HID node.
    Raw(String),
    /// For the raw HID node of a pad it has hidden.
    RawOf(String),
    /// Nothing more: the client's connection closed, which restores
    /// whatever it still had hidden.
    Closed,
}

/// A hiding service that speaks the real one's protocol on a socket of
/// its own and hides nothing: it hands back the pad's node opened for
/// reading and writing, as the real service does, and notes what it was
/// asked. The real service needs root; the virtual machine check runs it.
pub struct FakeHider {
    path: PathBuf,
    asked: Arc<Mutex<Vec<Asked>>>,
    /// The client being served, to hang up on as the stand-in goes.
    client: Arc<Mutex<Option<OwnedFd>>>,
    serving: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for FakeHider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeHider")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

/// The pad a descriptor is open on: its node, and what it is known by.
fn pad_of(fd: &OwnedFd) -> Option<(PathBuf, PadId)> {
    let node = std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd())).ok()?;
    let found = ctrlassist_device::connected()
        .ok()?
        .into_iter()
        .find(|found| found.node == node)?;
    Some((node, found.identifiers.id()))
}

/// The raw HID node beside an event node, for a pad with a HID device.
#[must_use]
pub fn raw_node_of(event: &Path) -> Option<PathBuf> {
    let name = event.file_name()?;
    let beside = Path::new("/sys/class/input")
        .join(name)
        .join("device/device/hidraw");
    let raw = std::fs::read_dir(beside).ok()?.next()?.ok()?;
    Some(Path::new("/dev").join(raw.file_name()))
}

fn open(node: &Path) -> Option<OwnedFd> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(node)
        .ok()
        .map(OwnedFd::from)
}

/// Serves one connection until it closes.
fn serve(connection: &OwnedFd, asked: &Mutex<Vec<Asked>>) {
    let note = |what: Asked| {
        asked
            .lock()
            .expect("nothing panics while the log is held")
            .push(what);
    };
    let mut hidden: BTreeMap<u32, PadId> = BTreeMap::new();
    let mut tokens = 0u32;
    loop {
        let mut buffer = [0u8; Request::SIZE];
        let Ok((length, fds)) = receive(connection.as_fd(), &mut buffer) else {
            break;
        };
        if length == 0 {
            break;
        }
        let mut pass = None;
        let reply = match Request::decode(&buffer[..length]) {
            Some(Request::Hide) => match fds.first().and_then(pad_of) {
                Some((node, id)) => {
                    note(Asked::Hide(id.to_string()));
                    tokens += 1;
                    hidden.insert(tokens, id);
                    pass = open(&node);
                    Reply {
                        refusal: pass.is_none().then_some(Refusal::Failed),
                        token: Token(tokens),
                        dislodged: false,
                    }
                }
                None => Reply::refused(Refusal::NotAPad),
            },
            Some(Request::Reopen(token)) => match hidden.get(&token.0) {
                Some(id) => {
                    note(Asked::Reopen(id.to_string()));
                    let node = ctrlassist_device::connected()
                        .ok()
                        .and_then(|pads| {
                            pads.into_iter().find(|found| found.identifiers.id() == *id)
                        })
                        .map(|found| found.node);
                    pass = node.as_deref().and_then(open);
                    match pass {
                        Some(_) => Reply {
                            refusal: None,
                            token,
                            dislodged: false,
                        },
                        None => Reply::refused(Refusal::Absent),
                    }
                }
                None => Reply::refused(Refusal::UnknownToken),
            },
            Some(Request::Restore(token)) => match hidden.remove(&token.0) {
                Some(id) => {
                    note(Asked::Restore(id.to_string()));
                    Reply {
                        refusal: None,
                        token,
                        dislodged: false,
                    }
                }
                None => Reply::refused(Refusal::UnknownToken),
            },
            // The real service opens it for the kinds of pad it names;
            // the stand-in for any pad whose raw node this user can open.
            Some(Request::Raw) => match fds.first().and_then(pad_of) {
                Some((node, id)) => {
                    note(Asked::Raw(id.to_string()));
                    pass = raw_node_of(&node).and_then(|raw| open(&raw));
                    match pass {
                        Some(_) => Reply {
                            refusal: None,
                            token: Token(0),
                            dislodged: false,
                        },
                        None => Reply::refused(Refusal::Unsupported),
                    }
                }
                None => Reply::refused(Refusal::NotAPad),
            },
            Some(Request::RawOf(token)) => match hidden.get(&token.0) {
                Some(id) => {
                    note(Asked::RawOf(id.to_string()));
                    let node = ctrlassist_device::connected()
                        .ok()
                        .and_then(|pads| {
                            pads.into_iter().find(|found| found.identifiers.id() == *id)
                        })
                        .map(|found| found.node);
                    pass = node
                        .as_deref()
                        .and_then(raw_node_of)
                        .and_then(|raw| open(&raw));
                    match pass {
                        Some(_) => Reply {
                            refusal: None,
                            token,
                            dislodged: false,
                        },
                        None => Reply::refused(Refusal::Unsupported),
                    }
                }
                None => Reply::refused(Refusal::UnknownToken),
            },
            None => Reply::refused(Refusal::BadRequest),
        };
        let fds: Vec<_> = pass.iter().map(AsFd::as_fd).collect();
        if send(connection.as_fd(), &reply.encode(), &fds).is_err() {
            break;
        }
    }
    note(Asked::Closed);
}

impl FakeHider {
    /// Listens at `path`, serving one client at a time.
    ///
    /// # Errors
    ///
    /// Fails if the socket can't be made there.
    pub fn listen(path: &Path) -> io::Result<Self> {
        let listener = socket(AddressFamily::UNIX, SocketType::SEQPACKET, None)?;
        bind(&listener, &SocketAddrUnix::new(path)?)?;
        listen(&listener, 4)?;
        let asked = Arc::new(Mutex::new(Vec::new()));
        let client = Arc::new(Mutex::new(None));
        let serving = std::thread::Builder::new()
            .name("fake-hider".into())
            .spawn({
                let asked = Arc::clone(&asked);
                let client = Arc::clone(&client);
                move || {
                    while let Ok(connection) = accept(&listener) {
                        // An empty message is the stand-in's own, asking
                        // it to stop.
                        let mut first = [0u8; 1];
                        let stop = rustix::net::recv(
                            &connection,
                            &mut first,
                            rustix::net::RecvFlags::PEEK,
                        )
                        .is_ok_and(|(length, _)| length == 0);
                        if stop {
                            return;
                        }
                        if let Ok(mut held) = client.lock() {
                            *held = connection.try_clone().ok();
                        }
                        serve(&connection, &asked);
                    }
                }
            })?;
        Ok(Self {
            path: path.to_path_buf(),
            asked,
            client,
            serving: Some(serving),
        })
    }

    /// Everything it was asked so far, in order.
    ///
    /// # Panics
    ///
    /// Panics if the serving thread panicked while noting one.
    #[must_use]
    pub fn asked(&self) -> Vec<Asked> {
        self.asked
            .lock()
            .expect("nothing panics while the log is held")
            .clone()
    }
}

impl Drop for FakeHider {
    fn drop(&mut self) {
        // A connection that says nothing and closes its sending side
        // reads as an empty first message.
        if let Ok(stopper) = socket(AddressFamily::UNIX, SocketType::SEQPACKET, None)
            && let Ok(address) = SocketAddrUnix::new(&self.path)
            && connect(&stopper, &address).is_ok()
        {
            let _ = shutdown(&stopper, Shutdown::Write);
        }
        // A service that stops hangs up on its client, which is how the
        // client finds out.
        if let Ok(mut held) = self.client.lock()
            && let Some(client) = held.take()
        {
            let _ = shutdown(&client, Shutdown::Both);
        }
        drop(self.serving.take());
        let _ = std::fs::remove_file(&self.path);
    }
}
