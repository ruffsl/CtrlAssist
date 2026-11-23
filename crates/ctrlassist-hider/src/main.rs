//! The hiding service: hides pads from games for the user's daemon and
//! hands the daemon open descriptors for them, and does nothing else.
//!
//! It runs as a user of its own, with no capabilities. What it may do
//! comes from the udev rule shipped with it: name pads to hide by marker
//! files in its own directory, ask udev to re-bind a pad's driver, and
//! open the nodes of pads it has hidden. It is small on purpose, with no
//! async runtime and none of the control-plane crates, so that it can be
//! read in one sitting.
//!
//! Each connection has a thread of its own, so a client that stops
//! reading its replies, or passes a descriptor whose closing blocks,
//! holds up nobody but itself. What is hidden is shared, behind one lock
//! that no thread holds while it waits on a client or on udev. A
//! connection owns the pads it has hidden; when it closes, for any
//! reason, they are shown again.

mod activation;
mod descriptor;
mod hide;
mod pad;
mod seat;

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex, MutexGuard};

use ctrlassist_hider::{Refusal, Reply, Request, Token, pack_release, receive, send};
use rustix::event::{EventfdFlags, PollFd, PollFlags, eventfd, poll};
use rustix::io::Errno;
use rustix::net::sockopt::socket_peercred;
use rustix::net::{
    AddressFamily, SocketAddrUnix, SocketFlags, SocketType, accept_with, bind, listen, socket,
};

use crate::pad::Pad;

/// The most connections one user may hold at once. The daemon holds one.
const CONNECTIONS_PER_USER: usize = 8;

/// Where a hidden pad is in its life.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    /// Its markers are made and its driver is being re-bound.
    Hiding,
    Hidden,
    /// Its markers are gone and its driver is being re-bound to show it.
    Showing,
}

/// A pad a connection has hidden.
#[derive(Debug)]
struct Entry {
    token: u32,
    /// The connection that hid it.
    owner: u64,
    pad: Pad,
    phase: Phase,
}

/// What every connection shares.
#[derive(Debug, Default)]
struct State {
    entries: Vec<Entry>,
    next_token: u32,
    next_connection: u64,
    /// Open connections, by user; `u32::MAX` for one whose user isn't known.
    connections: BTreeMap<u32, usize>,
    /// Whether anyone has connected since the service started.
    served_anyone: bool,
}

impl State {
    /// A token no hidden pad has.
    fn new_token(&mut self) -> u32 {
        loop {
            let token = self.next_token.max(1);
            self.next_token = token.wrapping_add(1);
            if !self.entries.iter().any(|entry| entry.token == token) {
                return token;
            }
        }
    }

    /// The markers of every pad still hidden or being hidden.
    fn markers(&self) -> BTreeSet<String> {
        self.entries
            .iter()
            .filter(|entry| entry.phase != Phase::Showing)
            .flat_map(|entry| entry.pad.markers.iter().cloned())
            .collect()
    }

    /// The device of every pad the service has, in whatever phase.
    fn held(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .map(|entry| entry.pad.parent.clone())
            .collect()
    }

    fn open(&self) -> usize {
        self.connections.values().sum()
    }
}

#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
    /// Written when a connection closes, which may let the service stop,
    /// or let it accept again after running out of descriptors.
    closed: OwnedFd,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("nothing panics while the service's state is held")
    }
}

/// Writes a line to the journal. A failed write is ignored: there is
/// nowhere else to report it.
fn log(line: &str) {
    let _ = writeln!(io::stderr(), "{line}");
}

/// The group the rule gives hidden nodes to, which the service runs as.
fn group() -> u32 {
    rustix::process::getegid().as_raw()
}

fn reply(socket: BorrowedFd<'_>, reply: Reply, fd: Option<&OwnedFd>) {
    let fds: Vec<_> = fd.iter().map(|fd| fd.as_fd()).collect();
    // A client that has gone is found out by its next receive. One that
    // doesn't read blocks this send, and so its own thread alone.
    let _ = send(socket, &reply.encode(), &fds);
}

fn refuse(socket: BorrowedFd<'_>, refusal: Refusal) {
    reply(socket, Reply::refused(refusal), None);
}

fn ok(token: u32, dislodged: bool) -> Reply {
    Reply {
        refusal: None,
        token: Token(token),
        dislodged,
    }
}

