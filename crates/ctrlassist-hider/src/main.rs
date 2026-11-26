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
//! holds up nobody but itself. Where the kernel can, a connection passes
//! no descriptor until the service has admitted it; a connection that
//! isn't served is closed by a thread kept for that, never by the one
//! that accepts. What is hidden is shared, behind one lock that no thread
//! holds while it waits on a client or on udev. A connection owns the
//! pads it has hidden; when it closes, for any reason, they are shown
//! again.
//!
//! Nothing a client does may end the service: it is every user's, and
//! systemd starts it again only so often, five starts in ten seconds by
//! default, whatever ended each run, before it fails its socket for
//! everyone until root resets it. So a panic serving a connection ends
//! that connection alone, the pads it hid shown again, and the shared
//! state stays usable after a thread panicked holding it. The packages'
//! profile unwinds on panic, which is what lets a panic be contained.

mod activation;
mod connections;
mod descriptor;
mod hide;
mod pad;
mod seat;

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::mem::ManuallyDrop;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, SendError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use ctrlassist_hider::{
    Refusal, Reply, Request, Token, is_unknown_to_the_kernel, pack_release, receive, send,
    take_descriptors, takes_descriptors,
};
use rustix::event::{EventfdFlags, PollFd, PollFlags, eventfd, poll};
use rustix::io::Errno;
use rustix::net::sockopt::socket_peercred;
use rustix::net::{
    AddressFamily, SocketAddrUnix, SocketFlags, SocketType, accept_with, bind, listen, socket,
};
use tracing::{debug, error, info, warn};

use crate::connections::{CLOSING, Closer, Connections};
use crate::hide::Leave;
use crate::pad::{Device, Pad};

/// A pad a connection has hidden, from when its markers are made. Until
/// the re-bind that hides it is done, nobody can name it: only its
/// connection is told its token, whose thread is in the hide meanwhile.
#[derive(Debug)]
struct Entry {
    token: u32,
    /// The connection that hid it.
    owner: u64,
    pad: Pad,
    /// Whether its markers are gone and its driver is being re-bound to
    /// show it.
    showing: bool,
}

/// What every connection shares.
#[derive(Debug, Default)]
struct State {
    entries: Vec<Entry>,
    next_token: u32,
    next_connection: u64,
    connections: Connections,
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
            .filter(|entry| !entry.showing)
            .flat_map(|entry| entry.pad.markers.iter().cloned())
            .collect()
    }

    /// The device of every pad the service has, being shown or not.
    fn held(&self) -> Vec<PathBuf> {
        self.entries
            .iter()
            .map(|entry| entry.pad.parent.path.clone())
            .collect()
    }

    /// The token of every pad connection `owner` has, being shown or not.
    /// Only the connection's own thread, and the threads it waits for,
    /// show its pads, so once it is done with them one still being shown
    /// is one whose showing a panic cut short.
    fn owned_by(&self, owner: u64) -> Vec<u32> {
        self.entries
            .iter()
            .filter(|entry| entry.owner == owner)
            .map(|entry| entry.token)
            .collect()
    }
}

#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
    /// Written when a connection closes, which may let the service accept
    /// again after running out of room to.
    closed: OwnedFd,
}

impl Shared {
    fn new() -> io::Result<Self> {
        Ok(Self {
            state: Mutex::new(State::default()),
            // Blocking: it is read only once a connection has closed, or
            // when waiting on that alone.
            closed: eventfd(0, EventfdFlags::CLOEXEC)?,
        })
    }

