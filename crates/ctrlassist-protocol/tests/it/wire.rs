//! The rpc table on a real link: a stand-in daemon on a temporary socket
//! and a client calling it.

use std::path::PathBuf;

use ctrlassist_protocol::caller::{Caller, RpcError};
use ctrlassist_protocol::endpoint::daemon;
use ctrlassist_protocol::error::{ErrorCode, WireError};
use ctrlassist_protocol::rpc::ServiceInfo;
use ctrlassist_protocol::{VERSION, bootstrap, compatible};
use tempfile::TempDir;
use zenoh::Session;

/// A socket path short enough for the kernel's limit, which a path under
/// the build tree may not be.
fn socket() -> (TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("ca")
        .tempdir_in("/tmp")
        .unwrap();
    let socket = dir.path().join("zenoh.sock");
    (dir, socket)
}

/// A stand-in daemon that answers `info` with `answer`.
async fn serve(
    socket: &std::path::Path,
    answer: fn() -> Result<ServiceInfo, WireError>,
) -> Session {
    let server = zenoh::open(bootstrap::router_config(socket).unwrap())
        .await
        .unwrap();
    let queryable = server
        .declare_queryable(daemon::Incoming::wild())
        .await
        .unwrap();
    tokio::spawn(async move {
        while let Ok(query) = queryable.recv_async().await {
            match daemon::Incoming::receive(query).await {
                Some(((), daemon::Incoming::Info((), responder))) => {
                    responder.answer(answer()).await;
                }
                Some(((), daemon::Incoming::Reload((), responder))) => {
                    responder
                        .ok(&ctrlassist_protocol::rpc::Reloaded::default())
                        .await;
                }
                // Refused already, with a typed error.
                None => {}
            }
        }
    });
    server
}

fn this_release() -> Result<ServiceInfo, WireError> {
    Ok(ServiceInfo {
        version: VERSION.to_string(),
        keys: Vec::new(),
    })
}

