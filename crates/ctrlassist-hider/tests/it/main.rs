//! The client and the wire format, against a stand-in for the service.
//!
//! The service itself needs its user, its udev rule and real device
//! nodes; the `hider` virtual machine check covers it.

#![expect(clippy::unwrap_used, reason = "tests fail loudly")]

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::JoinHandle;

use ctrlassist_hider::{
    Client, Error, Refusal, Reply, Request, Token, pack_release, receive, send,
};
use rustix::net::{AddressFamily, SocketAddrUnix, SocketType, accept, bind, listen, socket};

/// A socket path no other test uses.
fn socket_path() -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let name = format!(
        "hider-{}-{}.sock",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_file(&path);
    path
}

/// Listens at a fresh path and runs `service` on the first connection.
fn stand_in(service: impl FnOnce(OwnedFd) + Send + 'static) -> (PathBuf, JoinHandle<()>) {
    let path = socket_path();
    let listener = socket(AddressFamily::UNIX, SocketType::SEQPACKET, None).unwrap();
    bind(&listener, &SocketAddrUnix::new(&path).unwrap()).unwrap();
    listen(&listener, 1).unwrap();
    let thread = std::thread::spawn(move || service(accept(&listener).unwrap()));
    (path, thread)
}

/// The next request on a connection, with the descriptors beside it.
fn next(connection: &OwnedFd) -> (Request, Vec<OwnedFd>) {
    let mut buffer = [0u8; Request::SIZE];
    let (length, fds) = receive(connection.as_fd(), &mut buffer).unwrap();
    (Request::decode(&buffer[..length]).unwrap(), fds)
}

/// A reply that did as asked, from a service that doesn't count.
fn ok(token: u32, dislodged: bool) -> Reply {
    Reply {
        refusal: None,
        token: Token(token),
        dislodged,
        rebinds: None,
    }
}

/// `reply`, from a service that counts `rebinds` re-binds for it.
fn counted(reply: Reply, rebinds: u8) -> Reply {
    Reply {
        rebinds: Some(rebinds),
        ..reply
    }
}

