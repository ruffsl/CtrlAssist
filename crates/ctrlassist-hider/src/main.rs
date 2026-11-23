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
mod connections;
mod descriptor;
mod hide;
mod in_flight;
mod pad;
mod seat;

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
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
use tracing::{debug, error, info, warn};

use crate::connections::Connections;
use crate::hide::Leave;
use crate::pad::{Device, Pad};

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
    connections: Connections,
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
            .map(|entry| entry.pad.parent.path.clone())
            .collect()
    }
}

#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
    /// Written when a connection closes, which may let the service stop,
    /// or let it accept again after running out of room to.
    closed: OwnedFd,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .expect("nothing panics while the service's state is held")
    }
}

/// The group the rule gives hidden nodes to, which the service runs as.
fn group() -> u32 {
    rustix::process::getegid().as_raw()
}

fn reply(socket: BorrowedFd<'_>, reply: Reply, fd: Option<&OwnedFd>) {
    let fds: Vec<_> = fd.iter().map(|fd| fd.as_fd()).collect();
    // A client that has gone is found out by its next receive. One that
    // doesn't read blocks this send, and so its own thread alone.
    if let Err(error) = send(socket, &reply.encode(), &fds) {
        debug!(%error, "couldn't reply to a client, which has likely gone");
    }
}

/// A refusal of a request that re-bound nothing. It says so: every reply
/// of a service that counts carries a count, refusals included.
fn refuse(socket: BorrowedFd<'_>, refusal: Refusal) {
    reply(socket, refused(refusal, 0), None);
}

/// A reply that did as asked, saying how many times the pad was re-bound
/// for it.
fn ok(token: u32, dislodged: bool, rebinds: u8) -> Reply {
    Reply {
        refusal: None,
        token: Token(token),
        dislodged,
        rebinds: Some(rebinds),
    }
}

/// A refusal, saying how many times the pad was re-bound all the same.
fn refused(refusal: Refusal, rebinds: u8) -> Reply {
    Reply {
        rebinds: Some(rebinds),
        ..Reply::refused(refusal)
    }
}

/// Shows a pad again: the markers no other pad has go, its driver is
/// re-bound if it is there and `rebind` says to, as it does unless the
/// pad's hide never asked for a re-bind, and then any pad the rule hides
/// that nothing should is shown again too. The entry stays, as showing,
/// until the re-bind is done, so the pad can't be hidden again meanwhile;
/// the wait for it ends when udev is done, the pad is unplugged, or
/// `client`, the one that asked, hangs up; but not before udev has
/// answered the request, so a hide of the pad once the entry has gone is
/// never judged on this re-bind. Returns how many times the pad was
/// re-bound for it, which it goes and comes back for, as
/// [`hide::Rebound::went`] counts.
fn show(shared: &Arc<Shared>, token: u32, client: Option<BorrowedFd<'_>>, rebind: bool) -> u8 {
    let parent = {
        let mut state = shared.state();
        let Some(entry) = state.entries.iter_mut().find(|entry| entry.token == token) else {
            return 0;
        };
        entry.phase = Phase::Showing;
        let markers = entry.pad.markers.clone();
        let parent = (rebind && entry.pad.parent.is_there()).then(|| entry.pad.parent.clone());
        let remaining = state.markers();
        for marker in markers.difference(&remaining) {
            hide::delete(marker);
        }
        parent
    };
    let mut rebinds = 0;
    if let Some(parent) = parent {
        let asker = client.map_or(hide::Asker::Nobody, hide::Asker::HangsUp);
        let rebound = hide::rebind(&parent, Leave::AsTheRuleSays, asker);
        rebinds = u8::from(rebound.went);
        if let Err(refusal) = rebound.outcome {
            warn!(
                token,
                device = %parent.path.display(),
                %refusal,
                "the pad's driver wasn't re-bound to show it again; it is shown at its next event"
            );
        }
    }
    shared.state().entries.retain(|entry| entry.token != token);
    sweep(shared);
    rebinds
}

/// Shows again what the rule hides that nothing should, each device on a
/// thread of its own that waits for its turn, re-binds it and waits for
/// udev's answer: tracked as every re-bind is, so the next re-bind of the
/// device waits for this one. Doesn't wait itself.
fn sweep(shared: &Arc<Shared>) {
    let (held, markers) = {
        let state = shared.state();
        (state.held(), state.markers())
    };
    let held: Vec<&Path> = held.iter().map(PathBuf::as_path).collect();
    for (device, _) in hide::unnamed(group(), &held, &markers) {
        let theirs = Arc::clone(shared);
        let device_theirs = device.clone();
        let spawned = std::thread::Builder::new()
            .name("sweep".into())
            .spawn(move || sweep_one(&theirs, &device_theirs));
        if let Err(error) = spawned {
            warn!(device = %device.path.display(), %error, "couldn't start a thread to show a pad again; showing it here");
            sweep_one(shared, &device);
        }
    }
}

