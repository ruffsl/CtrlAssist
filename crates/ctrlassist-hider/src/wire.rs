//! What crosses the socket.
//!
//! One message per request and one per reply, on a sequenced-packet
//! socket, so there is no framing to get wrong. A request is an opcode and
//! a token; a reply is a status, a token and flags. Descriptors travel
//! beside them as ancillary data. Every request has exactly one reply, and
//! replies come in the order of their requests.

use std::io::{IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::{BorrowedFd, OwnedFd};

use rustix::net::{
    RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags, recvmsg, sendmsg,
};

/// Names a hidden pad within one connection.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Token(pub u32);

/// Why the service refused a request.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum Refusal {
    /// The caller isn't the user with the session in front on a seat.
    NotPermitted,
    /// The descriptor isn't on a pad's event node, or it is on something
    /// that is also a keyboard or a mouse.
    NotAPad,
    /// The pad is already hidden.
    Busy,
    /// The token doesn't name a pad this connection hid.
    UnknownToken,
    /// The pad isn't there, as when it hasn't reconnected yet.
    Absent,
    /// The service couldn't carry the request out.
    Failed,
    /// The request wasn't understood.
    BadRequest,
    /// The pad isn't of a kind the service can do that for: one with no
    /// driver to re-bind, which it can't hide, or one whose raw node it
    /// doesn't open.
    Unsupported,
    /// A hide given up because the client sent [`Request::Cancel`] before
    /// it was done: the pad is left as it was. Only a client that sends
    /// `cancel` is ever told this.
    Cancelled,
}

impl Refusal {
    const ALL: [Self; 9] = [
        Self::NotPermitted,
        Self::NotAPad,
        Self::Busy,
        Self::UnknownToken,
        Self::Absent,
        Self::Failed,
        Self::BadRequest,
        Self::Unsupported,
        Self::Cancelled,
    ];

    fn code(self) -> u8 {
        match self {
            Self::NotPermitted => 1,
            Self::NotAPad => 2,
            Self::Busy => 3,
            Self::UnknownToken => 4,
            Self::Absent => 5,
            Self::Failed => 6,
            Self::BadRequest => 7,
            Self::Unsupported => 8,
            Self::Cancelled => 9,
        }
    }
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotPermitted => "the caller isn't the user in front of the machine",
            Self::NotAPad => "that isn't a pad",
            Self::Busy => "the pad is already hidden",
            Self::UnknownToken => "no such hidden pad on this connection",
            Self::Absent => "the pad isn't connected",
            Self::Failed => "the service couldn't carry the request out",
            Self::BadRequest => "the request wasn't understood",
            Self::Unsupported => "the service can't do that for this kind of pad",
            Self::Cancelled => "the hide was given up as asked, and the pad left as it was",
        })
    }
}

impl std::error::Error for Refusal {}

/// A request to the service.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Request {
    /// Hide the pad whose event node the accompanying descriptor is on.
    Hide,
    /// Open a hidden pad again after it has reconnected.
    Reopen(Token),
    /// Restore a hidden pad.
    Restore(Token),
    /// Open the raw HID node of the pad whose event node the
    /// accompanying descriptor is on.
    Raw,
    /// Open the raw HID node of a pad this connection has hidden.
    RawOf(Token),
    /// Say the service's release, which the reply carries where a token
    /// goes, packed by [`pack_release`]. A service from before this
    /// request refuses it as one it doesn't understand.
    Version,
    /// Give up the hide asked last on this connection, leaving the pad as
    /// it was. Sent while the hide is still being done, and the next
    /// message after it, it ends it: the hide's reply is then
    /// [`Refusal::Cancelled`], with the pad shown again, and this
    /// request's says nothing more was done. Sent after the hide's reply,
    /// or behind another request sent during the hide, which is served in
    /// its turn after the hide, it shows the pad that hide hid again.
    /// Either way the two replies' [`rebinds`](Reply::rebinds) together say
    /// how many times the pad was re-bound. Only this, or the connection
    /// closing, ends a hide early: an empty message closes the connection
    /// as a hang-up does. A hide refused leaves nothing for this to give
    /// up. A service that says it counts, in its reply to
    /// [`Request::Version`], takes this; one from before refuses it as one
    /// it doesn't understand.
    Cancel,
}