    /// The shared state, even after a thread panicked holding it: it is
    /// still the record of what is hidden, which every other connection
    /// needs, and nothing under the lock leaves it unusable half done.
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Wakes the main thread: a connection has closed.
    fn say_closed(&self) {
        if let Err(error) = rustix::io::write(&self.closed, &1u64.to_ne_bytes()) {
            warn!(%error, "couldn't say a connection closed");
        }
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

fn refuse(socket: BorrowedFd<'_>, refusal: Refusal) {
    reply(socket, Reply::refused(refusal), None);
}

/// A reply that did as asked.
fn ok(token: u32) -> Reply {
    Reply {
        refusal: None,
        token: Token(token),
    }
}

/// Shows a pad again: the markers no other pad has go, its driver is
/// re-bound if it is there and `rebind` says to, as it does unless the
/// pad's hide never asked for a re-bind, and then any pad the rule hides
/// that nothing should is shown again too. The entry stays, as showing,
/// until the re-bind is done, so the pad can't be hidden again meanwhile;
/// the wait for it ends when udev is done, the pad is unplugged, or
/// `client`, the one that asked, hangs up. A hide of the pad once the
/// entry has gone asks its re-bind after this one's, which udev runs
/// first. A re-bind asked is run whether or not it was waited for to its
/// end, and shows the pad, its markers gone: the sweep after leaves that
/// device to it, rather than ask another.
fn show(shared: &Arc<Shared>, token: u32, client: Option<BorrowedFd<'_>>, rebind: bool) {
    let parent = {
        let mut state = shared.state();
        let Some(entry) = state.entries.iter_mut().find(|entry| entry.token == token) else {
            return;
        };
        entry.showing = true;
        let markers = entry.pad.markers.clone();
        let parent = (rebind && entry.pad.parent.is_there()).then(|| entry.pad.parent.clone());
        let remaining = state.markers();
        for marker in markers.difference(&remaining) {
            hide::delete(marker);
        }
        parent
    };
    let asked = parent.and_then(|parent| {
        let rebound = hide::rebind(&parent, Leave::AsTheRuleSays, client);
        if let Err(refusal) = rebound.outcome {
            warn!(
                token,
                device = %parent.path.display(),
                %refusal,
                asked = rebound.asked,
                "the pad's driver wasn't seen re-bound to show it again; it is shown by the re-bind if one was asked, else at its next event"
            );
        }
        rebound.asked.then_some(parent.path)
    });
    shared.state().entries.retain(|entry| entry.token != token);
    sweep(shared, asked.as_deref());
}

/// Runs `work`, and returns `None` if it panicked, having said so: a panic
/// ends what it was doing, never the service. The panic itself is told
/// by the standard hook, on stderr.
fn contained<T>(what: &str, work: impl FnOnce() -> T) -> Option<T> {
    let done = std::panic::catch_unwind(AssertUnwindSafe(work)).ok();
    if done.is_none() {
        error!(
            what,
            "panicked; that alone is given up, and the service goes on"
        );
    }
    done
}

/// Shows a pad again as [`show`] does, a panic in that ending it for this
/// pad alone: the pad is then [forgotten](forget). A panic in that is
/// contained too, so that whoever shows the pad goes on: a connection's
/// thread still gives back its place and says it closed.
fn show_contained(shared: &Arc<Shared>, token: u32) {
    if contained("showing a pad again", || show(shared, token, None, true)).is_some() {
        return;
    }
    contained("forgetting a pad whose showing panicked", || {
        forget(shared, token);
    });
}

/// Forgets a pad whose showing panicked, with the markers no other pad
/// has, so that nothing holds it hidden or busy; then sweeps, which has
/// its driver re-bound, its nodes being the group's and named by no
/// marker now, so that it is shown at once rather than at its next event.
fn forget(shared: &Shared, token: u32) {
    {
        let mut state = shared.state();
        let Some(index) = state.entries.iter().position(|entry| entry.token == token) else {
            return;
        };
        let entry = state.entries.remove(index);
        let remaining = state.markers();
        for marker in entry.pad.markers.difference(&remaining) {
            hide::delete(marker);
        }
    }
    sweep(shared, None);
}

/// Shows again every pad connection `owner` hid, each on a thread of its
/// own, so that one whose re-bind never ends holds none of the others up;
/// none holds the lock as it waits.
fn show_all(shared: &Arc<Shared>, owner: u64) {
    let mine = shared.state().owned_by(owner);
    std::thread::scope(|scope| {
        for token in mine {
            info!(token, "a client has gone; showing the pad it hid again");
            let spawned = std::thread::Builder::new()
                .name("show".into())
                .spawn_scoped(scope, move || show_contained(shared, token));
            if let Err(error) = spawned {
                warn!(token, %error, "couldn't start a thread to show a pad again; showing it here");
                show_contained(shared, token);
            }
        }
    });
}

/// A connection admitted, on its own thread: lets it pass descriptors,
/// if the listener refused them, now that a thread is here to take and
/// close them; has `serving` serve it until it closes; then shows again
/// what it hid, before its end is closed. A panic in serving it ends the
/// connection alone, what it hid shown again all the same.
fn connection(
    shared: &Arc<Shared>,
    socket: BorrowedFd<'_>,
    owner: u64,
    refusing: bool,
    serving: impl FnOnce(),
) {
    if refusing && let Err(error) = take_descriptors(socket, true) {
        warn!(%error, "couldn't let an admitted connection pass descriptors; it can have nothing hidden");
    }
    if contained("serving a client", serving).is_none() {
        warn!(
            owner,
            "a client's connection ended by a panic in serving it; showing what it hid again"
        );
    }
    show_all(shared, owner);
}

/// Shows again what the rule hides that nothing should, waiting for none
/// of it. Holds the state as it asks, so that a hide of one of those
/// devices is either in the state, and the device left alone, or asks
/// its own re-bind after this one, which udev runs after it. The device
/// at `asked` has a re-bind asked already that shows it, and is left to
/// that.
fn sweep(shared: &Shared, asked: Option<&Path>) {
    let state = shared.state();
    let mut held = state.held();
    held.extend(asked.map(Path::to_path_buf));
    let held: Vec<&Path> = held.iter().map(PathBuf::as_path).collect();
    hide::sweep(group(), &held, &state.markers());
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

/// Hides the pad `fd` is open on, for connection `owner`. Anything after
/// the re-bind failing, or the client hanging up before the hide is done,
/// shows the pad again.
fn hide_pad(
    shared: &Arc<Shared>,
    socket: BorrowedFd<'_>,
    owner: u64,
    user: Option<u32>,
    fd: OwnedFd,
) -> Result<(u32, OwnedFd), Refusal> {
    let (token, pad) = marked(shared, owner, user, fd)?;
    let (opened, rebound) = hide::cut_off(&pad, group(), socket);
    match opened {
        Ok(event) => Ok((token, event)),
        Err(refusal) => {
            // Shown again with a re-bind only if hiding it asked for one,
            // so the pad is left as it was.
            show(shared, token, None, rebound.asked);
            Err(refusal)
        }
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
            showing: false,
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
            .find(|entry| entry.token == token && entry.owner == owner && !entry.showing)
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
        .find(|entry| entry.token == token && entry.owner == owner && !entry.showing)
        .map(|entry| entry.pad.parent.clone())
        .ok_or(Refusal::UnknownToken)
}

/// Opens the raw node of the pad whose device is `parent`: a node of
/// that device, or of a Steam Controller's client device, the pad's own
/// still the enumeration that was checked once it is open.
fn raw_of(parent: &Device) -> Result<OwnedFd, Refusal> {
    let (node, under) = pad::raw_node(&parent.path)?;
    let raw = hide::open_under(&node, &under, None)?;
    if parent.is_there() {
        Ok(raw)
    } else {
        Err(Refusal::Absent)
    }
}

/// Serves one client until it closes. What it hid is shown again by
/// [`connection`] after.
fn serve(shared: &Arc<Shared>, socket: &OwnedFd, owner: u64, user: Option<u32>) {
    let socket = socket.as_fd();
    // A client found to be in front stays permitted: one that later
    // leaves the foreground keeps what it has.
    let mut permitted = user.is_some_and(seat::is_in_front);
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
        if !permitted {
            permitted = user.is_some_and(seat::is_in_front);
        }
        if !permitted {
            refuse(socket, Refusal::NotPermitted);
            continue;
        }
        match request {
            Some(Request::Version) => {
                reply(socket, ok(pack_release(env!("CARGO_PKG_VERSION"))), None);
            }
            Some(Request::Hide) => {
                let Some(fd) = fd else {
                    refuse(socket, Refusal::BadRequest);
                    continue;
                };
                match hide_pad(shared, socket, owner, user, fd) {
                    Ok((token, event)) => {
                        info!(token, "hid a pad");
                        reply(socket, ok(token), Some(&event));
                    }
                    // A request sent during the hide is served next, in
                    // order.
                    Err(refusal) => refuse(socket, refusal),
                }
            }
            Some(Request::Reopen(Token(token))) => match reopen(shared, owner, user, token) {
                Ok(event) => reply(socket, ok(token), Some(&event)),
                Err(refusal) => refuse(socket, refusal),
            },
            Some(Request::Restore(Token(token))) => {
                if hidden_parent(shared, owner, token).is_ok() {
                    show(shared, token, Some(socket), true);
                    info!(token, "showed a pad again");
                    reply(socket, ok(token), None);
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
                    Ok(raw) => reply(socket, ok(0), Some(&raw)),
                    Err(refusal) => refuse(socket, refusal),
                }
            }
            Some(Request::RawOf(Token(token))) => {
                match hidden_parent(shared, owner, token).and_then(|parent| raw_of(&parent)) {
                    Ok(raw) => reply(socket, ok(token), Some(&raw)),
                    Err(refusal) => refuse(socket, refusal),
                }
            }
            None => refuse(socket, Refusal::BadRequest),
        }
    }
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
            // when there's no room to accept, by reading the closings,
            // else a client to accept, by the accept, which waits for one.
            warn!(%error, "couldn't wait on the listener and the connections together; waiting on one alone");
            Woken {
                closed: exhausted,
                waiting: !exhausted,
            }
        }
    }
}

/// Whether a connection is refused descriptors until the service admits
/// it, as the socket unit has the listener refuse them and `--listen`
/// makes it: then each connection admitted is let pass them, and one the
/// service won't serve never could, so its close releases nothing that
/// could block the closer. Asked of the listener once, and said: a kernel
/// from before Linux 6.16 has no such setting, and every connection may
/// pass descriptors as soon as it has connected, leaving the closer the
/// only defense against a close that blocks. A listener that takes them
/// although the socket unit refuses them is made to refuse them here, for
/// the connections queued from now on: systemd before 258 doesn't know
/// the unit's setting, and a socket not restarted since its unit gained
/// it keeps the listener it made before. Which it was can't be told from
/// here, so both are said.
fn refusing(listener: BorrowedFd<'_>) -> bool {
    match takes_descriptors(listener) {
        Ok(false) => {
            info!("connections pass no descriptor until they are admitted");
            true
        }
        Ok(true) => match take_descriptors(listener, false) {
            Ok(()) => {
                warn!(
                    "the listener took descriptors although the socket unit refuses them: systemd before 258 ignores that setting, and a socket not restarted since its unit gained it keeps its listener as it was; it refuses them from now on, but a connection queued before now may still carry them, for the closer to close"
                );
                true
            }
            Err(error) => {
                warn!(%error, "the listener takes descriptors and couldn't be made to refuse them; a connection the service won't serve may carry a socket whose close blocks the closer");
                false
            }
        },
        Err(error) if is_unknown_to_the_kernel(&error) => {
            warn!(
                "this kernel can't refuse descriptors to a connection not yet admitted (Linux 6.16 can): a connection the service won't serve may carry a socket whose close blocks the closer, and with the closer's queue full, accepting"
            );
            false
        }
        // Each connection admitted is let pass them all the same, in case
        // the listener refuses them.
        Err(error) => {
            warn!(%error, "couldn't tell whether the listener refuses descriptors");
            true
        }
    }
}

/// Serves clients until stopped, or until accepting fails with nothing
/// connected.
///
/// Started on demand, the service stays once started: how often it starts
/// is then not its clients' doing, and systemd's own limits on how often
/// a socket may start its service, and the service start, can stay, to
/// stop a start that keeps failing (a missing user or binary) rather than
/// try it again for as long as a client waits. Were it to exit with its
/// last connection, anyone could start it as often as they connect, and
/// a daemon showing its pads again and hiding them anew a few times in a
/// row would meet the same limits.
///
/// `listener` is blocking, so an accept waits for a client: systemd
/// clears `O_NONBLOCK` on the sockets it passes, as `NonBlocking=` is
/// off, and [`listen_at`] makes it so. Only this thread accepts, so one
/// that `poll` said is waiting is there to accept.
///
/// The caller never closes `listener`, leaving it to the kernel as the
/// process exits: closing the last reference to a listener releases the
/// connections still queued on it, with what their clients sent, on the
/// thread that closes it, which a lingering socket would hold there.
fn run(listener: &OwnedFd) -> io::Result<()> {
    let refusing = refusing(listener.as_fd());
    let shared = Arc::new(Shared::new()?);
    // Woken after each close the closer makes, the main thread hands it
    // any connection it kept for want of room.
    let closing = Arc::clone(&shared);
    let mut closer = Closer::start(CLOSING, move || closing.say_closed())?;
    // Nothing is connected yet, so anything still hidden was left by a
    // service that died.
    hide::clear();
    sweep(&shared, None);
    // Out of descriptors or memory, accepting is left until a connection
    // closes, which frees both.
    let mut exhausted = false;
    loop {
        // With the closer behind, accepting is left until it has room:
        // the clients wait in the backlog, as they do for one another.
        let woken = wait(&shared, listener, exhausted || closer.full());
        if woken.closed {
            let mut count = [0u8; 8];
            if let Err(error) = rustix::io::read(&shared.closed, &mut count) {
                warn!(%error, "couldn't read how many connections closed");
            }
            exhausted = false;
            closer.retry();
        }
        if !woken.waiting {
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
                // the client still queued, within its limits.
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
            let Some(holder) = state.connections.admit(user, seated) else {
                // Closed at once, by the closer, with no thread made and
                // nothing read: neither one user nor every user with no
                // seat can take every descriptor. What no cap reaches is
                // the queue of connections not yet accepted, which is
                // everyone's: one who connects without pause can keep it
                // near full, and a client that connects without waiting
                // is then refused by the kernel, but nobody holds an
                // accepted connection over their cap longer than the
                // closer takes to close it.
                debug!(user, seated, "closing a connection over the cap");
                closer.close(socket);
                continue;
            };
            state.next_connection += 1;
            (state.next_connection, holder)
        };
        // The connection is handed over once the thread runs, so that one
        // whose thread couldn't be started is still here, for the closer;
        // and only its thread lets it pass descriptors, so the closer is
        // never handed one that could carry them. The client sends one
        // only once it has had a reply, which comes after.
        let (hand, handed) = mpsc::channel::<OwnedFd>();
        let client = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("client".into())
            .spawn(move || {
                if let Ok(socket) = handed.recv() {
                    connection(&client, socket.as_fd(), owner, refusing, || {
                        serve(&client, &socket, owner, user);
                    });
                }
                client.state().connections.release(holder);
                client.say_closed();
            });
        match spawned {
            Ok(_) => {
                // The thread takes it before anything else.
                if let Err(SendError(socket)) = hand.send(socket) {
                    closer.close(socket);
                }
            }
            Err(error) => {
                shared.state().connections.release(holder);
                warn!(%error, "couldn't start a thread for a connection; it is closed");
                closer.close(socket);
            }
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
    // Descriptors refused until a connection is admitted, as the socket
    // unit has it, from before anyone can connect. Where that fails, the
    // service says what it makes of the listener as it starts.
    let _ = take_descriptors(socket.as_fd(), false);
    // Anyone may connect, as with the installed socket; who is served is
    // decided per connection. The mode is given as the socket is made,
    // under a umask, so nothing is changed by a path that could have been
    // swapped since.
    let was = rustix::process::umask(rustix::fs::Mode::from_raw_mode(0o111));
    let bound = SocketAddrUnix::new(path).and_then(|address| bind(&socket, &address));
    rustix::process::umask(was);
    bound?;
    listen(&socket, 8)?;
    // Touched once it listens, as systemd touches a socket unit's: a
    // client that found it refusing between the bind and the listen
    // hears that it takes connections now.
    let now = rustix::fs::Timespec {
        tv_sec: 0,
        tv_nsec: rustix::fs::UTIME_NOW,
    };
    let times = rustix::fs::Timestamps {
        last_access: now,
        last_modification: now,
    };
    if let Err(error) =
        rustix::fs::utimensat(rustix::fs::CWD, path, &times, rustix::fs::AtFlags::empty())
    {
        warn!(path = %path.display(), %error, "couldn't touch the socket once it listens; a client waiting for it hears of it when it next needs it");
    }
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
/// write is dropped, not a panic: by default the subscriber reports one
/// with `eprintln!`, which panics when stderr fails too.
fn logging() {
    let _ = tracing_subscriber::fmt()
        .log_internal_errors(false)
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
            Some(listener) => run(&ManuallyDrop::new(listener)),
            None => Err(io::Error::other(
                "not started by a systemd socket; use --listen <path>",
            )),
        }),
        ["--listen", path] => unprivileged()
            .and_then(|()| listen_at(Path::new(path)))
            .and_then(|listener| run(&ManuallyDrop::new(listener))),
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use rustix::net::{AddressFamily, SocketFlags, SocketType, socketpair};

    use super::*;
    use crate::pad::Identity;

    fn pair() -> (OwnedFd, OwnedFd) {
        socketpair(
            AddressFamily::UNIX,
            SocketType::SEQPACKET,
            SocketFlags::CLOEXEC,
            None,
        )
        .unwrap()
    }

    /// Sends a request with a descriptor beside it, as a hide does.
    fn passes(client: &OwnedFd) -> rustix::io::Result<()> {
        let null = std::fs::File::open("/dev/null").unwrap();
        send(client.as_fd(), &Request::Hide.encode(), &[null.as_fd()])
    }

    fn entry(token: u32, owner: u64, showing: bool) -> Entry {
        Entry {
            token,
            owner,
            pad: Pad {
                parent: Device {
                    path: PathBuf::from("/sys/devices/virtual/ctrlassist-test-no-such-device"),
                    enumeration: None,
                },
                ordinal: 0,
                identity: Identity {
                    bus: 3,
                    vendor: 0x054c,
                    product: 0x0ce6,
                    seat: "seat0".into(),
                    virtual_device: true,
                },
                markers: BTreeSet::new(),
                named_by: BTreeSet::new(),
            },
            showing,
        }
    }

    /// A panic serving a connection, even with the shared state held,
    /// ends that connection and nothing else: the state is still there
    /// for every other connection, and the thread goes on to show again
    /// what it hid. Its thread, not the one that accepted it, lets it
    /// pass descriptors.
    #[test]
    fn a_panic_serving_a_client_ends_its_connection_alone() {
        let shared = Arc::new(Shared::new().unwrap());
        let (service, client) = pair();
        // Refused, as a connection is left by a listener that refuses
        // them, where the kernel can.
        let refusing = take_descriptors(service.as_fd(), false).is_ok();
        if refusing {
            assert_eq!(passes(&client), Err(Errno::PERM));
        }
        let other = entry(2, 8, false);
        shared.state().entries.push(other);
        connection(&shared, service.as_fd(), 7, refusing, || {
            passes(&client).expect("an admitted connection passes descriptors as it is served");
            let _held = shared.state();
            panic!("a request went wrong");
        });
        assert_eq!(
            shared.state().owned_by(8),
            [2],
            "another connection's pad stays"
        );
    }

    /// Once a connection's thread is done serving it, a pad of it still
    /// being shown is one a panic cut short, and is shown again too.
    #[test]
    fn a_connection_s_pads_are_all_shown_again_being_shown_or_not() {
        let mut state = State::default();
        state.entries.push(entry(1, 7, false));
        state.entries.push(entry(2, 7, true));
        state.entries.push(entry(3, 8, false));
        assert_eq!(state.owned_by(7), [1, 2]);
    }
}
