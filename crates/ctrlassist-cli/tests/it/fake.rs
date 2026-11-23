//! A stand-in daemon for what a real one can't be made to do on cue: one
//! that has withdrawn its token and still answers, as while it stops; one
//! whose token is up and that doesn't answer; and one whose answers
//! quote a profile's text with control characters in it. It makes no
//! pads.

use std::path::Path;

use ctrlassist_protocol::endpoint::{daemon, profile};
use ctrlassist_protocol::error::{ErrorCode, WireError};
use ctrlassist_protocol::payload::{Problem, Profile};
use ctrlassist_protocol::{VERSION, keys, rpc};
use tokio::task::JoinHandle;
use zenoh::Session;
use zenoh::liveliness::LivelinessToken;

/// Text that would clear the screen, as a profile's author might write.
pub(crate) const HOSTILE: &str = "\u{1b}[2J";

/// The problem the stand-in finds with any profile: a reference to a
/// node by a name with [`HOSTILE`] in it.
pub(crate) fn problem() -> Problem {
    Problem {
        at: "node.mux.inputs[0]".to_string(),
        message: format!("no source or node is called \"{HOSTILE}\""),
    }
}

/// A running stand-in.
pub(crate) struct FakeDaemon {
    session: Session,
    _token: Option<LivelinessToken>,
    serving: Vec<JoinHandle<()>>,
}

impl FakeDaemon {
    /// Listen on `socket`; answer the daemon's rpcs when `answers`, and
    /// hold its liveliness token when `alive`.
    pub(crate) async fn start(socket: &Path, answers: bool, alive: bool) -> Self {
        let session = zenoh::open(ctrlassist_protocol::bootstrap::router_config(socket).unwrap())
            .await
            .unwrap();
        let mut serving = Vec::new();
        if answers {
            serving.push(serve_daemon(&session).await);
            serving.push(serve_profiles(&session).await);
        }
        let token = if alive {
            Some(
                session
                    .liveliness()
                    .declare_token(keys::daemon_alive())
                    .await
                    .unwrap(),
            )
        } else {
            None
        };
        Self {
            session,
            _token: token,
            serving,
        }
    }

    pub(crate) async fn stop(self) {
        for serving in &self.serving {
            serving.abort();
        }
        drop(self._token);
        self.session.close().await.unwrap();
    }
}

async fn serve_daemon(session: &Session) -> JoinHandle<()> {
    let queryable = session
        .declare_queryable(keys::daemon_rpc_wild())
        .await
        .unwrap();
    tokio::spawn(async move {
        while let Ok(query) = queryable.recv_async().await {
            match daemon::Incoming::receive(query).await {
                Some(((), daemon::Incoming::Info((), responder))) => {
                    let info = rpc::ServiceInfo {
                        version: VERSION.to_string(),
                        keys: Vec::new(),
                        config: String::new(),
                    };
                    responder.ok(&info).await;
                }
                Some(((), daemon::Incoming::Reload((), responder))) => {
                    let name = format!("odd{HOSTILE}");
                    let reloaded = rpc::Reloaded {
                        changed: vec![name.clone()],
                        removed: vec![name.clone()],
                        wrong: vec![Profile {
                            name: name.clone(),
                            toml: String::new(),
                            problem: Some(problem()),
                            unset: Vec::new(),
                        }],
                        updating: vec![name],
                    };
                    responder.ok(&reloaded).await;
                }
                None => {}
            }
        }
    })
}

async fn serve_profiles(session: &Session) -> JoinHandle<()> {
    let queryable = session
        .declare_queryable(keys::profile_rpc_wild())
        .await
        .unwrap();
    tokio::spawn(async move {
        while let Ok(query) = queryable.recv_async().await {
            match profile::Incoming::receive(query).await {
                Some(((), profile::Incoming::Validate(_, responder))) => {
                    let validation = rpc::Validation {
                        problem: Some(problem()),
                    };
                    responder.ok(&validation).await;
                }
                Some(((), profile::Incoming::Put(_, responder))) => {
                    let refusal =
                        WireError::refusal(ErrorCode::InvalidProfile, problem().to_string());
                    responder.err(&refusal).await;
                }
                Some(((), profile::Incoming::Schema((), responder))) => {
                    responder.err(&WireError::internal("not served")).await;
                }
                Some(((), profile::Incoming::Delete(_, responder))) => {
                    responder.err(&WireError::internal("not served")).await;
                }
                None => {}
            }
        }
    })
}