/// A release, `major.minor.patch`, as the 32 bits a reply has room for:
/// twelve for the major and ten for each of the others. Anything after
/// the patch, such as a pre-release tag, doesn't travel. Zero for text
/// that isn't a release, which no release packs to but `0.0.0`.
#[must_use]
pub fn pack_release(release: &str) -> u32 {
    let core = release.split(['-', '+']).next().unwrap_or_default();
    let mut parts = core.split('.').map(str::parse::<u32>);
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Ok(major)), Some(Ok(minor)), Some(Ok(patch)), None)
            if major < 1 << 12 && minor < 1 << 10 && patch < 1 << 10 =>
        {
            major << 20 | minor << 10 | patch
        }
        _ => 0,
    }
}

/// The release [`pack_release`] packed.
#[must_use]
pub(crate) fn unpack_release(packed: u32) -> String {
    format!(
        "{}.{}.{}",
        packed >> 20,
        (packed >> 10) & 0x3ff,
        packed & 0x3ff
    )
}

impl Request {
    /// A request's size on the wire.
    pub const SIZE: usize = 5;

    /// The request as bytes.
    #[must_use]
    pub fn encode(self) -> [u8; Self::SIZE] {
        let (opcode, token) = match self {
            Self::Hide => (1, 0),
            Self::Reopen(token) => (2, token.0),
            Self::Restore(token) => (3, token.0),
            Self::Raw => (4, 0),
            Self::RawOf(token) => (5, token.0),
            Self::Version => (6, 0),
            Self::Cancel => (7, 0),
        };
        let token = token.to_le_bytes();
        [opcode, token[0], token[1], token[2], token[3]]
    }

    /// The request in `bytes`, if they are one.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let [opcode, a, b, c, d] = *bytes else {
            return None;
        };
        let token = Token(u32::from_le_bytes([a, b, c, d]));
        match opcode {
            1 => Some(Self::Hide),
            2 => Some(Self::Reopen(token)),
            3 => Some(Self::Restore(token)),
            4 => Some(Self::Raw),
            5 => Some(Self::RawOf(token)),
            6 => Some(Self::Version),
            7 => Some(Self::Cancel),
            _ => None,
        }
    }
}

/// The service's answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Reply {
    /// Why the request was refused, if it was.
    pub refusal: Option<Refusal>,
    /// The pad's token.
    pub token: Token,
    /// Whether programs that had the pad open were cut off.
    pub dislodged: bool,
    /// How many times the service re-bound the pad's driver in serving
    /// the request, each of which makes the pad go and come back: a
    /// re-bind it saw end without that, the pad unplugged or the rule
    /// not acting, isn't counted, and one still under way is. A service
    /// that counts says it in every reply, refusals included, as `Some(0)`
    /// where it re-bound nothing (a refusal before it looked at the pad,
    /// a request that re-binds nothing); one from before says none in
    /// any. In its reply to [`Request::Version`], `Some(0)` says the
    /// service counts and takes [`Request::Cancel`]. At most
    /// [`Reply::MOST_REBINDS`].
    pub rebinds: Option<u8>,
}

impl Reply {
    /// A reply's size on the wire.
    pub const SIZE: usize = 6;

    /// The most re-binds a reply can say: two bits of the flags.
    pub const MOST_REBINDS: u8 = 3;

    /// A refusal, as a service from before the count says it: with no
    /// count.
    #[must_use]
    pub fn refused(refusal: Refusal) -> Self {
        Self {
            refusal: Some(refusal),
            token: Token(0),
            dislodged: false,
            rebinds: None,
        }
    }

    /// The reply as bytes: the flags are whether the pad was dislodged
    /// (bit 0), whether re-binds are counted (bit 1), and how many
    /// (bits 2 and 3).
    #[must_use]
    pub fn encode(self) -> [u8; Self::SIZE] {
        let status = self.refusal.map_or(0, Refusal::code);
        let token = self.token.0.to_le_bytes();
        let counted = self
            .rebinds
            .map_or(0, |rebinds| 0b10 | rebinds.min(Self::MOST_REBINDS) << 2);
        [
            status,
            token[0],
            token[1],
            token[2],
            token[3],
            u8::from(self.dislodged) | counted,
        ]
    }