/// Shows a pad again: the markers no other pad has go, its driver is
/// re-bound if it is there, and then any pad the rule hides that nothing
/// should is shown again too. The entry stays, as showing, until the
/// re-bind is done, so the pad can't be hidden again meanwhile.
fn show(shared: &Shared, token: u32) {
    let parent = {
        let mut state = shared.state();
        let Some(entry) = state.entries.iter_mut().find(|entry| entry.token == token) else {
            return;
        };
        entry.phase = Phase::Showing;
        let markers = entry.pad.markers.clone();
        let parent = pad::is_there(&entry.pad).then(|| entry.pad.parent.clone());
        let remaining = state.markers();
        for marker in markers.difference(&remaining) {
            hide::delete(marker);
        }
        parent
    };
    if let Some(parent) = parent {
        let _ = hide::rebind(&parent, None);
    }
    let mut state = shared.state();
    state.entries.retain(|entry| entry.token != token);
    let held = state.held();
    let held: Vec<&Path> = held.iter().map(PathBuf::as_path).collect();
    hide::sweep(group(), &held, &state.markers());
}

/// The pad a descriptor names, if the caller may have it hidden or its
/// raw node opened: the descriptor was opened for reading on a pad the
/// seat user could open, and the caller is in front, now, on the seat the
/// pad is on. A caller who has left the foreground takes nothing new.
fn presented(user: Option<u32>, fd: BorrowedFd<'_>) -> Result<Pad, Refusal> {
    let pad = pad::identify(fd)?;
    let seats = user.map(seat::seats_in_front).unwrap_or_default();
    if !seats.contains(&pad.identity.seat) {
        return Err(Refusal::NotPermitted);
    }
    Ok(pad)
}

/// Hides the pad `fd` is open on, for connection `owner`.
fn hide_pad(
    shared: &Shared,
    socket: BorrowedFd<'_>,
    owner: u64,
    user: Option<u32>,
    fd: OwnedFd,
) -> Result<(u32, OwnedFd), Refusal> {
    let pad = presented(user, fd.as_fd())?;
    let token = {
        let mut state = shared.state();
        if state
            .entries
            .iter()
            .any(|entry| entry.pad.parent == pad.parent)
        {
            return Err(Refusal::Busy);
        }
        let token = state.new_token();
        // Made before the re-bind they take effect at: from here on a
        // crash leaves enough to undo.
        let others = state.markers();
        for marker in &pad.markers {
            if hide::create(marker).is_err() {
                for made in pad.markers.difference(&others) {
                    hide::delete(made);
                }
                return Err(Refusal::Failed);
            }
        }
        state.entries.push(Entry {
            token,
            owner,
            pad: pad.clone(),
            phase: Phase::Hiding,
        });
        token
    };
    // The caller's own descriptor is closed before the driver is re-bound,
    // so the service isn't holding the pad open itself.
    drop(fd);
    match hide::cut_off(&pad, socket) {
        Ok(event) => {
            let mut state = shared.state();
            if let Some(entry) = state.entries.iter_mut().find(|entry| entry.token == token) {
                entry.phase = Phase::Hidden;
            }
            Ok((token, event))
        }
        Err(refusal) => {
            show(shared, token);
            Err(refusal)
        }
    }
}

/// Opens a hidden pad's event node again, finding the pad first if it
/// reconnected: only once the device it was checked as is gone.
fn reopen(shared: &Shared, owner: u64, token: u32) -> Result<OwnedFd, Refusal> {
    let mut state = shared.state();
    let held = state.held();
    let entry = state
        .entries
        .iter_mut()
        .find(|entry| entry.token == token && entry.owner == owner && entry.phase == Phase::Hidden)
        .ok_or(Refusal::UnknownToken)?;
    if !pad::is_there(&entry.pad) {
        let taken: Vec<&Path> = held.iter().map(PathBuf::as_path).collect();
        let found = pad::returned(&entry.pad, group(), &taken).ok_or(Refusal::Absent)?;
        for marker in found.markers.difference(&entry.pad.markers) {
            let _ = hide::create(marker);
        }
        entry.pad.parent = found.parent;
        entry.pad.ordinal = found.ordinal;
        entry.pad.markers.extend(found.markers);
    }
    let node = pad::event_node(&entry.pad.parent, entry.pad.ordinal).ok_or(Refusal::Absent)?;
    drop(state);
    hide::open(&node)
}

