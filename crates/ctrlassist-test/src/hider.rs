//! A stand-in for the hiding service.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
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
    /// To give up the hide asked last on a connection.
    Cancel,
    /// Nothing more: a connection closed with a pad still hidden through
    /// it, which restores it. One that closes with nothing hidden says
    /// nothing.
    Closed,
}

/// What a hide the stand-in [holds](FakeHider::holding) comes to once
/// it is cancelled, as the real service's would, by how far it had got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cancelled {
    /// The cancel came before the pad was re-bound: nothing was done.
    Untouched,
    /// The cancel came as the pad was being re-bound: the hide ends, and
    /// a second re-bind shows the pad again. Two goings and comings back.
    Undone,
    /// The hide was done before the cancel was read, which then shows
    /// the pad again. Two goings and comings back, one for each reply.
    DoneFirst,
}

/// How a [scripted](FakeHider::scripted) stand-in answers, beside
/// speaking the real service's protocol and counting as it does. It hides
/// whatever descriptor a hide carries, needing no pad, and hands that
/// descriptor back as the pad's node. What it holds, it holds until it is
/// [let go](FakeHider::let_go).
#[derive(Clone, Debug, Default)]
pub struct Script {
    /// How many of the first requests it is asked, on any connection, it
    /// refuses as from a caller not in front of a seat, with no count, as
    /// a service refuses at its gate.
    pub refuse: usize,
    /// Holds each hide until it is cancelled, and then answers it and the
    /// cancel as the real service would have, by how far it had got. None
    /// answers each hide at once, the pad hidden.
    pub hold_hides: Option<Cancelled>,
    /// How many re-binds a hide answered at once says it made.
    pub hide_rebinds: u8,
    /// How many re-binds a restore, or a cancel that shows a pad again,
    /// says it made.
    pub restore_rebinds: u8,
    /// How many of the first restores it holds.
    pub hold_restores: usize,
    /// How many of the first reopens and openings of a hidden pad's raw
    /// node it holds.
    pub hold_opens: usize,
}

/// Where a scripted stand-in has got to.
#[derive(Debug, Default)]
struct Progress {
    /// How many requests it has refused.
    refused: usize,
    /// How many restores, and how many opens, it has been asked.
    restores: usize,
    opens: usize,
    /// How many requests it holds now.
    held: usize,
    /// Whether what it holds may go.
    let_go: bool,
}

/// What [`FakeHider::scripted`] answers by, and where it has got to.
#[derive(Debug)]
struct Hold {
    script: Script,
    state: Mutex<Progress>,
    changed: Condvar,
}

impl Hold {
    fn state(&self) -> std::sync::MutexGuard<'_, Progress> {
        self.state
            .lock()
            .expect("nothing panics while the stand-in's hold is held")
    }

    /// Waits until `until` holds of the hold's state.
    fn wait(&self, until: impl Fn(&Progress) -> bool) {
        let mut state = self.state();
        while !until(&state) {
            state = self
                .changed
                .wait(state)
                .expect("nothing panics while the stand-in's hold is held");
        }
    }

    fn set(&self, change: impl FnOnce(&mut Progress)) {
        change(&mut self.state());
        self.changed.notify_all();
    }

    /// Holds a request until let go, if `hold` says to.
    fn hold(&self, hold: bool) {
        if hold {
            self.set(|state| state.held += 1);
            self.wait(|state| state.let_go);
            self.set(|state| state.held -= 1);
        }
    }

    /// Whether the next request is refused, counting it if it is.
    fn refuses(&self) -> bool {
        let mut state = self.state();
        let refuses = state.refused < self.script.refuse;
        state.refused += usize::from(refuses);
        refuses
    }
}

