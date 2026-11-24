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
    Client, Error, Refusal, Reply, Request, Token, is_unknown_to_the_kernel, pack_release, receive,
    send, take_descriptors,
};
use rustix::event::{PollFd, PollFlags, poll};
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

/// A reply that did as asked.
fn ok(token: u32, dislodged: bool) -> Reply {
    Reply {
        refusal: None,
        token: Token(token),
        dislodged,
    }
}

/// Answers the release the client asks first on a connection, before it
/// sends a descriptor.
fn admit(connection: &OwnedFd) {
    let (request, fds) = next(connection);
    assert_eq!((request, fds.len()), (Request::Version, 0));
    let release = ok(pack_release("1.2.3"), false);
    send(connection.as_fd(), &release.encode(), &[]).unwrap();
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
    ];
    for (status, refusal) in (1..).zip(refusals) {
        let reply = Reply::refused(refusal);
        assert_eq!(reply.encode(), [status, 0, 0, 0, 0, 0]);
        assert_eq!(Reply::decode(&reply.encode()), Some(reply));
        assert!(!refusal.to_string().is_empty());
    }
    for reply in [ok(1, true), ok(0xdead_beef, false)] {
        assert_eq!(Reply::decode(&reply.encode()), Some(reply));
    }
    // The flags say whether the pad was dislodged, in bit 0, and nothing
    // else is read from them.
    assert_eq!(ok(3, true).encode()[5], 1);
    assert_eq!(Reply::decode(&[0, 3, 0, 0, 0, 0xff]), Some(ok(3, true)));
}