/// The device of a pad connection `owner` has hidden.
fn hidden_parent(shared: &Shared, owner: u64, token: u32) -> Result<PathBuf, Refusal> {
    shared
        .state()
        .entries
        .iter()
        .find(|entry| entry.token == token && entry.owner == owner && entry.phase == Phase::Hidden)
        .map(|entry| entry.pad.parent.clone())
        .ok_or(Refusal::UnknownToken)
}

/// Serves one client until it closes, then shows again what it hid.
fn serve(shared: &Shared, socket: &OwnedFd, owner: u64, user: Option<u32>) {
    let socket = socket.as_fd();
    // A client found to be in front stays permitted: one that later
    // leaves the foreground keeps what it has.
    let mut permitted = user.is_some_and(seat::is_in_front);
    loop {
        let mut buffer = [0u8; Request::SIZE];
        let Ok((length, mut fds)) = receive(socket, &mut buffer) else {
            break;
        };
        if length == 0 {
            break;
        }
        // Every descriptor received is closed on this thread, and any but
        // the one a request carries is closed now, before anything is
        // locked: closing one can block, as a FUSE file's flush does.
        fds.truncate(1);
        let fd = fds.pop();
        if !permitted {
            permitted = user.is_some_and(seat::is_in_front);
        }
        if !permitted {
            refuse(socket, Refusal::NotPermitted);
            continue;
        }
        match buffer.get(..length).and_then(Request::decode) {
            Some(Request::Version) => {
                reply(
                    socket,
                    ok(pack_release(env!("CARGO_PKG_VERSION")), false),
                    None,
                );
            }
            Some(Request::Hide) => {
                let Some(fd) = fd else {
                    refuse(socket, Refusal::BadRequest);
                    continue;
                };
                match hide_pad(shared, socket, owner, user, fd) {
                    Ok((token, event)) => {
                        log(&format!("hid pad {token}"));
                        // Every pad hidden is cut off by a re-bind, and
                        // shown again by another.
                        reply(socket, ok(token, true), Some(&event));
                    }
                    Err(refusal) => refuse(socket, refusal),
                }
            }
            Some(Request::Reopen(Token(token))) => match reopen(shared, owner, token) {
                Ok(event) => reply(socket, ok(token, true), Some(&event)),
                Err(refusal) => refuse(socket, refusal),
            },
            Some(Request::Restore(Token(token))) => {
                if hidden_parent(shared, owner, token).is_ok() {
                    show(shared, token);
                    log(&format!("showed pad {token} again"));
                    reply(socket, ok(token, false), None);
                } else {
                    refuse(socket, Refusal::UnknownToken);
                }
            }
            Some(Request::Raw) => {
                let Some(fd) = fd else {
                    refuse(socket, Refusal::BadRequest);
                    continue;
                };
                let raw = presented(user, fd.as_fd())
                    .and_then(|pad| pad::raw_node(&pad.parent))
                    .and_then(|node| hide::open(&node));
                match raw {
                    Ok(raw) => reply(socket, ok(0, false), Some(&raw)),
                    Err(refusal) => refuse(socket, refusal),
                }
            }
            Some(Request::RawOf(Token(token))) => {
                let raw = hidden_parent(shared, owner, token)
                    .and_then(|parent| pad::raw_node(&parent))
                    .and_then(|node| hide::open(&node));
                match raw {
                    Ok(raw) => reply(socket, ok(token, false), Some(&raw)),
                    Err(refusal) => refuse(socket, refusal),
                }
            }
            None => refuse(socket, Refusal::BadRequest),
        }
    }
    let mine: Vec<u32> = shared
        .state()
        .entries
        .iter()
        .filter(|entry| entry.owner == owner && entry.phase != Phase::Showing)
        .map(|entry| entry.token)
        .collect();
    for token in mine {
        log(&format!("client gone; showing pad {token} again"));
        show(shared, token);
    }
}

