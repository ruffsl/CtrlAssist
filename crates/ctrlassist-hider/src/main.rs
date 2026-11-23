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
//! One thread serves every connection. A connection owns the pads it has
//! hidden; when it closes, for any reason, they are shown again.

mod activation;
mod hide;
mod pad;
mod seat;

use std::collections::BTreeSet;
use std::io::{self, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use ctrlassist_hider::{Refusal, Reply, Request, Token, pack_release, receive, send};
use rustix::event::{PollFd, PollFlags, poll};
use rustix::net::sockopt::socket_peercred;
use rustix::net::{AddressFamily, SocketAddrUnix, SocketType, accept, bind, listen, socket};

use crate::pad::Pad;

/// A pad a connection has hidden.
#[derive(Debug)]
struct Entry {
    token: u32,
    pad: Pad,
}

/// One client and what it has hidden.
struct Connection {
    socket: OwnedFd,
    /// The client's user, from the socket.
    user: Option<u32>,
    /// Whether the client has been found to be the user in front of the
    /// machine. Looked at as it connects and, until it is so, at each
    /// request: a user's services start before their session is in
    /// front. Once it is so it stays so.
    permitted: bool,
    hidden: Vec<Entry>,
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

fn reply(connection: &Connection, reply: Reply, event: Option<&OwnedFd>) {
    let fds: Vec<_> = event.iter().map(|fd| fd.as_fd()).collect();
    // A client that has gone is found out by the poll loop.
    let _ = send(connection.socket.as_fd(), &reply.encode(), &fds);
}

fn refuse(connection: &Connection, refusal: Refusal) {
    reply(connection, Reply::refused(refusal), None);
}

fn ok(token: u32, dislodged: bool) -> Reply {
    Reply {
        refusal: None,
        token: Token(token),
        dislodged,
    }
}

/// Every hidden pad's entry, across connections.
fn entries(connections: &[Connection]) -> impl Iterator<Item = &Entry> {
    connections.iter().flat_map(|connection| &connection.hidden)
}

/// A token no hidden pad has, the next after `next`.
fn new_token(connections: &[Connection], next: &mut u32) -> u32 {
    loop {
        let token = *next;
        *next = next.wrapping_add(1).max(1);
        if !entries(connections).any(|entry| entry.token == token) {
            return token;
        }
    }
}

/// Shows a pad again once its entry is gone from `connections`: the
/// markers no other hidden pad has go, its driver is re-bound if it is
/// there, and any pad the rule now hides that nothing should is shown
/// again too.
fn show(entry: Entry, connections: &[Connection]) {
    let remaining: BTreeSet<String> = entries(connections)
        .flat_map(|other| other.pad.markers.iter().cloned())
        .collect();
    for marker in entry.pad.markers.difference(&remaining) {
        hide::delete(marker);
    }
    if pad::is_there(&entry.pad) {
        let _ = hide::rebind(&entry.pad.parent, None);
    }
    let held: Vec<&Path> = entries(connections)
        .map(|other| other.pad.parent.as_path())
        .collect();
    hide::sweep(group(), &held, &remaining);
}

/// Finds a hidden pad again after it has reconnected, and records where
/// it is now. Acts only once the device it was checked as is gone.
fn follow(connections: &mut [Connection], index: usize, position: usize) -> Result<(), Refusal> {
    if pad::is_there(&connections[index].hidden[position].pad) {
        return Ok(());
    }
    let taken: Vec<PathBuf> = entries(connections)
        .map(|entry| entry.pad.parent.clone())
        .collect();
    let taken: Vec<&Path> = taken.iter().map(PathBuf::as_path).collect();
    let entry = &mut connections[index].hidden[position];
    let found = pad::returned(&entry.pad, group(), &taken).ok_or(Refusal::Absent)?;
    for marker in found.markers.difference(&entry.pad.markers) {
        let _ = hide::create(marker);
    }
    entry.pad.parent = found.parent;
    entry.pad.ordinal = found.ordinal;
    entry.pad.markers.extend(found.markers);
    Ok(())
}

/// Serves one request. Returns false when the client has closed.
fn serve(connections: &mut [Connection], index: usize, next_token: &mut u32) -> bool {
    let mut buffer = [0u8; Request::SIZE];
    let Ok((length, fds)) = receive(connections[index].socket.as_fd(), &mut buffer) else {
        return false;
    };
    if length == 0 {
        return false;
    }
    if !connections[index].permitted {
        connections[index].permitted = connections[index].user.is_some_and(seat::is_in_front);
    }
    if !connections[index].permitted {
        refuse(&connections[index], Refusal::NotPermitted);
        return true;
    }
    match Request::decode(&buffer[..length]) {
        Some(Request::Version) => {
            let answer = ok(pack_release(env!("CARGO_PKG_VERSION")), false);
            reply(&connections[index], answer, None);
        }
        Some(Request::Hide) => {
            let Some(descriptor) = fds.first() else {
                refuse(&connections[index], Refusal::BadRequest);
                return true;
            };
            let pad = match pad::identify(descriptor.as_fd()) {
                Ok(pad) => pad,
                Err(refusal) => {
                    refuse(&connections[index], refusal);
                    return true;
                }
            };
            if entries(connections).any(|entry| entry.pad.parent == pad.parent) {
                refuse(&connections[index], Refusal::Busy);
                return true;
            }
            // The caller's own descriptor is closed before the driver is
            // re-bound, so the service isn't holding the pad open itself.
            drop(fds);
            let token = new_token(connections, next_token);
            match hide::hide(&pad, connections[index].socket.as_fd()) {
                Ok(event) => {
                    log(&format!(
                        "hid pad {:04x}:{:04x} as {token}",
                        pad.identity.vendor, pad.identity.product
                    ));
                    connections[index].hidden.push(Entry { token, pad });
                    // Every pad hidden is cut off by a re-bind, and shown
                    // again by another.
                    reply(&connections[index], ok(token, true), Some(&event));
                }
                Err(refusal) => {
                    show(Entry { token, pad }, connections);
                    refuse(&connections[index], refusal);
                }
            }
        }
        Some(Request::Reopen(Token(token))) => {
            let Some(position) = connections[index]
                .hidden
                .iter()
                .position(|entry| entry.token == token)
            else {
                refuse(&connections[index], Refusal::UnknownToken);
                return true;
            };
            let opened = follow(connections, index, position).and_then(|()| {
                let pad = &connections[index].hidden[position].pad;
                hide::open(&pad::event_node(&pad.parent, pad.ordinal).ok_or(Refusal::Absent)?)
            });
            match opened {
                Ok(event) => reply(&connections[index], ok(token, true), Some(&event)),
                Err(refusal) => refuse(&connections[index], refusal),
            }
        }
        Some(Request::Restore(Token(token))) => {
            match connections[index]
                .hidden
                .iter()
                .position(|entry| entry.token == token)
            {
                Some(position) => {
                    let entry = connections[index].hidden.swap_remove(position);
                    show(entry, connections);
                    log(&format!("showed pad {token} again"));
                    reply(&connections[index], ok(token, false), None);
                }
                None => refuse(&connections[index], Refusal::UnknownToken),
            }
        }
        Some(Request::Raw) => {
            let connection = &connections[index];
            let Some(descriptor) = fds.first() else {
                refuse(connection, Refusal::BadRequest);
                return true;
            };
            let raw = pad::identify(descriptor.as_fd())
                .and_then(|pad| pad::raw_node(&pad.parent))
                .and_then(|node| hide::open(&node));
            match raw {
                Ok(raw) => reply(connection, ok(0, false), Some(&raw)),
                Err(refusal) => refuse(connection, refusal),
            }
        }
        Some(Request::RawOf(Token(token))) => {
            let connection = &connections[index];
            let raw = connection
                .hidden
                .iter()
                .find(|entry| entry.token == token)
                .ok_or(Refusal::UnknownToken)
                .and_then(|entry| pad::raw_node(&entry.pad.parent))
                .and_then(|node| hide::open(&node));
            match raw {
                Ok(raw) => reply(connection, ok(token, false), Some(&raw)),
                Err(refusal) => refuse(connection, refusal),
            }
        }
        None => refuse(&connections[index], Refusal::BadRequest),
    }
    true
}

/// Serves clients until none is left, or forever if `stay` is set.
fn run(listener: &OwnedFd, stay: bool) -> io::Result<()> {
    // Nothing is connected yet, so anything still hidden was left by a
    // service that died.
    hide::reconcile(group());

    let mut connections: Vec<Connection> = Vec::new();
    let mut next_token = 1u32;
    let mut served_anyone = false;
    loop {
        if connections.is_empty() && served_anyone && !stay {
            // Started on demand, and nobody needs it now.
            return Ok(());
        }
        let mut fds = vec![PollFd::new(listener, PollFlags::IN)];
        fds.extend(
            connections
                .iter()
                .map(|connection| PollFd::new(&connection.socket, PollFlags::IN)),
        );
        poll(&mut fds, None)?;
        let ready: Vec<bool> = fds.iter().map(|fd| !fd.revents().is_empty()).collect();
        drop(fds);

        // Connections first, last to first, so removing one doesn't
        // disturb the indices still to visit.
        for index in (0..connections.len()).rev() {
            if ready[index + 1] && !serve(&mut connections, index, &mut next_token) {
                let closed = connections.swap_remove(index);
                for entry in closed.hidden {
                    log(&format!("client gone; showing pad {} again", entry.token));
                    show(entry, &connections);
                }
            }
        }
        if ready[0] {
            let socket = accept(listener)?;
            // A client found to be in front stays permitted: one that
            // later leaves the foreground keeps what it has.
            let user = socket_peercred(&socket).ok().map(|peer| peer.uid.as_raw());
            let permitted = user.is_some_and(seat::is_in_front);
            served_anyone = true;
            connections.push(Connection {
                socket,
                user,
                permitted,
                hidden: Vec::new(),
            });
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