/// A descriptor that reads back `text`, standing in for an event node.
fn readable(text: &str) -> OwnedFd {
    let path = socket_path().with_extension("node");
    File::create(&path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    let file = File::open(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    file.into()
}

fn read_all(fd: OwnedFd) -> String {
    let mut text = String::new();
    File::from(fd).read_to_string(&mut text).unwrap();
    text
}

#[test]
fn every_request_and_reply_survives_the_wire() {
    for request in [
        Request::Hide,
        Request::Reopen(Token(7)),
        Request::Restore(Token(u32::MAX)),
        Request::Raw,
        Request::RawOf(Token(9)),
        Request::Version,
        Request::Cancel,
    ] {
        assert_eq!(Request::decode(&request.encode()), Some(request));
    }
    let refusals = [
        Refusal::NotPermitted,
        Refusal::NotAPad,
        Refusal::Busy,
        Refusal::UnknownToken,
        Refusal::Absent,
        Refusal::Failed,
        Refusal::BadRequest,
        Refusal::Unsupported,
        Refusal::Cancelled,
    ];
    for refusal in refusals {
        let reply = Reply::refused(refusal);
        assert_eq!(Reply::decode(&reply.encode()), Some(reply));
        assert!(!refusal.to_string().is_empty());
    }
    for reply in [ok(1, true), ok(0xdead_beef, false), counted(ok(3, true), 2)] {
        assert_eq!(Reply::decode(&reply.encode()), Some(reply));
    }
}

#[test]
fn what_is_not_a_message_is_not_decoded() {
    for bytes in [
        &[][..],
        &[1],
        &[1, 0, 0, 0],
        &[1, 0, 0, 0, 0, 0],
        &[0, 0, 0, 0, 0],
        &[9, 0, 0, 0, 0],
    ] {
        assert_eq!(Request::decode(bytes), None, "{bytes:?}");
    }
    for bytes in [
        &[][..],
        &[0, 0, 0, 0, 0],
        &[0, 0, 0, 0, 0, 0, 0],
        &[200, 0, 0, 0, 0, 0],
    ] {
        assert_eq!(Reply::decode(bytes), None, "{bytes:?}");
    }
}

/// The service says its release in the room a reply has for a token. A
/// service from before it could be asked refuses, as it does anything
/// it doesn't understand.
#[test]
fn the_service_s_release_travels_where_a_token_goes() {
    let (path, service) = stand_in(|connection| {
        let (request, fds) = next(&connection);
        assert_eq!(request, Request::Version);
        assert!(fds.is_empty());
        let packed = pack_release("12.34.56-rc.1+build");
        send(connection.as_fd(), &ok(packed, false).encode(), &[]).unwrap();
        let (request, _) = next(&connection);
        assert_eq!(request, Request::Version);
        let refused = Reply::refused(Refusal::BadRequest);
        send(connection.as_fd(), &refused.encode(), &[]).unwrap();
    });
    let client = Client::connect(&path).unwrap();
    assert_eq!(client.version().unwrap(), "12.34.56");
    assert!(matches!(
        client.version(),
        Err(Error::Refused(Refusal::BadRequest))
    ));
    service.join().unwrap();

    // The largest that fits, and what doesn't or isn't a release.
    assert_eq!(pack_release("4095.1023.1023"), u32::MAX);
    for not in ["", "1.2", "1.2.3.4", "one.2.3", "4096.0.0", "0.1024.0"] {
        assert_eq!(pack_release(not), 0, "{not}");
    }
}

#[test]
fn hide_passes_the_pad_s_descriptor_and_gets_one_back() {
    let (path, service) = stand_in(|connection| {
        let (request, fds) = next(&connection);
        assert_eq!(request, Request::Hide);
        let [pad] = <[OwnedFd; 1]>::try_from(fds).expect("exactly the pad's descriptor");
        assert_eq!(read_all(pad), "the pad");
        let event = readable("hidden");
        send(connection.as_fd(), &ok(7, true).encode(), &[event.as_fd()]).unwrap();
    });
    let client = Client::connect(&path).unwrap();
    let pad = readable("the pad");
    let hidden = client.hide(pad.as_fd()).unwrap();
    assert_eq!(hidden.token, Token(7));
    assert!(hidden.dislodged);
    assert_eq!(read_all(hidden.event), "hidden");
    service.join().unwrap();
}

#[test]
fn reopen_and_restore_name_the_pad_by_its_token() {
    let (path, service) = stand_in(|connection| {
        let (request, fds) = next(&connection);
        assert_eq!(request, Request::Reopen(Token(3)));
        assert!(fds.is_empty());
        let event = readable("again");
        send(connection.as_fd(), &ok(3, true).encode(), &[event.as_fd()]).unwrap();

        let (request, _) = next(&connection);
        assert_eq!(request, Request::Restore(Token(3)));
        send(connection.as_fd(), &ok(3, false).encode(), &[]).unwrap();
    });
    let client = Client::connect(&path).unwrap();
    let hidden = client.reopen(Token(3)).unwrap();
    assert_eq!(read_all(hidden.event), "again");
    client.restore(Token(3)).unwrap();
    service.join().unwrap();
}

#[test]
fn a_refusal_reaches_the_caller_as_the_reason_given() {
    let (path, service) = stand_in(|connection| {
        for refusal in [Refusal::NotPermitted, Refusal::UnknownToken] {
            let _ = next(&connection);
            send(connection.as_fd(), &Reply::refused(refusal).encode(), &[]).unwrap();
        }
    });
    let client = Client::connect(&path).unwrap();
    let pad = readable("the pad");
    let error = client.hide(pad.as_fd()).unwrap_err();
    assert!(
        matches!(error, Error::Refused(Refusal::NotPermitted)),
        "{error:?}"
    );
    assert!(
        error.to_string().contains("in front of the machine"),
        "{error}"
    );
    assert!(std::error::Error::source(&error).is_some());
    let error = client.restore(Token(9)).unwrap_err();
    assert!(
        matches!(error, Error::Refused(Refusal::UnknownToken)),
        "{error:?}"
    );
    service.join().unwrap();
}

#[test]
fn an_answer_that_makes_no_sense_is_a_protocol_error() {
    let (path, service) = stand_in(|connection| {
        // Not a reply at all.
        let _ = next(&connection);
        send(connection.as_fd(), b"nonsense", &[]).unwrap();
        // A success that should carry a descriptor and doesn't.
        let _ = next(&connection);
        send(connection.as_fd(), &ok(1, true).encode(), &[]).unwrap();
    });
    let client = Client::connect(&path).unwrap();
    let pad = readable("the pad");
    for _ in 0..2 {
        let error = client.hide(pad.as_fd()).unwrap_err();
        assert!(matches!(error, Error::Protocol), "{error:?}");
        assert!(std::error::Error::source(&error).is_none());
        assert!(!error.to_string().is_empty());
    }
    service.join().unwrap();
}

#[test]
fn a_service_that_is_not_there_or_goes_away_is_an_io_error() {
    let error = Client::connect(&socket_path()).unwrap_err();
    assert!(matches!(error, Error::Io(_)), "{error:?}");
    assert!(error.to_string().contains("couldn't be reached"), "{error}");
    assert!(std::error::Error::source(&error).is_some());

    // It accepts, then closes without answering.
    let (path, service) = stand_in(drop);
    let client = Client::connect(&path).unwrap();
    service.join().unwrap();
    let error = client.restore(Token(1)).unwrap_err();
    assert!(matches!(error, Error::Io(_) | Error::Protocol), "{error:?}");
}

/// What a service that counts says of a hide given up: the hide's reply,
/// then the cancel's, each with how many times the pad was re-bound,
/// which the client adds up. While it waits for the hide's reply the
/// client can be woken, and then cancels.
#[test]
fn a_hide_given_up_is_answered_with_what_it_took() {
    let (path, service) = stand_in(|connection| {
        let (request, _) = next(&connection);
        assert_eq!(request, Request::Version);
        send(
            connection.as_fd(),
            &counted(ok(pack_release("0.5.0"), false), 0).encode(),
            &[],
        )
        .unwrap();
        // The hide is held until the cancel comes, which ends it: re-bound
        // once to hide it and once to show it again.
        let (request, fds) = next(&connection);
        assert_eq!((request, fds.len()), (Request::Hide, 1));
        let (request, _) = next(&connection);
        assert_eq!(request, Request::Cancel);
        let ended = counted(Reply::refused(Refusal::Cancelled), 2);
        send(connection.as_fd(), &ended.encode(), &[]).unwrap();
        send(connection.as_fd(), &counted(ok(0, false), 0).encode(), &[]).unwrap();
        // A hide done before its cancel came, which shows it again.
        let _ = next(&connection);
        let event = readable("hidden");
        send(
            connection.as_fd(),
            &counted(ok(4, true), 1).encode(),
            &[event.as_fd()],
        )
        .unwrap();
        let (request, _) = next(&connection);
        assert_eq!(request, Request::Cancel);
        send(connection.as_fd(), &counted(ok(0, false), 1).encode(), &[]).unwrap();
        // And one with nothing to undo, from a service that doesn't count.
        let _ = next(&connection);
        send(
            connection.as_fd(),
            &Reply::refused(Refusal::Busy).encode(),
            &[],
        )
        .unwrap();
        let _ = next(&connection);
        send(connection.as_fd(), &ok(0, false).encode(), &[]).unwrap();
    });
    let client = Client::connect(&path).unwrap();
    assert_eq!(client.release().unwrap(), ("0.5.0".to_string(), true));
    let pad = readable("the pad");
    let (wake, woken) = std::os::unix::net::UnixStream::pair().unwrap();
    client.send_hide(pad.as_fd()).unwrap();
    // Nothing has come yet: the wake ends the wait.
    (&woken).write_all(b"!").unwrap();
    client.wait(Some(wake.as_fd())).unwrap();
    client.cancel().unwrap();
    assert_eq!(client.cancelled().unwrap(), 2);

    client.send_hide(pad.as_fd()).unwrap();
    client.cancel().unwrap();
    assert_eq!(client.cancelled().unwrap(), 2);

    client.send_hide(pad.as_fd()).unwrap();
    client.cancel().unwrap();
    assert!(matches!(client.cancelled(), Err(Error::Protocol)));
    service.join().unwrap();
}

/// A hide's reply says how many times the pad was re-bound, refused or
/// not; a service from before says nothing, nor that it counts.
#[test]
fn a_hide_s_reply_says_how_many_times_the_pad_was_re_bound() {
    let (path, service) = stand_in(|connection| {
        let _ = next(&connection);
        let event = readable("hidden");
        send(
            connection.as_fd(),
            &counted(ok(2, true), 1).encode(),
            &[event.as_fd()],
        )
        .unwrap();
        let _ = next(&connection);
        let refused = counted(Reply::refused(Refusal::Failed), 2);
        send(connection.as_fd(), &refused.encode(), &[]).unwrap();
        let _ = next(&connection);
        send(
            connection.as_fd(),
            &ok(pack_release("0.5.0"), false).encode(),
            &[],
        )
        .unwrap();
        let (request, _) = next(&connection);
        assert_eq!(request, Request::Restore(Token(2)));
        send(connection.as_fd(), &counted(ok(2, false), 1).encode(), &[]).unwrap();
    });
    let client = Client::connect(&path).unwrap();
    let pad = readable("the pad");
    client.send_hide(pad.as_fd()).unwrap();
    client.wait(None).unwrap();
    let hid = client.hid().unwrap();
    assert_eq!(hid.rebinds, Some(1));
    assert_eq!(read_all(hid.outcome.unwrap().event), "hidden");
    client.send_hide(pad.as_fd()).unwrap();
    let hid = client.hid().unwrap();
    assert_eq!(
        (hid.outcome.unwrap_err(), hid.rebinds),
        (Refusal::Failed, Some(2))
    );
    assert_eq!(client.release().unwrap(), ("0.5.0".to_string(), false));
    assert_eq!(client.restore(Token(2)).unwrap(), Some(1));
    service.join().unwrap();
}

/// A refusal that says no count, as one at the service's gate doesn't,
/// re-bound nothing: a hide's, and a hide's and a cancel's given up. And
/// the reply to a request given up waiting for is read past, to the
/// reply to the one asked after it.
#[test]
fn a_refusal_that_says_no_count_re_bound_nothing() {
    let (path, service) = stand_in(|connection| {
        let refused = Reply::refused(Refusal::NotPermitted).encode();
        for _ in 0..3 {
            let _ = next(&connection);
            send(connection.as_fd(), &refused, &[]).unwrap();
        }
        // A reopen given up, then a restore asked after it.
        let (request, _) = next(&connection);
        assert_eq!(request, Request::Reopen(Token(3)));
        let event = readable("hidden");
        send(
            connection.as_fd(),
            &counted(ok(3, true), 0).encode(),
            &[event.as_fd()],
        )
        .unwrap();
        let (request, _) = next(&connection);
        assert_eq!(request, Request::Restore(Token(3)));
        send(connection.as_fd(), &counted(ok(3, false), 1).encode(), &[]).unwrap();
    });
    let client = Client::connect(&path).unwrap();
    let pad = readable("the pad");
    client.send_hide(pad.as_fd()).unwrap();
    let hid = client.hid().unwrap();
    assert_eq!(
        (hid.outcome.unwrap_err(), hid.rebinds),
        (Refusal::NotPermitted, Some(0))
    );
    client.send_hide(pad.as_fd()).unwrap();
    client.cancel().unwrap();
    assert_eq!(client.cancelled().unwrap(), 0);

    client.send_reopen(Token(3)).unwrap();
    client.skip().unwrap();
    assert_eq!(client.restore(Token(3)).unwrap(), Some(1));
    service.join().unwrap();
}
