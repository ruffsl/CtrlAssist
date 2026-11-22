//! The SDK's error: named for what a caller can act on. Dependency
//! errors ride as boxed sources and never appear in a signature.

use std::fmt;

use ctrlassist_protocol::caller::RpcError;
use ctrlassist_protocol::error::ErrorCode;

/// A boxed cause, kept for `source()` chains and logs.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Why an SDK call failed.
#[non_exhaustive]
#[derive(Debug)]
pub enum Error {
    /// The service refused the request. Nothing was applied; branch on
    /// `code`.
    Rpc {
        /// Why the service refused, for a caller to branch on.
        code: ErrorCode,
        /// The service's explanation, in English, for a person to read.
        message: String,
    },
    /// The service isn't running, or isn't reachable yet: nothing took
    /// the call before the deadline, so a write wasn't applied. Retry
    /// once the client's `is_available` turns true. A socket this
    /// process may not open looks the same.
    Unavailable {
        /// The service the call went to.
        service: Service,
    },
    /// No answer (timed out, or the service dropped the call). For a
    /// write, the outcome is unknown.
    Timeout {
        /// The service the call went to.
        service: Service,
    },
    /// The local link failed: opening the session, a subscription, or a
    /// stream that ended early.
    Transport {
        /// What the SDK was doing when the link failed.
        what: &'static str,
        /// The underlying failure, when there is one.
        source: Option<BoxError>,
    },
    /// The service runs a release this client can't speak to (a
    /// different `major.minor`). Restart the service or upgrade so the
    /// two match.
    VersionMismatch {
        /// The mismatched service.
        service: Service,
        /// The protocol release this SDK speaks.
        ours: String,
        /// The service's release.
        theirs: String,
    },
    /// A reply didn't decode as its rpc's response type.
    Protocol {
        /// The service that sent the reply.
        service: Service,
        /// The decode failure, naming the rpc.
        source: BoxError,
    },
}

/// Which service a call went to.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    /// The daemon, which owns the pads, the profiles and the setups
    /// ([`DaemonClient`]).
    ///
    /// [`DaemonClient`]: crate::DaemonClient
    Daemon,
    /// The tray's settings service, which owns its notification
    /// switches ([`TrayClient`]).
    ///
    /// [`TrayClient`]: crate::TrayClient
    Tray,
}

impl fmt::Display for Service {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Daemon => "daemon",
            Self::Tray => "tray",
        })
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rpc { code, message } => write!(f, "{message} ({code})"),
            Self::Unavailable { service } => write!(f, "the {service} is unavailable"),
            Self::Timeout { service } => write!(f, "the {service} didn't answer"),
            Self::Transport { what, .. } => f.write_str(what),
            Self::VersionMismatch {
                service,
                ours,
                theirs,
            } => write!(
                f,
                "the {service} runs release {theirs}, which this client ({ours}) can't talk to"
            ),
            Self::Protocol { service, .. } => write!(f, "malformed reply from the {service}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport {
                source: Some(source),
                ..
            }
            | Self::Protocol { source, .. } => Some(source.as_ref()),
            _ => None,
        }
    }
}

/// The SDK's result type.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// The service's refusal code, when the service refused.
    #[must_use]
    pub fn rpc_code(&self) -> Option<ErrorCode> {
        match self {
            Self::Rpc { code, .. } => Some(*code),
            _ => None,
        }
    }

    /// A local-link failure with its cause.
    pub(crate) fn transport(what: &'static str, source: impl Into<BoxError>) -> Self {
        Self::Transport {
            what,
            source: Some(source.into()),
        }
    }

    /// Map a call's outcome to the SDK's error, naming the service. A
    /// transport or decode failure keeps the call's error (its key, and
    /// zenoh's or serde's cause) as the source.
    pub(crate) fn from_rpc(service: Service, error: RpcError) -> Self {
        match error {
            RpcError::Service(refusal) => Self::Rpc {
                code: refusal.code,
                message: refusal.message,
            },
            RpcError::Unavailable { .. } => Self::Unavailable { service },
            RpcError::Timeout { .. } | RpcError::NoReply { .. } => Self::Timeout { service },
            error @ RpcError::Undecodable { .. } => Self::Protocol {
                service,
                source: Box::new(error),
            },
            // A transport failure, or one the protocol added later: keep
            // it as the source.
            error => Self::Transport {
                what: "rpc transport failure",
                source: Some(Box::new(error)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use ctrlassist_protocol::error::WireError;

    use super::*;

    #[test]
    fn transport_keeps_its_cause_in_source() {
        let cause = std::io::Error::new(std::io::ErrorKind::NotFound, "no such socket");
        let error = Error::transport("connect to the daemon socket", cause);
        assert_eq!(error.to_string(), "connect to the daemon socket");
        let source = std::error::Error::source(&error).expect("transport carries its cause");
        assert_eq!(source.to_string(), "no such socket");
    }

    #[test]
    fn call_outcomes_map_onto_what_a_caller_can_act_on() {
        let key = || "ctrlassist/rpc/daemon/info".to_string();
        let from_rpc = |error| Error::from_rpc(Service::Daemon, error);
        let refusal = from_rpc(RpcError::Service(WireError::bad_request("no")));
        assert_eq!(refusal.rpc_code(), Some(ErrorCode::BadRequest));
        assert_eq!(refusal.to_string(), "no (bad-request)");
        assert!(std::error::Error::source(&refusal).is_none());

        let dropped = from_rpc(RpcError::NoReply { key: key() });
        assert!(matches!(
            dropped,
            Error::Timeout {
                service: Service::Daemon
            }
        ));
        assert_eq!(dropped.to_string(), "the daemon didn't answer");
        let slow = from_rpc(RpcError::Timeout { key: key() });
        assert!(matches!(slow, Error::Timeout { .. }));

        let absent = from_rpc(RpcError::Unavailable { key: key() });
        assert_eq!(absent.rpc_code(), None);
        assert_eq!(absent.to_string(), "the daemon is unavailable");
        let no_tray = Error::from_rpc(Service::Tray, RpcError::Unavailable { key: key() });
        assert_eq!(no_tray.to_string(), "the tray is unavailable");

        let malformed = from_rpc(RpcError::Undecodable {
            key: key(),
            source: serde_json::from_str::<u8>("\"x\"").unwrap_err(),
        });
        assert!(matches!(malformed, Error::Protocol { .. }));
        // The chain names the rpc and serde's reason, for the log line a
        // version-skewed client produces.
        let mut chain = malformed.to_string();
        let mut source = std::error::Error::source(&malformed);
        while let Some(cause) = source {
            chain.push_str(": ");
            chain.push_str(&cause.to_string());
            source = cause.source();
        }
        assert!(chain.contains("malformed reply"), "{chain}");
        assert!(chain.contains("daemon/info"), "{chain}");
        assert!(chain.contains("invalid type"), "{chain}");

        let broken = from_rpc(RpcError::Transport {
            key: key(),
            source: "the session closed".into(),
        });
        assert_eq!(broken.to_string(), "rpc transport failure");
        assert!(matches!(broken, Error::Transport { .. }));
        let cause = std::error::Error::source(&broken).expect("the call's error is kept");
        assert!(cause.to_string().contains("daemon/info"), "{cause}");

        let mismatch = Error::VersionMismatch {
            service: Service::Daemon,
            ours: "0.5.0".into(),
            theirs: "9.0.0".into(),
        };
        assert_eq!(
            mismatch.to_string(),
            "the daemon runs release 9.0.0, which this client (0.5.0) can't talk to"
        );
    }
}
