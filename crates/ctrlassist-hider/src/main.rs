//! The hiding service: the one part of CtrlAssist that runs as root.
//!
//! It hides pads from games for the user's daemon and hands the daemon
//! open descriptors for them, and it does nothing else. It is small on
//! purpose, with no async runtime and none of the control-plane crates,
//! so that it can be read in one sitting.
//!
//! One thread serves every connection. A connection owns the pads it has
//! hidden; when it closes, for any reason, they are restored.

mod activation;
mod hide;
mod pad;
mod seat;

use std::io::{self, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::ExitCode;

use ctrlassist_hider::{Refusal, Reply, Request, Token, receive, send};
use rustix::event::{PollFd, PollFlags, poll};
use rustix::net::sockopt::socket_peercred;
use rustix::net::{AddressFamily, SocketAddrUnix, SocketType, accept, bind, listen, socket};

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
    hidden: Vec<hide::Entry>,
}

/// Writes a line to the journal. A failed write is ignored: there is
/// nowhere else to report it.
fn log(line: &str) {
    let _ = writeln!(io::stderr(), "{line}");
}

fn reply(connection: &Connection, reply: Reply, event: Option<&OwnedFd>) {
    let fds: Vec<_> = event.iter().map(|fd| fd.as_fd()).collect();
    // A client that has gone is found out by the poll loop.
    let _ = send(connection.socket.as_fd(), &reply.encode(), &fds);
}

fn refuse(connection: &Connection, refusal: Refusal) {
    reply(connection, Reply::refused(refusal), None);
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
            let taken = connections
                .iter()
                .flat_map(|connection| &connection.hidden)
                .any(|entry| entry.identity == pad.identity);
            if taken {
                refuse(&connections[index], Refusal::Busy);
                return true;
            }
            // The caller's own descriptor is closed before the driver is
            // re-bound, so the service isn't holding the pad open itself.
            drop(fds);
            let token = *next_token;
            *next_token = next_token.wrapping_add(1).max(1);
            match hide::hide(token, pad) {
                Ok((entry, event, dislodged)) => {
                    log(&format!(
                        "hid pad {:04x}:{:04x} as {token}",
                        entry.identity.vendor, entry.identity.product
                    ));
                    connections[index].hidden.push(entry);
                    let answer = Reply {
                        refusal: None,
                        token: Token(token),
                        dislodged,
                    };
                    reply(&connections[index], answer, Some(&event));
                }
                Err(refusal) => refuse(&connections[index], refusal),
            }
        }
        Some(Request::Reopen(Token(token))) => {
            let connection = &connections[index];
            match connection.hidden.iter().find(|entry| entry.token == token) {
                Some(entry) => match entry.reopen() {
                    Ok(event) => {
                        let answer = Reply {
                            refusal: None,
                            token: Token(token),
                            dislodged: true,
                        };
                        reply(connection, answer, Some(&event));
                    }
                    Err(refusal) => refuse(connection, refusal),
                },
                None => refuse(connection, Refusal::UnknownToken),
            }
        }
        Some(Request::Restore(Token(token))) => {
            let connection = &mut connections[index];
            match connection
                .hidden
                .iter()
                .position(|entry| entry.token == token)
            {
                Some(position) => {
                    connection.hidden.swap_remove(position).restore();
                    log(&format!("restored pad {token}"));
                    let answer = Reply {
                        refusal: None,
                        token: Token(token),
                        dislodged: false,
                    };
                    reply(connection, answer, None);
                }
                None => refuse(connection, Refusal::UnknownToken),
            }
        }
        Some(Request::Raw) => {
            let connection = &connections[index];
            let Some(descriptor) = fds.first() else {
                refuse(connection, Refusal::BadRequest);
                return true;
            };
            match pad::raw_node(descriptor.as_fd()).and_then(|node| hide::open(&node)) {
                Ok(raw) => {
                    let answer = Reply {
                        refusal: None,
                        token: Token(0),
                        dislodged: false,
                    };
                    reply(connection, answer, Some(&raw));
                }
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
                .and_then(|entry| pad::hidden_raw_node(&entry.identity))
                .and_then(|node| hide::open(&node));
            match raw {
                Ok(raw) => {
                    let answer = Reply {
                        refusal: None,
                        token: Token(token),
                        dislodged: false,
                    };
                    reply(connection, answer, Some(&raw));
                }
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
    hide::reconcile();

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
                    log(&format!("client gone; restoring pad {}", entry.token));
                    entry.restore();
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

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    let outcome = match arguments.as_slice() {
        // Started by systemd on a client's first connection.
        [] => match activation::listener() {
            Some(listener) => run(&listener, false),
            None => Err(io::Error::other(
                "not started by a systemd socket; use --listen <path>",
            )),
        },
        ["--listen", path] => listen_at(Path::new(path)).and_then(|listener| run(&listener, true)),
        // Undo whatever is still hidden: run by the unit after the service
        // stops, so a service that died doesn't leave pads hidden until
        // the next one starts.
        ["reconcile"] => {
            hide::reconcile();
            Ok(())
        }
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
