//! The hiding service's wire protocol and its client.
//!
//! The service is a system process, a user of its own with no
//! capabilities, on a system socket. A client asks it to hide a pad by
//! passing an open descriptor on the pad's event node; the service has
//! the pad's nodes given to its own group alone, has its driver re-bound
//! so that whatever had it open is cut off, and passes back a descriptor
//! on the new event node with a token. When the client's connection
//! closes, for any reason, the service restores everything that
//! connection hid.
//!
//! A re-bind makes the pad go and come back, which a caller that keeps a
//! book of pads must not take for the pad unplugged and plugged in again.
//! So each reply says how many times the service re-bound the pad in
//! serving it; and a hide can be given up with [`Client::cancel`] rather
//! than by hanging up, which leaves the pad as it was and has the service
//! say what that took, where a hang-up would leave the caller not knowing.
//!
//! A pad is named by a descriptor the caller already holds, never by a
//! path or a name, so the service grants nothing the caller couldn't
//! already read.
//!
//! One request grants more than that, to the same caller and for the
//! same pad: the pad's raw HID node, which a seat user can't open on a
//! clean machine, for the kinds of pad named in the service. A virtual
//! pad that is to pass as such a pad is made from what that node says.

mod wire;

use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::Path;

use rustix::event::{PollFd, PollFlags, poll};
use rustix::io::Errno;
use rustix::net::{
    AddressFamily, Shutdown, SocketAddrUnix, SocketFlags, SocketType, connect, shutdown,
    socket_with,
};

pub use wire::{Refusal, Token};
#[doc(hidden)]
pub use wire::{Reply, Request, pack_release, receive, send};

/// Where the service listens when installed.
pub const SOCKET_PATH: &str = "/run/ctrlassist/hider.sock";