/// Re-binds one device the sweep found, once its turn has come, if it is
/// still hidden and named by nothing: a hide of it since would be cut off
/// by this re-bind, and another sweep may have shown it already.
fn sweep_one(shared: &Shared, device: &Device) {
    let unnamed = || {
        let (held, markers) = {
            let state = shared.state();
            (state.held(), state.markers())
        };
        let held: Vec<&Path> = held.iter().map(PathBuf::as_path).collect();
        let still = hide::unnamed(group(), &held, &markers)
            .iter()
            .any(|(other, _)| other.path == device.path);
        if still { Ok(()) } else { Err(Refusal::Busy) }
    };
    let rebound = hide::rebind_if(device, Leave::AsTheRuleSays, hide::Asker::Nobody, unnamed);
    match rebound.outcome {
        Ok(()) => debug!(device = %device.path.display(), "showed again a pad no marker names"),
        Err(Refusal::Busy) => {}
        Err(refusal) => warn!(
            device = %device.path.display(),
            %refusal,
            "couldn't show again a pad no marker names; it stays hidden until it is plugged in again"
        ),
    }
}

/// The seats `user` is in front on now, as a person at the machine.
fn seats_in_front(user: Option<u32>) -> Vec<String> {
    user.map(seat::seats_in_front).unwrap_or_default()
}

/// The pad a descriptor names, if the caller may have it hidden or its
/// raw node opened: the descriptor was opened for reading on a pad the
/// seat user could open, and the caller is in front, now, on the seat the
/// pad is on. A caller who has left the foreground takes nothing new.
fn presented(user: Option<u32>, fd: BorrowedFd<'_>) -> Result<Pad, Refusal> {
    let pad = pad::identify(fd)?;
    if !seats_in_front(user).contains(&pad.identity.seat) {
        return Err(Refusal::NotPermitted);
    }
    Ok(pad)
}

/// Hides the pad `fd` is open on, for connection `owner`. Returns too how
/// many times the pad was re-bound for it, which it goes and comes back
/// for: once if it was hidden, and once more if it was then shown again,
/// as it is when anything after the re-bind fails, or the client sends
/// `cancel`, or hangs up, before the hide is done.
fn hide_pad(
    shared: &Arc<Shared>,
    socket: BorrowedFd<'_>,
    owner: u64,
    user: Option<u32>,
    fd: OwnedFd,
) -> (Result<(u32, OwnedFd), Refusal>, u8) {
    match marked(shared, owner, user, fd) {
        Ok((token, pad)) => {
            let (opened, rebound) = hide::cut_off(&pad, group(), socket);
            let rebinds = u8::from(rebound.went);
            match opened {
                Ok(event) => {
                    let mut state = shared.state();
                    if let Some(entry) = state.entries.iter_mut().find(|entry| entry.token == token)
                    {
                        entry.phase = Phase::Hidden;
                    }
                    (Ok((token, event)), rebinds)
                }
                Err(refusal) => {
                    // Shown again with a re-bind only if hiding it asked
                    // for one, so the pad is left as it was.
                    let shown = show(shared, token, None, rebound.asked);
                    (Err(refusal), rebinds + shown)
                }
            }
        }
        Err(refusal) => (Err(refusal), 0),
    }
}