/// The refusal a raw query on `key` gets, for requests no typed call can
/// make.
async fn refusal(client: &Session, key: &str, body: &str) -> WireError {
    let replies = client.get(key).payload(body.to_string()).await.unwrap();
    let reply = replies.recv_async().await.unwrap();
    let error = reply.result().unwrap_err();
    serde_json::from_slice(&error.payload().to_bytes()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_asks_the_daemon_its_release_and_finds_it_compatible() {
    let (_dir, socket) = socket();
    let _server = serve(&socket, this_release).await;
    let client = zenoh::open(bootstrap::client_config(&socket).unwrap())
        .await
        .unwrap();
    let caller = Caller::new(client);
    let info = caller.call::<daemon::Info>(&(), &()).await.unwrap();
    assert_eq!(info.version, VERSION);
    assert!(compatible(VERSION, &info.version));
    // A second call reuses the declared querier.
    assert_eq!(caller.call::<daemon::Info>(&(), &()).await.unwrap(), info);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_arrives_as_the_typed_error_the_daemon_sent() {
    let (_dir, socket) = socket();
    let _server = serve(&socket, || Err(WireError::internal("the stand-in refuses"))).await;
    let client = zenoh::open(bootstrap::client_config(&socket).unwrap())
        .await
        .unwrap();
    let error = Caller::new(client)
        .call::<daemon::Info>(&(), &())
        .await
        .unwrap_err();
    match &error {
        RpcError::Service(refusal) => {
            assert_eq!(refusal.code, ErrorCode::Internal);
            assert_eq!(refusal.message, "the stand-in refuses");
        }
        other => unreachable!("expected the daemon's refusal, got {other:?}"),
    }
    assert_eq!(error.to_string(), "the stand-in refuses (internal)");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_the_table_does_not_hold_is_refused_before_any_handler() {
    let (_dir, socket) = socket();
    let _server = serve(&socket, this_release).await;
    let client = zenoh::open(bootstrap::client_config(&socket).unwrap())
        .await
        .unwrap();

    let unknown = refusal(&client, "ctrlassist/rpc/daemon/nonsense", "").await;
    assert_eq!(unknown.code, ErrorCode::UnknownRpc);
    assert_eq!(unknown.message, "no such rpc: nonsense");

    // `info` takes no body; one that isn't JSON at all is a bad request.
    let garbled = refusal(&client, "ctrlassist/rpc/daemon/info", "{ not json").await;
    assert_eq!(garbled.code, ErrorCode::BadRequest);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_may_start_before_the_daemon_and_calls_once_it_arrives() {
    let (_dir, socket) = socket();
    // Nothing listens yet: the link dials in the background.
    let client = zenoh::open(bootstrap::client_config_resilient(&socket).unwrap())
        .await
        .unwrap();
    let caller = Caller::new(client);
    match caller.call::<daemon::Info>(&(), &()).await {
        Err(RpcError::Unavailable { key }) => assert_eq!(key, "ctrlassist/rpc/daemon/info"),
        other => unreachable!("expected no daemon to be found, got {other:?}"),
    }

    let _server = serve(&socket, this_release).await;
    // The call waits for the daemon's rpc to become routable; the link's
    // own retry decides how soon that is.
    let mut outcome = caller.call::<daemon::Info>(&(), &()).await;
    for _ in 0..10 {
        if outcome.is_ok() {
            break;
        }
        outcome = caller.call::<daemon::Info>(&(), &()).await;
    }
    assert_eq!(outcome.unwrap().version, VERSION);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_one_shot_client_fails_at_once_when_no_daemon_is_listening() {
    let (_dir, socket) = socket();
    assert!(
        zenoh::open(bootstrap::client_config(&socket).unwrap())
            .await
            .is_err()
    );
}

/// A stand-in daemon that handles each `info` query with `handle`, for
/// the ways a real one can misbehave.
async fn misbehave<F, Fut>(socket: &std::path::Path, handle: F) -> Session
where
    F: Fn(zenoh::query::Query) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send,
{
    let server = zenoh::open(bootstrap::router_config(socket).unwrap())
        .await
        .unwrap();
    let queryable = server
        .declare_queryable(daemon::Incoming::wild())
        .await
        .unwrap();
    tokio::spawn(async move {
        while let Ok(query) = queryable.recv_async().await {
            handle(query).await;
        }
    });
    server
}

#[tokio::test(flavor = "multi_thread")]
async fn a_daemon_that_drops_a_request_unanswered_is_reported_as_no_reply() {
    let (_dir, socket) = socket();
    // The typed responder is dropped without answering; the daemon logs
    // it, and the caller's query closes empty.
    let _server = misbehave(&socket, |query| async move {
        let received = daemon::Incoming::receive(query).await;
        assert!(format!("{received:?}").contains("ctrlassist/rpc/daemon/info"));
        drop(received);
    })
    .await;
    let client = zenoh::open(bootstrap::client_config(&socket).unwrap())
        .await
        .unwrap();
    let error = Caller::new(client)
        .call::<daemon::Info>(&(), &())
        .await
        .unwrap_err();
    assert!(matches!(error, RpcError::NoReply { .. }), "{error:?}");
    assert_eq!(
        error.to_string(),
        "rpc ctrlassist/rpc/daemon/info: no reply"
    );
    assert!(std::error::Error::source(&error).is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_or_a_refusal_of_the_wrong_shape_is_reported_as_undecodable() {
    let (_dir, socket) = socket();
    let _server = misbehave(&socket, |query| async move {
        let key = query.key_expr().clone().into_owned();
        // First a reply that isn't a `ServiceInfo`, then a refusal that
        // isn't a `WireError`.
        static CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
            query.reply(key, r#"{"not":"service info"}"#).await.unwrap();
        } else {
            query.reply_err("some other daemon's words").await.unwrap();
        }
    })
    .await;
    let client = zenoh::open(bootstrap::client_config(&socket).unwrap())
        .await
        .unwrap();
    let caller = Caller::new(client);
    for _ in 0..2 {
        let error = caller.call::<daemon::Info>(&(), &()).await.unwrap_err();
        assert!(matches!(error, RpcError::Undecodable { .. }), "{error:?}");
        assert_eq!(
            error.to_string(),
            "rpc ctrlassist/rpc/daemon/info: undecodable reply"
        );
        assert!(std::error::Error::source(&error).is_some());
    }
}

#[test]
fn every_way_a_call_can_fail_says_which_rpc_and_how() {
    let key = || "ctrlassist/rpc/daemon/info".to_string();
    let said = |error: RpcError| error.to_string();
    assert_eq!(
        said(RpcError::Unavailable { key: key() }),
        "no rpc service for ctrlassist/rpc/daemon/info within 2s"
    );
    assert_eq!(
        said(RpcError::Timeout { key: key() }),
        "rpc ctrlassist/rpc/daemon/info: timed out"
    );
    let transport = RpcError::Transport {
        key: key(),
        source: "the link is gone".into(),
    };
    assert!(std::error::Error::source(&transport).is_some());
    assert_eq!(
        said(transport),
        "rpc ctrlassist/rpc/daemon/info: transport failure"
    );
    let refused = RpcError::Service(WireError::bad_request("no"));
    assert!(std::error::Error::source(&refused).is_none());
}

#[test]
fn the_daemon_s_socket_is_in_the_user_s_runtime_directory() {
    let path = bootstrap::default_socket_path();
    assert!(
        path.ends_with("ctrlassist/zenoh.sock"),
        "{}",
        path.display()
    );
    // A path zenoh can't name is a typed error that says so.
    use std::os::unix::ffi::OsStrExt;
    let odd = std::path::Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff.sock"));
    let error = bootstrap::router_config(odd).unwrap_err();
    assert!(error.to_string().contains("isn't valid UTF-8"), "{error}");
    assert!(std::error::Error::source(&error).is_none());
}