    /// The reply in `bytes`, if they are one.
    #[must_use]
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let [status, a, b, c, d, flags] = *bytes else {
            return None;
        };
        let refusal = match status {
            0 => None,
            code => Some(*Refusal::ALL.iter().find(|refusal| refusal.code() == code)?),
        };
        // Bits above these are left for later, and not read.
        Some(Self {
            refusal,
            token: Token(u32::from_le_bytes([a, b, c, d])),
            dislodged: flags & 1 != 0,
            rebinds: (flags & 0b10 != 0).then_some(flags >> 2 & 0b11),
        })
    }
}

/// The most descriptors one message carries.
const MAX_FDS: usize = 2;

/// Sends one message with any descriptors beside it.
///
/// # Errors
///
/// Fails if the socket does.
pub fn send(
    socket: BorrowedFd<'_>,
    bytes: &[u8],
    fds: &[BorrowedFd<'_>],
) -> rustix::io::Result<()> {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
    let mut control = SendAncillaryBuffer::new(&mut space);
    if !fds.is_empty() {
        control.push(SendAncillaryMessage::ScmRights(fds));
    }
    sendmsg(
        socket,
        &[IoSlice::new(bytes)],
        &mut control,
        SendFlags::NOSIGNAL,
    )?;
    Ok(())
}

/// Receives one message into `buffer`, with any descriptors that came
/// beside it. A length of zero means the other end has closed.
///
/// The length is the message's own, which is more than the buffer holds
/// when the message didn't fit: what was cut off isn't to be read as a
/// shorter message, so a caller decodes `buffer.get(..length)`, which is
/// then nothing.
///
/// # Errors
///
/// Fails if the socket does.
pub fn receive(
    socket: BorrowedFd<'_>,
    buffer: &mut [u8],
) -> rustix::io::Result<(usize, Vec<OwnedFd>)> {
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
    let mut control = RecvAncillaryBuffer::new(&mut space);
    let received = recvmsg(
        socket,
        &mut [IoSliceMut::new(buffer)],
        &mut control,
        RecvFlags::CMSG_CLOEXEC | RecvFlags::TRUNC,
    )?;
    let mut fds = Vec::new();
    for message in control.drain() {
        if let RecvAncillaryMessage::ScmRights(received) = message {
            fds.extend(received);
        }
    }
    Ok((received.bytes, fds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cancel_and_a_count_of_re_binds_survive_the_wire() {
        assert_eq!(
            Request::decode(&Request::Cancel.encode()),
            Some(Request::Cancel)
        );
        assert_eq!(Request::Cancel.encode(), [7, 0, 0, 0, 0]);
        for rebinds in [None, Some(0), Some(1), Some(2), Some(3)] {
            for refusal in [None, Some(Refusal::Cancelled), Some(Refusal::Failed)] {
                let reply = Reply {
                    refusal,
                    token: Token(5),
                    dislodged: refusal.is_none(),
                    rebinds,
                };
                assert_eq!(Reply::decode(&reply.encode()), Some(reply));
            }
        }
        // More than fits is said as the most that does.
        let many = Reply {
            rebinds: Some(9),
            ..Reply::refused(Refusal::Cancelled)
        };
        assert_eq!(Reply::decode(&many.encode()).unwrap().rebinds, Some(3));
        assert!(!Refusal::Cancelled.to_string().is_empty());
    }

    /// A client from before the count reads the flags' first bit alone, and
    /// a service from before sets no other: each reads the other as it did.
    #[test]
    fn a_count_is_invisible_to_a_peer_from_before_it() {
        let reply = Reply {
            refusal: None,
            token: Token(1),
            dislodged: true,
            rebinds: Some(1),
        };
        assert_eq!(reply.encode()[5] & 1, 1, "dislodged where it always was");
        let from_before = [0, 1, 0, 0, 0, 1];
        let read = Reply::decode(&from_before).unwrap();
        assert!(read.dislodged);
        assert_eq!(read.rebinds, None, "a service from before counts nothing");
        // Bits above the count are read as nothing.
        assert_eq!(
            Reply::decode(&[0, 1, 0, 0, 0, 0xf0 | 0b0110])
                .unwrap()
                .rebinds,
            Some(1)
        );
    }
}