/// Why a request to the service failed.
#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// The service couldn't be reached, or the connection broke.
    Io(io::Error),
    /// The service refused.
    Refused(Refusal),
    /// The service answered with something this client doesn't understand.
    Protocol,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "the hiding service couldn't be reached: {error}"),
            Self::Refused(refusal) => write!(f, "the hiding service refused: {refusal}"),
            Self::Protocol => f.write_str("the hiding service's answer wasn't understood"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Refused(refusal) => Some(refusal),
            Self::Protocol => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<Errno> for Error {
    fn from(error: Errno) -> Self {
        Self::Io(error.into())
    }
}

/// A pad the service has hidden for this client.
#[derive(Debug)]
pub struct Hidden {
    /// Names the pad in later requests on the same connection.
    pub token: Token,
    /// The pad's event node, opened by the service after hiding it.
    pub event: OwnedFd,
    /// Whether whatever had the pad open before was cut off, by a re-bind
    /// of its driver, which showing it again repeats. A service that
    /// can't re-bind a pad refuses to hide it, so a pad it hid always has
    /// been; a service from before that said false for such a pad, which
    /// stayed readable by programs that already had it open.
    pub dislodged: bool,
}

/// What a hide came to, as the service said.
#[derive(Debug)]
pub struct Hid {
    /// The pad hidden, or why it wasn't.
    pub outcome: Result<Hidden, Refusal>,
    /// How many times the service re-bound the pad in serving the hide,
    /// each a going and a coming back: none from a service that doesn't
    /// count, which re-bound it once if it hid it.
    pub rebinds: Option<u8>,
}

/// A connection to the hiding service.
///
/// Dropping it closes the connection, and the service then restores every
/// pad it hid through it.
#[derive(Debug)]
#[must_use = "dropping the client closes the connection, which restores every pad hidden through it"]
pub struct Client {
    socket: OwnedFd,
}

impl Client {
    /// Connects to the service at `path`, without waiting: a service
    /// whose queue of connections to accept is full is refused at once,
    /// as one that isn't there is. Each request then waits for its
    /// reply, until the reply comes or the connection is
    /// [hung up](Self::hang_up).
    ///
    /// # Errors
    ///
    /// Fails if the socket can't be reached, or has no room for another
    /// connection. A caller the service won't serve is told so on its
    /// first request, not here.
    pub fn connect(path: &Path) -> Result<Self, Error> {
        let flags = SocketFlags::CLOEXEC | SocketFlags::NONBLOCK;
        let socket = socket_with(AddressFamily::UNIX, SocketType::SEQPACKET, flags, None)?;
        connect(&socket, &SocketAddrUnix::new(path)?)?;
        rustix::io::ioctl_fionbio(&socket, false)?;
        Ok(Self { socket })
    }

    /// Hangs up, from any thread, whatever else is using the client: a
    /// request under way ends at once with an error, and so does any
    /// asked after, and the service restores every pad hidden through
    /// the connection, as when the client is dropped.
    pub fn hang_up(&self) {
        // Fails only for a socket that is no longer connected, which has
        // nothing left to hang up.
        let _ = shutdown(&self.socket, Shutdown::Both);
    }

    fn send(&self, request: Request, pad: Option<BorrowedFd<'_>>) -> Result<(), Error> {
        let fds: &[BorrowedFd<'_>] = match &pad {
            Some(pad) => std::slice::from_ref(pad),
            None => &[],
        };
        send(self.socket.as_fd(), &request.encode(), fds)?;
        Ok(())
    }

    /// The next reply, refusals included.
    fn reply(&self) -> Result<(Reply, Vec<OwnedFd>), Error> {
        let mut buffer = [0u8; Reply::SIZE];
        let (length, fds) = receive(self.socket.as_fd(), &mut buffer)?;
        let reply = buffer
            .get(..length)
            .and_then(Reply::decode)
            .ok_or(Error::Protocol)?;
        Ok((reply, fds))
    }

    fn ask(
        &self,
        request: Request,
        pad: Option<BorrowedFd<'_>>,
    ) -> Result<(Reply, Vec<OwnedFd>), Error> {
        self.send(request, pad)?;
        let (reply, fds) = self.reply()?;
        match reply.refusal {
            Some(refusal) => Err(Error::Refused(refusal)),
            None => Ok((reply, fds)),
        }
    }

    /// The service's release, as `major.minor.patch`.
    ///
    /// # Errors
    ///
    /// Fails if the service refuses, as one from before it could be
    /// asked does, or the connection breaks.
    pub fn version(&self) -> Result<String, Error> {
        self.release().map(|(release, _)| release)
    }

    /// The service's release, as [`version`](Self::version) says it, and
    /// whether the service counts the re-binds it makes and takes
    /// [`cancel`](Self::cancel).
    ///
    /// # Errors
    ///
    /// As [`version`](Self::version).
    pub fn release(&self) -> Result<(String, bool), Error> {
        let (reply, _) = self.ask(Request::Version, None)?;
        Ok((wire::unpack_release(reply.token.0), reply.rebinds.is_some()))
    }

    fn hidden(reply: Reply, fds: Vec<OwnedFd>) -> Result<Hidden, Error> {
        let event = fds.into_iter().next().ok_or(Error::Protocol)?;
        Ok(Hidden {
            token: reply.token,
            event,
            dislodged: reply.dislodged,
        })
    }

    /// Hides the pad whose event node `pad` is open on.
    ///
    /// # Errors
    ///
    /// Fails if the service refuses (the caller isn't the user in front of
    /// the machine, the descriptor isn't a pad's, or another user has the
    /// pad hidden) or the connection breaks.
    pub fn hide(&self, pad: BorrowedFd<'_>) -> Result<Hidden, Error> {
        self.send_hide(pad)?;
        self.hid()?.outcome.map_err(Error::Refused)
    }

    /// Asks the service to hide the pad whose event node `pad` is open
    /// on, and returns without waiting for the reply: [`wait`](Self::wait)
    /// waits for it and [`hid`](Self::hid) reads it, or
    /// [`cancel`](Self::cancel) gives the hide up meanwhile.
    ///
    /// # Errors
    ///
    /// Fails if the connection breaks.
    pub fn send_hide(&self, pad: BorrowedFd<'_>) -> Result<(), Error> {
        self.send(Request::Hide, Some(pad))
    }

    /// Waits until a reply has come, the connection is hung up, or
    /// `wake`, if given, is readable: whichever is first.
    ///
    /// # Errors
    ///
    /// Fails if the two can't be waited on.
    pub fn wait(&self, wake: Option<BorrowedFd<'_>>) -> Result<(), Error> {
        let mut fds = vec![PollFd::new(&self.socket, PollFlags::IN)];
        if let Some(wake) = wake {
            fds.push(PollFd::from_borrowed_fd(wake, PollFlags::IN));
        }
        loop {
            match poll(&mut fds, None) {
                Ok(_) => return Ok(()),
                Err(Errno::INTR) => {}
                Err(error) => return Err(error.into()),
            }
        }
    }

    /// The reply to a hide sent with [`send_hide`](Self::send_hide):
    /// whether the pad was hidden, and how many times the service
    /// re-bound it, a refusal included.
    ///
    /// # Errors
    ///
    /// Fails if the connection breaks, or the reply makes no sense.
    pub fn hid(&self) -> Result<Hid, Error> {
        let (reply, fds) = self.reply()?;
        let outcome = match reply.refusal {
            Some(refusal) => Err(refusal),
            None => Ok(Self::hidden(reply, fds)?),
        };
        Ok(Hid {
            outcome,
            rebinds: reply.rebinds,
        })
    }

    /// Gives up the hide sent last, leaving the pad as it was: one still
    /// being done is ended and undone, and one done is undone. Doesn't
    /// wait: [`cancelled`](Self::cancelled) reads what it came to. Only
    /// for a service that says, in its [`release`](Self::release), that
    /// it takes this.
    ///
    /// # Errors
    ///
    /// Fails if the connection breaks.
    pub fn cancel(&self) -> Result<(), Error> {
        self.send(Request::Cancel, None)
    }

    /// After [`cancel`](Self::cancel): reads the hide's reply and the
    /// cancel's, and says how many times the service re-bound the pad for
    /// the two together, which is how many times the pad goes and comes
    /// back for them.
    ///
    /// # Errors
    ///
    /// Fails if the connection breaks before both replies have come, or
    /// one doesn't count.
    pub fn cancelled(&self) -> Result<u8, Error> {
        let mut rebinds = 0;
        // The hide's reply, whatever it was, and then the cancel's.
        for _ in 0..2 {
            let (reply, _) = self.reply()?;
            rebinds += reply.rebinds.ok_or(Error::Protocol)?;
        }
        Ok(rebinds)
    }

    /// Opens a hidden pad again after it has reconnected. The pad was
    /// never visible in between: the rule covered its new nodes.
    ///
    /// # Errors
    ///
    /// Fails if the token is unknown, the pad hasn't returned, or the
    /// connection breaks.
    pub fn reopen(&self, token: Token) -> Result<Hidden, Error> {
        let (reply, fds) = self.ask(Request::Reopen(token), None)?;
        Self::hidden(reply, fds)
    }

    /// Opens the raw HID node of the pad whose event node `pad` is open
    /// on, for reading and writing.
    ///
    /// # Errors
    ///
    /// Fails if the service refuses (the caller isn't the user in front
    /// of the machine, the descriptor isn't a pad's, or the pad isn't of
    /// a kind whose raw node it opens) or the connection breaks.
    pub fn raw(&self, pad: BorrowedFd<'_>) -> Result<OwnedFd, Error> {
        let (_, fds) = self.ask(Request::Raw, Some(pad))?;
        fds.into_iter().next().ok_or(Error::Protocol)
    }

    /// Opens the raw HID node of a pad hidden through this connection,
    /// for reading and writing: what a game sends a virtual pad beyond
    /// rumble is written there.
    ///
    /// # Errors
    ///
    /// Fails if the token is unknown, the pad isn't of a kind whose raw
    /// node the service opens, or the connection breaks.
    pub fn raw_of(&self, token: Token) -> Result<OwnedFd, Error> {
        let (_, fds) = self.ask(Request::RawOf(token), None)?;
        fds.into_iter().next().ok_or(Error::Protocol)
    }

    /// Restores a hidden pad. Returns how many times the service re-bound
    /// it to, if it counts: none where the pad wasn't there to.
    ///
    /// # Errors
    ///
    /// Fails if the token is unknown or the connection breaks.
    pub fn restore(&self, token: Token) -> Result<Option<u8>, Error> {
        self.ask(Request::Restore(token), None)
            .map(|(reply, _)| reply.rebinds)
    }
}