/// Serves clients until none is left, or forever if `stay` is set.
fn run(listener: &OwnedFd, stay: bool) -> io::Result<()> {
    // Nothing is connected yet, so anything still hidden was left by a
    // service that died.
    hide::reconcile(group());

    let shared = Arc::new(Shared {
        state: Mutex::new(State::default()),
        closed: eventfd(0, EventfdFlags::CLOEXEC | EventfdFlags::NONBLOCK)?,
    });
    // Out of descriptors, accepting is left until a connection closes.
    let mut exhausted = false;
    loop {
        let mut fds = vec![PollFd::new(&shared.closed, PollFlags::IN)];
        if !exhausted {
            fds.push(PollFd::new(listener, PollFlags::IN));
        }
        match poll(&mut fds, None) {
            Ok(_) | Err(Errno::INTR) => {}
            Err(error) => return Err(error.into()),
        }
        let closed = !fds[0].revents().is_empty();
        let waiting = fds.get(1).is_some_and(|fd| !fd.revents().is_empty());
        drop(fds);
        if closed {
            let mut count = [0u8; 8];
            let _ = rustix::io::read(&shared.closed, &mut count);
            exhausted = false;
        }
        if !waiting {
            let state = shared.state();
            if state.open() == 0 && state.served_anyone && !stay {
                // Started on demand, and nobody needs it now. A client
                // that connects from here on waits on the socket systemd
                // holds, which starts the service again.
                return Ok(());
            }
            continue;
        }
        let socket = match accept_with(listener, SocketFlags::CLOEXEC) {
            Ok(socket) => socket,
            Err(Errno::MFILE | Errno::NFILE | Errno::NOBUFS | Errno::NOMEM) => {
                exhausted = true;
                continue;
            }
            Err(error) => {
                log(&format!("couldn't accept a connection: {error}"));
                continue;
            }
        };
        let user = socket_peercred(&socket).ok().map(|peer| peer.uid.as_raw());
        let owner = {
            let mut state = shared.state();
            state.served_anyone = true;
            let count = state
                .connections
                .entry(user.unwrap_or(u32::MAX))
                .or_default();
            if *count >= CONNECTIONS_PER_USER {
                // Closed at once: one user can't take every descriptor.
                continue;
            }
            *count += 1;
            state.next_connection += 1;
            state.next_connection
        };
        let client = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("client".into())
            .spawn(move || {
                serve(&client, &socket, owner, user);
                drop(socket);
                let mut state = client.state();
                let key = user.unwrap_or(u32::MAX);
                if let Some(count) = state.connections.get_mut(&key) {
                    *count -= 1;
                    if *count == 0 {
                        state.connections.remove(&key);
                    }
                }
                drop(state);
                let _ = rustix::io::write(&client.closed, &1u64.to_ne_bytes());
            });
        if spawned.is_err() {
            let mut state = shared.state();
            if let Some(count) = state.connections.get_mut(&user.unwrap_or(u32::MAX)) {
                *count -= 1;
            }
            log("couldn't start a thread for a connection");
        }
    }
}

/// A listening socket at `path`, for running outside systemd.
fn listen_at(path: &Path) -> io::Result<OwnedFd> {
    let _ = std::fs::remove_file(path);
    let socket = socket(AddressFamily::UNIX, SocketType::SEQPACKET, None)?;
    bind(&socket, &SocketAddrUnix::new(path)?)?;
    listen(&socket, 8)?;
    // Anyone may connect, as with the installed socket; who is served is
    // decided per connection.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))?;
    Ok(socket)
}

/// The service's own user, never root: its power is what the rule gives
/// its group, and root's is everything.
fn unprivileged() -> io::Result<()> {
    if rustix::process::geteuid().is_root() || rustix::process::getegid().is_root() {
        return Err(io::Error::other(
            "the hiding service runs as its own user, ctrlassist-hider, not as root",
        ));
    }
    Ok(())
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let outcome = match arguments.as_slice() {
        // Started by systemd on a client's first connection.
        [] => unprivileged().and_then(|()| match activation::listener() {
            Some(listener) => run(&listener, false),
            None => Err(io::Error::other(
                "not started by a systemd socket; use --listen <path>",
            )),
        }),
        ["--listen", path] => unprivileged()
            .and_then(|()| listen_at(Path::new(path)))
            .and_then(|listener| run(&listener, true)),
        // Undo whatever is still hidden: run by the unit after the service
        // stops, so a service that died doesn't leave pads hidden until
        // the next one starts.
        ["reconcile"] => unprivileged().map(|()| hide::reconcile(group())),
        // What the service makes of a device node, without hiding it.
        ["check", node] => std::fs::File::open(node).and_then(|file| {
            let verdict = match pad::identify(file.as_fd()) {
                Ok(pad) => format!("pad {pad:?}"),
                Err(refusal) => format!("refused: {refusal}"),
            };
            writeln!(io::stdout(), "{verdict}")
        }),
        _ => Err(io::Error::other(
            "usage: ctrlassist-hider [--listen <path> | reconcile | check <node>]",
        )),
    };
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log(&format!("ctrlassist-hider: {error}"));
            ExitCode::FAILURE
        }
    }
}