/// A hiding service that speaks the real one's protocol on a socket of
/// its own and hides nothing: it hands back the pad's node opened for
/// reading and writing, as the real service does, and notes what it was
/// asked. It counts, as the real service does: never a re-bind, since
/// it makes none. The real service runs as its own user with the rule
/// shipped with it installed, which a test can't arrange; the virtual
/// machine check runs it.
pub struct FakeHider {
    path: PathBuf,
    asked: Arc<Mutex<Vec<Asked>>>,
    /// Set for a scripted stand-in.
    hold: Option<Arc<Hold>>,
    /// The connections being served, each on a thread of its own as the
    /// real service serves them, to hang up on as the stand-in goes.
    clients: Arc<Mutex<Vec<OwnedFd>>>,
    /// Set as it goes, for the accepting thread to stop.
    stopping: Arc<AtomicBool>,
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

/// A reply that did as asked, re-binding nothing, as the stand-in never
/// does.
fn done(token: Token) -> Reply {
    Reply {
        refusal: None,
        token,
        dislodged: false,
        rebinds: Some(0),
    }
}

/// A reply of a service that counts, that re-bound the pad `rebinds`
/// times for the request.
fn counted(reply: Reply, rebinds: u8) -> Reply {
    Reply {
        rebinds: Some(rebinds),
        ..reply
    }
}

/// Serves one connection of a [scripted](FakeHider::scripted) stand-in.
fn serve_scripted(connection: &OwnedFd, asked: &Mutex<Vec<Asked>>, hold: &Hold) {
    let note = |what: Asked| {
        asked
            .lock()
            .expect("nothing panics while the log is held")
            .push(what);
    };
    let next = || {
        let mut buffer = [0u8; Request::SIZE];
        let (length, fds) = receive(connection.as_fd(), &mut buffer).ok()?;
        Some((Request::decode(buffer.get(..length)?)?, fds))
    };
    let answer = |reply: Reply, fds: &[std::os::fd::BorrowedFd<'_>]| {
        send(connection.as_fd(), &reply.encode(), fds).is_ok()
    };
    let script = &hold.script;
    // Each pad hidden, by token, with the descriptor that came with it.
    let mut hidden: BTreeMap<u32, OwnedFd> = BTreeMap::new();
    let mut tokens = 0u32;
    // The pad the last hide hid, while it is: what a cancel shows again.
    let mut last_hidden = None;
    while let Some((request, mut fds)) = next() {
        if hold.refuses() {
            if answer(Reply::refused(Refusal::NotPermitted), &[]) {
                continue;
            }
            break;
        }
        let answered = match request {
            Request::Version => answer(
                done(Token(ctrlassist_hider::pack_release(env!(
                    "CARGO_PKG_VERSION"
                )))),
                &[],
            ),
            Request::Hide if fds.is_empty() => answer(Reply::refused(Refusal::BadRequest), &[]),
            Request::Hide => match script.hold_hides {
                Some(outcome) => {
                    note(Asked::Hide(String::new()));
                    hold.set(|state| state.held += 1);
                    // Nothing more comes until the cancel, which ends it.
                    let Some((Request::Cancel, _)) = next() else {
                        break;
                    };
                    note(Asked::Cancel);
                    hold.wait(|state| state.let_go);
                    hold.set(|state| state.held -= 1);
                    let cancelled = counted(Reply::refused(Refusal::Cancelled), 0);
                    let (hide, cancel) = match outcome {
                        Cancelled::Untouched => (cancelled, counted(done(Token(0)), 0)),
                        Cancelled::Undone => (counted(cancelled, 2), counted(done(Token(0)), 0)),
                        Cancelled::DoneFirst => {
                            (counted(done(Token(1)), 1), counted(done(Token(0)), 1))
                        }
                    };
                    // A hide done passes the pad's descriptor back, as the
                    // real service passes the new node's.
                    let pass: Vec<_> = match outcome {
                        Cancelled::DoneFirst => fds.iter().map(AsFd::as_fd).collect(),
                        Cancelled::Untouched | Cancelled::Undone => Vec::new(),
                    };
                    answer(hide, &pass) && answer(cancel, &[])
                }
                None => {
                    note(Asked::Hide(String::new()));
                    tokens += 1;
                    let fd = fds.swap_remove(0);
                    let reply = Reply {
                        dislodged: true,
                        ..counted(done(Token(tokens)), script.hide_rebinds)
                    };
                    let answered = answer(reply, &[fd.as_fd()]);
                    hidden.insert(tokens, fd);
                    last_hidden = Some(tokens);
                    answered
                }
            },
            Request::Restore(token) => match hidden.remove(&token.0) {
                Some(_) => {
                    note(Asked::Restore(String::new()));
                    if last_hidden == Some(token.0) {
                        last_hidden = None;
                    }
                    let restores = {
                        let mut state = hold.state();
                        state.restores += 1;
                        state.restores
                    };
                    hold.hold(restores <= script.hold_restores);
                    answer(counted(done(token), script.restore_rebinds), &[])
                }
                None => answer(Reply::refused(Refusal::UnknownToken), &[]),
            },
            Request::Reopen(token) | Request::RawOf(token) => match hidden.get(&token.0) {
                Some(fd) => {
                    note(match request {
                        Request::Reopen(_) => Asked::Reopen(String::new()),
                        _ => Asked::RawOf(String::new()),
                    });
                    let opens = {
                        let mut state = hold.state();
                        state.opens += 1;
                        state.opens
                    };
                    hold.hold(opens <= script.hold_opens);
                    answer(done(token), &[fd.as_fd()])
                }
                None => answer(Reply::refused(Refusal::UnknownToken), &[]),
            },
            Request::Cancel => {
                note(Asked::Cancel);
                let shown = last_hidden.take().and_then(|token| hidden.remove(&token));
                if shown.is_some() {
                    note(Asked::Restore(String::new()));
                }
                let rebinds = if shown.is_some() {
                    script.restore_rebinds
                } else {
                    0
                };
                answer(counted(done(Token(0)), rebinds), &[])
            }
            Request::Raw => answer(Reply::refused(Refusal::Unsupported), &[]),
        };
        if !answered {
            break;
        }
    }
    if !hidden.is_empty() {
        note(Asked::Closed);
    }
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
    // The pad the last hide hid, while it is: what a cancel shows again.
    let mut last_hidden = None;
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
            Some(Request::Version) => done(Token(ctrlassist_hider::pack_release(env!(
                "CARGO_PKG_VERSION"
            )))),
            Some(Request::Hide) => match fds.first().and_then(pad_of) {
                Some((node, id)) => {
                    note(Asked::Hide(id.to_string()));
                    tokens += 1;
                    hidden.insert(tokens, id);
                    last_hidden = Some(tokens);
                    pass = open(&node);
                    Reply {
                        refusal: pass.is_none().then_some(Refusal::Failed),
                        ..done(Token(tokens))
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
                        Some(_) => done(token),
                        None => Reply::refused(Refusal::Absent),
                    }
                }
                None => Reply::refused(Refusal::UnknownToken),
            },
            Some(Request::Restore(token)) => match hidden.remove(&token.0) {
                Some(id) => {
                    note(Asked::Restore(id.to_string()));
                    done(token)
                }
                None => Reply::refused(Refusal::UnknownToken),
            },
            // Shows again what the last hide hid, as the real service does.
            Some(Request::Cancel) => {
                note(Asked::Cancel);
                if let Some(id) = last_hidden.take().and_then(|token| hidden.remove(&token)) {
                    note(Asked::Restore(id.to_string()));
                }
                done(Token(0))
            }
            // The real service opens it for the kinds of pad it names;
            // the stand-in for any pad whose raw node this user can open.
            Some(Request::Raw) => match fds.first().and_then(pad_of) {
                Some((node, id)) => {
                    note(Asked::Raw(id.to_string()));
                    pass = raw_node_of(&node).and_then(|raw| open(&raw));
                    match pass {
                        Some(_) => done(Token(0)),
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
                        Some(_) => done(token),
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
    if !hidden.is_empty() {
        note(Asked::Closed);
    }
}

impl FakeHider {
    /// Listens at `path`, serving each connection on a thread of its
    /// own, as the real service does.
    ///
    /// # Errors
    ///
    /// Fails if the socket can't be made there.
    pub fn listen(path: &Path) -> io::Result<Self> {
        Self::serving(path, None)
    }

    /// Listens at `path` as a service that counts, and holds every hide
    /// it is asked, whatever descriptor comes with it, until it is
    /// cancelled: then, once [let go](Self::let_go), it answers the hide
    /// and the cancel as the real service would have, by `outcome`. It
    /// needs no pad.
    ///
    /// # Errors
    ///
    /// Fails if the socket can't be made there.
    pub fn holding(path: &Path, outcome: Cancelled) -> io::Result<Self> {
        Self::scripted(
            path,
            Script {
                hold_hides: Some(outcome),
                ..Script::default()
            },
        )
    }

    /// Listens at `path` as a service that counts, answering as `script`
    /// says.
    ///
    /// # Errors
    ///
    /// Fails if the socket can't be made there.
    pub fn scripted(path: &Path, script: Script) -> io::Result<Self> {
        let hold = Hold {
            script,
            state: Mutex::default(),
            changed: Condvar::new(),
        };
        Self::serving(path, Some(Arc::new(hold)))
    }

    /// Waits until it holds a request.
    ///
    /// # Panics
    ///
    /// Panics if the stand-in isn't scripted.
    pub fn wait_held(&self) {
        self.hold
            .as_ref()
            .expect("only a scripted stand-in is waited on to hold something")
            .wait(|state| state.held > 0);
    }

    /// Lets what it holds go, now or once it comes.
    pub fn let_go(&self) {
        if let Some(hold) = &self.hold {
            hold.set(|state| state.let_go = true);
        }
    }

    fn serving(path: &Path, hold: Option<Arc<Hold>>) -> io::Result<Self> {
        let listener = socket(AddressFamily::UNIX, SocketType::SEQPACKET, None)?;
        bind(&listener, &SocketAddrUnix::new(path)?)?;
        listen(&listener, 16)?;
        let asked = Arc::new(Mutex::new(Vec::new()));
        let clients = Arc::new(Mutex::new(Vec::new()));
        let stopping = Arc::new(AtomicBool::new(false));
        let serving = std::thread::Builder::new()
            .name("fake-hider".into())
            .spawn({
                let asked = Arc::clone(&asked);
                let clients = Arc::clone(&clients);
                let stopping = Arc::clone(&stopping);
                let hold = hold.clone();
                move || {
                    while let Ok(connection) = accept(&listener) {
                        // Woken by the stand-in itself, to stop.
                        if stopping.load(Ordering::Relaxed) {
                            return;
                        }
                        if let Ok(mut held) = clients.lock()
                            && let Ok(held_too) = connection.try_clone()
                        {
                            held.push(held_too);
                        }
                        let asked = Arc::clone(&asked);
                        let hold = hold.clone();
                        let served = std::thread::Builder::new()
                            .name("fake-hider-client".into())
                            .spawn(move || match hold {
                                Some(hold) => serve_scripted(&connection, &asked, &hold),
                                None => serve(&connection, &asked),
                            });
                        drop(served);
                    }
                }
            })?;
        Ok(Self {
            path: path.to_path_buf(),
            asked,
            hold,
            clients,
            stopping,
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
        // Nothing it holds is held any longer.
        self.let_go();
        // A connection of its own wakes the accepting thread to stop.
        self.stopping.store(true, Ordering::Relaxed);
        if let Ok(stopper) = socket(AddressFamily::UNIX, SocketType::SEQPACKET, None)
            && let Ok(address) = SocketAddrUnix::new(&self.path)
        {
            let _ = connect(&stopper, &address);
        }
        // A service that stops hangs up on its clients, which is how
        // each finds out.
        if let Ok(mut held) = self.clients.lock() {
            for client in held.drain(..) {
                let _ = shutdown(&client, Shutdown::Both);
            }
        }
        drop(self.serving.take());
        let _ = std::fs::remove_file(&self.path);
    }
}