/// Checks the pad `fd` is open on for connection `owner`, and makes its
/// markers: everything of a hide before the re-bind. Returns its token.
fn marked(
    shared: &Shared,
    owner: u64,
    user: Option<u32>,
    fd: OwnedFd,
) -> Result<(u32, Pad), Refusal> {
    let pad = presented(user, fd.as_fd())?;
    // Before any marker is made: without the rule, the request can't be
    // made, and the pad is left as it was for the caller to grab.
    hide::may_request(&pad.parent)?;
    let token = {
        let mut state = shared.state();
        if state
            .entries
            .iter()
            .any(|entry| entry.pad.parent.path == pad.parent.path)
        {
            return Err(Refusal::Busy);
        }
        let token = state.new_token();
        // Made before the re-bind they take effect at: from here on a
        // crash leaves enough to undo.
        let others = state.markers();
        for marker in &pad.markers {
            if let Err(error) = hide::create(marker) {
                error!(marker, %error, "couldn't make a marker; the pad isn't hidden");
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
    Ok((token, pad))
}

/// Opens a hidden pad's event node again, finding the pad first if it
/// reconnected: only once the device it was checked as is gone, or is
/// another enumeration at the same place (a pad on USB with no HID device
/// is recorded by its USB interface, whose place is its port's), and only
/// for a caller in front, now, on the pad's seat, since the pad found may
/// be another the rule hides.
fn reopen(shared: &Shared, owner: u64, user: Option<u32>, token: u32) -> Result<OwnedFd, Refusal> {
    let seats = seats_in_front(user);
    let (parent, ordinal) = {
        let mut state = shared.state();
        // Every other hidden pad's device: this one's may be at the same
        // place, enumerated afresh.
        let taken: Vec<PathBuf> = state
            .entries
            .iter()
            .filter(|entry| entry.token != token)
            .map(|entry| entry.pad.parent.path.clone())
            .collect();
        let entry = state
            .entries
            .iter_mut()
            .find(|entry| {
                entry.token == token && entry.owner == owner && entry.phase == Phase::Hidden
            })
            .ok_or(Refusal::UnknownToken)?;
        if !entry.pad.parent.is_there() {
            if !seats.contains(&entry.pad.identity.seat) {
                return Err(Refusal::NotPermitted);
            }
            let taken: Vec<&Path> = taken.iter().map(PathBuf::as_path).collect();
            let found = pad::returned(&entry.pad, group(), &taken).ok_or(Refusal::Absent)?;
            for marker in found.markers.difference(&entry.pad.markers) {
                if let Err(error) = hide::create(marker) {
                    warn!(
                        token,
                        marker,
                        %error,
                        "couldn't mark the pad that came back by all it is named by; it is hidden by the markers it had"
                    );
                }
            }
            entry.pad.parent = found.parent;
            entry.pad.ordinal = found.ordinal;
            entry.pad.markers.extend(found.markers);
        }
        (entry.pad.parent.clone(), entry.pad.ordinal)
    };
    let node = pad::event_node(&parent.path, ordinal).ok_or(Refusal::Absent)?;
    hide::open_under(&node, &parent, Some(group()))
}

/// The device of a pad connection `owner` has hidden.
fn hidden_parent(shared: &Shared, owner: u64, token: u32) -> Result<Device, Refusal> {
    shared
        .state()
        .entries
        .iter()
        .find(|entry| entry.token == token && entry.owner == owner && entry.phase == Phase::Hidden)
        .map(|entry| entry.pad.parent.clone())
        .ok_or(Refusal::UnknownToken)
}

/// Opens the raw node of the pad whose device is `parent`.
fn raw_of(parent: &Device) -> Result<OwnedFd, Refusal> {
    let node = pad::raw_node(&parent.path)?;
    hide::open_under(&node, parent, None)
}

/// Serves one client until it closes, then shows again what it hid.
fn serve(shared: &Arc<Shared>, socket: &OwnedFd, owner: u64, user: Option<u32>) {
    let socket = socket.as_fd();
    // A client found to be in front stays permitted: one that later
    // leaves the foreground keeps what it has.
    let mut permitted = user.is_some_and(seat::is_in_front);
    // The pad the last hide hid, while it is: what `cancel` undoes.
    let mut last_hidden: Option<u32> = None;
    loop {
        let mut buffer = [0u8; Request::SIZE];
        let (length, mut fds) = match receive(socket, &mut buffer) {
            Ok(received) => received,
            Err(error) => {
                debug!(%error, "a client's connection broke");
                break;
            }
        };
        if length == 0 {
            break;
        }
        // Every descriptor received is closed on this thread, and any but
        // the one a request carries is closed now, before anything is
        // locked: closing one can block, as a FUSE file's flush does.
        fds.truncate(1);
        let fd = fds.pop();
        let request = buffer.get(..length).and_then(Request::decode);
        // A hide asked is the one a `cancel` gives up, whatever becomes of
        // it: one refused, at the gate or after, leaves nothing to undo.
        if request == Some(Request::Hide) {
            last_hidden = None;
        }
        if !permitted {
            permitted = user.is_some_and(seat::is_in_front);
        }
        if !permitted {
            refuse(socket, Refusal::NotPermitted);
            continue;
        }
        match request {
            Some(Request::Version) => {
                // Counting nothing here, it says it counts.
                reply(
                    socket,
                    ok(pack_release(env!("CARGO_PKG_VERSION")), false, 0),
                    None,
                );
            }
            Some(Request::Hide) => {
                let Some(fd) = fd else {
                    refuse(socket, Refusal::BadRequest);
                    continue;
                };
                match hide_pad(shared, socket, owner, user, fd) {
                    (Ok((token, event)), rebinds) => {
                        info!(token, "hid a pad");
                        last_hidden = Some(token);
                        // Every pad hidden is cut off by a re-bind, and
                        // shown again by another.
                        reply(socket, ok(token, true, rebinds), Some(&event));
                    }
                    (Err(refusal), rebinds) => {
                        // A cancel that ended it is read, and answered,
                        // next: nothing is left for it to do. A request
                        // sent ahead of it is served first, in order.
                        reply(socket, refused(refusal, rebinds), None);
                    }
                }
            }
            Some(Request::Cancel) => {
                let rebinds = match last_hidden.take() {
                    Some(token) if hidden_parent(shared, owner, token).is_ok() => {
                        let rebinds = show(shared, token, None, true);
                        info!(token, "showed a pad again, its hide cancelled");
                        rebinds
                    }
                    _ => 0,
                };
                reply(socket, ok(0, false, rebinds), None);
            }
            Some(Request::Reopen(Token(token))) => match reopen(shared, owner, user, token) {
                Ok(event) => reply(socket, ok(token, true, 0), Some(&event)),
                Err(refusal) => refuse(socket, refusal),
            },
            Some(Request::Restore(Token(token))) => {
                if hidden_parent(shared, owner, token).is_ok() {
                    if last_hidden == Some(token) {
                        last_hidden = None;
                    }
                    let rebinds = show(shared, token, Some(socket), true);
                    info!(token, "showed a pad again");
                    reply(socket, ok(token, false, rebinds), None);
                } else {
                    refuse(socket, Refusal::UnknownToken);
                }
            }
            Some(Request::Raw) => {
                let Some(fd) = fd else {
                    refuse(socket, Refusal::BadRequest);
                    continue;
                };
                match presented(user, fd.as_fd()).and_then(|pad| raw_of(&pad.parent)) {
                    Ok(raw) => reply(socket, ok(0, false, 0), Some(&raw)),
                    Err(refusal) => refuse(socket, refusal),
                }
            }
            Some(Request::RawOf(Token(token))) => {
                match hidden_parent(shared, owner, token).and_then(|parent| raw_of(&parent)) {
                    Ok(raw) => reply(socket, ok(token, false, 0), Some(&raw)),
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
    // Each on a thread of its own, so that one whose re-bind never ends
    // holds none of the others up; none holds the lock as it waits.
    std::thread::scope(|scope| {
        for token in mine {
            info!(token, "a client has gone; showing the pad it hid again");
            let spawned = std::thread::Builder::new()
                .name("show".into())
                .spawn_scoped(scope, move || show(shared, token, None, true));
            if let Err(error) = spawned {
                warn!(token, %error, "couldn't start a thread to show a pad again; showing it here");
                show(shared, token, None, true);
            }
        }
    });
}

/// What woke the main thread.
struct Woken {
    /// A connection closed.
    closed: bool,
    /// A client is waiting to be accepted.
    waiting: bool,
}

/// Waits for a connection to close, or, unless there is no room to accept
/// another, for a client to connect.
fn wait(shared: &Shared, listener: &OwnedFd, exhausted: bool) -> Woken {
    let mut fds = vec![PollFd::new(&shared.closed, PollFlags::IN)];
    if !exhausted {
        fds.push(PollFd::new(listener, PollFlags::IN));
    }
    match poll(&mut fds, None) {
        Ok(_) => Woken {
            closed: !fds[0].revents().is_empty(),
            waiting: fds.get(1).is_some_and(|fd| !fd.revents().is_empty()),
        },
        Err(Errno::INTR) => Woken {
            closed: false,
            waiting: false,
        },
        Err(error) => {
            // Ending the service would show every hidden pad, and trying
            // again at once would spin. Instead it waits on the one thing
            // that matters now, blocking on it alone: a connection closing
            // when there's no room to accept, else a client to accept.
            warn!(%error, "couldn't wait on the listener and the connections together; waiting on one alone");
            Woken {
                closed: exhausted,
                waiting: !exhausted,
            }
        }
    }
}

/// Serves clients until none is left, or forever if `stay` is set.
fn run(listener: &OwnedFd, stay: bool) -> io::Result<()> {
    let shared = Arc::new(Shared {
        state: Mutex::new(State::default()),
        // Blocking: it is read only once a connection has closed, or when
        // waiting on that alone.
        closed: eventfd(0, EventfdFlags::CLOEXEC)?,
    });
    // Nothing is connected yet, so anything still hidden was left by a
    // service that died. Its showing again takes each device's turn, so
    // a hide of it asked meanwhile waits for it.
    hide::clear();
    sweep(&shared);
    // Out of descriptors or memory, accepting is left until a connection
    // closes, which frees both.
    let mut exhausted = false;
    loop {
        let woken = wait(&shared, listener, exhausted);
        if woken.closed {
            let mut count = [0u8; 8];
            if let Err(error) = rustix::io::read(&shared.closed, &mut count) {
                warn!(%error, "couldn't read how many connections closed");
            }
            exhausted = false;
        }
        if !woken.waiting {
            let state = shared.state();
            if state.connections.open() == 0 && state.served_anyone && !stay {
                // Started on demand, and nobody needs it now. A client
                // that connects from here on waits on the socket systemd
                // holds, which starts the service again.
                return Ok(());
            }
            continue;
        }
        let socket = match accept_with(listener, SocketFlags::CLOEXEC) {
            Ok(socket) => socket,
            // The client gave up, or a signal came: nothing is left over.
            Err(Errno::INTR | Errno::CONNABORTED | Errno::AGAIN) => continue,
            Err(error) => {
                // The client stays queued: whatever ran out (descriptors,
                // memory) is what a connection closing gives back, and
                // that is the one event there is to wait for.
                if shared.state().connections.open() > 0 {
                    warn!(%error, "couldn't accept a connection; accepting again once one closes");
                    exhausted = true;
                    continue;
                }
                // With nothing connected nothing is hidden, so stopping
                // shows nothing: systemd starts the service afresh for
                // the client still queued.
                return Err(io::Error::other(format!(
                    "couldn't accept a connection with none open, so nothing will free what it needs: {error}"
                )));
            }
        };
        let user = match socket_peercred(&socket) {
            Ok(peer) => Some(peer.uid.as_raw()),
            Err(error) => {
                warn!(%error, "couldn't tell who connected; they are served as nobody");
                None
            }
        };
        let seated = user.is_some_and(seat::has_seat);
        let (owner, holder) = {
            let mut state = shared.state();
            state.served_anyone = true;
            let Some(holder) = state.connections.admit(user, seated) else {
                // Closed at once, here, with no thread made and nothing
                // read: neither one user nor every user with no seat can
                // take every descriptor. What no cap reaches is the queue
                // of connections not yet accepted, which is everyone's:
                // one who connects without pause can keep it near full,
                // and a client that connects without waiting is then
                // refused by the kernel, but nobody holds an accepted
                // connection over their cap longer than this.
                debug!(user, seated, "closed a connection over the cap");
                continue;
            };
            state.next_connection += 1;
            (state.next_connection, holder)
        };
        let client = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("client".into())
            .spawn(move || {
                serve(&client, &socket, owner, user);
                drop(socket);
                client.state().connections.release(holder);
                if let Err(error) = rustix::io::write(&client.closed, &1u64.to_ne_bytes()) {
                    warn!(%error, "couldn't say a connection closed");
                }
            });
        if let Err(error) = spawned {
            shared.state().connections.release(holder);
            warn!(%error, "couldn't start a thread for a connection; it is closed");
        }
    }
}

/// A listening socket at `path`, for running outside systemd.
fn listen_at(path: &Path) -> io::Result<OwnedFd> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            warn!(path = %path.display(), %error, "couldn't remove what was at the socket's path");
        }
    }
    let socket = socket(AddressFamily::UNIX, SocketType::SEQPACKET, None)?;
    // Anyone may connect, as with the installed socket; who is served is
    // decided per connection. The mode is given as the socket is made,
    // under a umask, so nothing is changed by a path that could have been
    // swapped since.
    let was = rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o111));
    let bound = SocketAddrUnix::new(path).and_then(|address| bind(&socket, &address));
    rustix::process::umask(was);
    bound?;
    listen(&socket, 8)?;
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

/// Logs to stderr, which systemd gives the journal: as lines with each
/// field as `name=value`, with no time, which the journal adds. A failed
/// write is dropped, not a panic.
fn logging() {
    let _ = tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_ansi(false)
        .without_time()
        .with_target(false)
        .try_init();
}

fn main() -> ExitCode {
    logging();
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
            error!(%error, "ctrlassist-hider");
            ExitCode::FAILURE
        }
    }
}