#[test]
fn what_is_not_a_message_is_not_decoded() {
    for bytes in [
        &[][..],
        &[1],
        &[1, 0, 0, 0],
        &[1, 0, 0, 0, 0, 0],
        &[0, 0, 0, 0, 0],
        &[7, 0, 0, 0, 0],
        &[9, 0, 0, 0, 0],
    ] {
        assert_eq!(Request::decode(bytes), None, "{bytes:?}");
    }
    for bytes in [
        &[][..],
        &[0, 0, 0, 0, 0],
        &[0, 0, 0, 0, 0, 0, 0],
        &[9, 0, 0, 0, 0, 0],
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
        admit(&connection);
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
        admit(&connection);
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
        admit(&connection);
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

/// A hide sent without waiting: the wait for its reply ends at the reply,
/// or sooner at a wake, and the reply is read once it has come, a refusal
/// as the reason given. And the reply to a request given up waiting for
/// is read past, to the reply to the one asked after it.
#[test]
fn a_hide_sent_is_waited_for_and_read_and_a_reply_given_up_is_read_past() {
    let (woken_up, held) = std::sync::mpsc::channel::<()>();
    let (path, service) = stand_in(move |connection| {
        admit(&connection);
        // Answered once the client has been woken without it.
        let (request, fds) = next(&connection);
        assert_eq!((request, fds.len()), (Request::Hide, 1));
        held.recv().unwrap();
        let event = readable("hidden");
        send(connection.as_fd(), &ok(2, true).encode(), &[event.as_fd()]).unwrap();
        let _ = next(&connection);
        let refused = Reply::refused(Refusal::Failed);
        send(connection.as_fd(), &refused.encode(), &[]).unwrap();
        // A reopen given up, then a restore asked after it.
        let (request, _) = next(&connection);
        assert_eq!(request, Request::Reopen(Token(2)));
        let event = readable("again");
        send(connection.as_fd(), &ok(2, true).encode(), &[event.as_fd()]).unwrap();
        let (request, _) = next(&connection);
        assert_eq!(request, Request::Restore(Token(2)));
        send(connection.as_fd(), &ok(2, false).encode(), &[]).unwrap();
    });
    let client = Client::connect(&path).unwrap();
    let pad = readable("the pad");
    let (wake, woken) = std::os::unix::net::UnixStream::pair().unwrap();
    client.send_hide(pad.as_fd()).unwrap();
    // Nothing has come yet: the wake ends the wait.
    (&woken).write_all(b"!").unwrap();
    client.wait(Some(wake.as_fd())).unwrap();
    woken_up.send(()).unwrap();
    client.wait(None).unwrap();
    assert_eq!(read_all(client.hid().unwrap().event), "hidden");

    client.send_hide(pad.as_fd()).unwrap();
    assert!(matches!(client.hid(), Err(Error::Refused(Refusal::Failed))));

    client.send_reopen(Token(2)).unwrap();
    client.skip().unwrap();
    client.restore(Token(2)).unwrap();
    service.join().unwrap();
}

/// A stand-in that, as the service does, takes no descriptor on a
/// connection until it has admitted it, which it does only once the
/// client has sent something: a descriptor sent before then fails at the
/// client. None on a kernel that can't refuse them.
fn refusing_stand_in(
    service: impl FnOnce(OwnedFd) + Send + 'static,
) -> Option<(PathBuf, JoinHandle<()>)> {
    let path = socket_path();
    let listener = socket(AddressFamily::UNIX, SocketType::SEQPACKET, None).unwrap();
    match take_descriptors(listener.as_fd(), false) {
        Err(error) if is_unknown_to_the_kernel(&error) => return None,
        taken => taken.unwrap(),
    }
    bind(&listener, &SocketAddrUnix::new(&path).unwrap()).unwrap();
    listen(&listener, 1).unwrap();
    let thread = std::thread::spawn(move || {
        let connection = accept(&listener).unwrap();
        // Admitted only once something has come, so the client can't
        // have been answered yet.
        let mut fds = [PollFd::new(&connection, PollFlags::IN)];
        poll(&mut fds, None).unwrap();
        take_descriptors(connection.as_fd(), true).unwrap();
        service(connection);
    });
    Some((path, thread))
}

/// A hide on a connection the service hasn't answered asks the release
/// first, and passes the pad's descriptor only once the reply has come.
#[test]
fn a_hide_on_a_new_connection_waits_for_a_first_reply() {
    let Some((path, service)) = refusing_stand_in(|connection| {
        admit(&connection);
        let (request, fds) = next(&connection);
        assert_eq!(request, Request::Hide);
        let [pad] = <[OwnedFd; 1]>::try_from(fds).expect("exactly the pad's descriptor");
        send(connection.as_fd(), &ok(7, true).encode(), &[pad.as_fd()]).unwrap();
        let (request, fds) = next(&connection);
        assert_eq!((request, fds.len()), (Request::Raw, 1));
        send(
            connection.as_fd(),
            &ok(0, false).encode(),
            &[fds[0].as_fd()],
        )
        .unwrap();
    }) else {
        return;
    };
    let client = Client::connect(&path).unwrap();
    let hidden = client.hide(readable("the pad").as_fd()).unwrap();
    assert_eq!(read_all(hidden.event), "the pad");
    // Answered already: nothing more is asked first.
    let raw = client.raw(readable("its raw node").as_fd()).unwrap();
    assert_eq!(read_all(raw), "its raw node");
    service.join().unwrap();
}

/// With a request sent and its reply not yet read, a descriptor waits for
/// that reply to come, and leaves it to be read: nothing more is asked.
#[test]
fn a_descriptor_sent_behind_a_request_waits_for_that_request_s_reply() {
    let Some((path, service)) = refusing_stand_in(|connection| {
        let (request, _) = next(&connection);
        assert_eq!(request, Request::Reopen(Token(4)));
        send(
            connection.as_fd(),
            &Reply::refused(Refusal::UnknownToken).encode(),
            &[],
        )
        .unwrap();
        let (request, fds) = next(&connection);
        assert_eq!((request, fds.len()), (Request::Hide, 1));
        send(connection.as_fd(), &ok(5, true).encode(), &[fds[0].as_fd()]).unwrap();
    }) else {
        return;
    };
    let client = Client::connect(&path).unwrap();
    client.send_reopen(Token(4)).unwrap();
    client.send_hide(readable("the pad").as_fd()).unwrap();
    assert!(matches!(
        client.reopened(),
        Err(Error::Refused(Refusal::UnknownToken))
    ));
    assert_eq!(client.hid().unwrap().token, Token(5));
    service.join().unwrap();
}
