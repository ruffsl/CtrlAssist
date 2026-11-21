//! Typed rpc endpoints: every rpc is declared once, here, as a key plus
//! its request and response types, and both sides derive from that
//! declaration. Clients call through [`Caller`](crate::caller::Caller)
//! with the marker type. Servers receive a query as their family's
//! `Incoming` enum and match it exhaustively: each variant carries the
//! decoded request and a [`Responder`](crate::serve::Responder) typed by
//! that rpc, so a family member without a handler, a handler fed the
//! wrong body type, or a reply of another rpc's type doesn't compile.

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::WireError;
use crate::{keys, rpc};

/// One rpc: where it lives and what it carries.
pub trait Rpc {
    /// What addresses a call: `()` for an rpc of the daemon itself.
    type Target: ?Sized;
    /// The request body, sent as JSON; `()` for an rpc that takes none.
    type Request: Serialize + DeserializeOwned;
    /// The reply body.
    type Response: Serialize + DeserializeOwned;
    /// The rpc's name within its family, for logs.
    const NAME: &'static str;
    /// The rpc's key.
    fn key(target: &Self::Target) -> String;
}

/// An rpc answered by exactly one reply.
pub trait Unary: Rpc {
    /// Safe to re-issue when a matched query closes without a reply:
    /// reads, which neither mint state nor apply a relative effect.
    const REISSUE: bool;
}

/// A family key parser's result: the verb, plus the target for a family
/// whose keys carry one.
pub(crate) trait ParsedKey<'a> {
    type Target;
    fn split(self) -> (Self::Target, &'a str);
}

impl<'a> ParsedKey<'a> for &'a str {
    type Target = ();
    fn split(self) -> ((), &'a str) {
        ((), self)
    }
}

/// Decode an rpc body; an absent payload reads as JSON `null`.
fn decode_body<T: DeserializeOwned>(payload: &[u8]) -> Result<T, WireError> {
    let payload = if payload.is_empty() {
        b"null".as_slice()
    } else {
        payload
    };
    serde_json::from_slice(payload).map_err(|e| WireError::bad_request(e.to_string()))
}

/// Declare one family of verb-shaped rpcs: a marker type per rpc
/// implementing [`Rpc`] and [`Unary`] (`read` or `write`); the family's
/// `Call` enum carrying each rpc's decoded request; and its `Incoming`
/// enum pairing that request with the rpc's
/// [`Responder`](crate::serve::Responder).
///
/// `key` builds an rpc's key and `parse` splits one back into its target
/// and verb; `wild` is the wildcard the family's queryable declares. The
/// `key` form reads like a closure but is spliced into each marker's
/// [`Rpc::key`], with `verb` bound to that rpc's verb string.
macro_rules! family {
    (
        $(#[$family_meta:meta])*
        target: $target:ty,
        key: |$tgt:ident, $verb:ident| $key:expr,
        parse: $parse:path,
        wild: $wild:path,
        rpcs: {
            $(
                $(#[$meta:meta])*
                $name:ident = $verb_str:literal ($request:ty) -> $response:ty, $kind:ident;
            )*
        }
    ) => {
        $(
            $(#[$meta])*
            #[derive(Debug, Clone, Copy)]
            pub struct $name;

            impl $crate::endpoint::Rpc for $name {
                type Target = $target;
                type Request = $request;
                type Response = $response;
                const NAME: &'static str = $verb_str;
                fn key($tgt: &$target) -> String {
                    let $verb = $verb_str;
                    $key
                }
            }

            family!(@kind $name $kind);
        )*

        $(#[$family_meta])*
        #[derive(Debug)]
        pub(crate) enum Call {
            $(
                #[doc = concat!("[`", stringify!($name), "`]'s decoded request.")]
                $name($request),
            )*
        }

        impl Call {
            /// Decode a query's verb and body into the rpc it names.
            pub(crate) fn decode(verb: &str, payload: &[u8]) -> Result<Self, $crate::error::WireError> {
                match verb {
                    $( $verb_str => $crate::endpoint::decode_body(payload).map(Self::$name), )*
                    other => Err($crate::error::WireError::unknown_rpc(other)),
                }
            }
        }

        /// A received rpc of this family: its decoded request and the reply
        /// it owes.
        #[derive(Debug)]
        pub enum Incoming {
            $(
                #[doc = concat!(
                    "[`", stringify!($name), "`]'s decoded request and the responder that owes its reply."
                )]
                $name($request, $crate::serve::Responder<$name>),
            )*
        }

        impl Incoming {
            /// The wildcard this family's queryable declares.
            pub fn wild() -> String {
                $wild()
            }

            /// Receive a query on this family. A key naming no rpc of the
            /// family, or a body that doesn't decode, is refused here and
            /// yields `None`.
            pub async fn receive(query: zenoh::query::Query) -> Option<($target, Self)> {
                let parsed = $parse(query.key_expr().as_str())
                    .map($crate::endpoint::ParsedKey::split)
                    .map(|(target, verb)| (target, verb.to_string()));
                let Some((target, verb)) = parsed else {
                    let error = $crate::error::WireError::unknown_rpc(query.key_expr().as_str());
                    $crate::serve::reply_err(&query, &error).await;
                    return None;
                };
                match Call::decode(&verb, &$crate::serve::payload(&query)) {
                    Ok(call) => Some((target, Self::with_responder(call, query))),
                    Err(error) => {
                        $crate::serve::reply_err(&query, &error).await;
                        None
                    }
                }
            }

            fn with_responder(call: Call, query: zenoh::query::Query) -> Self {
                match call {
                    $( Call::$name(request) => {
                        Self::$name(request, $crate::serve::Responder::new(query))
                    } )*
                }
            }
        }
    };
    (@kind $name:ident read) => {
        impl $crate::endpoint::Unary for $name {
            const REISSUE: bool = true;
        }
    };
    (@kind $name:ident write) => {
        impl $crate::endpoint::Unary for $name {
            const REISSUE: bool = false;
        }
    };
}

/// The daemon's own rpcs: `ctrlassist/rpc/daemon/<verb>`.
pub mod daemon {
    use super::{keys, rpc};

    family! {
        /// A decoded daemon rpc.
        target: (),
        key: |_target, verb| keys::daemon_rpc(verb),
        parse: keys::parse_daemon_rpc,
        wild: keys::daemon_rpc_wild,
        rpcs: {
            /// The daemon's release.
            Info = "info" (()) -> rpc::ServiceInfo, read;
        }
    }
}

/// The profile rpcs: `ctrlassist/rpc/profile/<verb>`.
pub mod profile {
    use super::{keys, rpc};

    family! {
        /// A decoded profile rpc.
        target: (),
        key: |_target, verb| keys::profile_rpc(verb),
        parse: keys::parse_profile_rpc,
        wild: keys::profile_rpc_wild,
        rpcs: {
            /// Check a profile and say where it is wrong. Nothing is kept.
            Validate = "validate" (rpc::ValidateRequest) -> rpc::Validation, read;
            /// Keep a profile, durably, once it is found to be right.
            Put = "put" (rpc::PutProfileRequest) -> rpc::Ack, write;
            /// Stop keeping a profile.
            Delete = "delete" (rpc::ProfileRequest) -> rpc::Ack, write;
        }
    }
}

/// The setup rpcs: `ctrlassist/rpc/setup/<verb>`.
pub mod setup {
    use super::{keys, rpc};

    family! {
        /// A decoded setup rpc.
        target: (),
        key: |_target, verb| keys::setup_rpc(verb),
        parse: keys::parse_setup_rpc,
        wild: keys::setup_rpc_wild,
        rpcs: {
            /// Run a kept profile. Replies once its virtual pads exist
            /// and it is running or held, or with why it couldn't start.
            Start = "start" (rpc::SetupRequest) -> rpc::Ack, write;
            /// Stop a setup. Replies once its virtual pads are gone.
            Stop = "stop" (rpc::SetupRequest) -> rpc::Ack, write;
            /// Have a node select another input or output. Replies once
            /// the input thread has been asked; the setup's state says
            /// when it has.
            Select = "select" (rpc::SelectRequest) -> rpc::Ack, write;
        }
    }
}

/// The pad rpcs: `ctrlassist/rpc/pad/<verb>`.
pub mod pad {
    use super::{keys, rpc};

    family! {
        /// A decoded pad rpc.
        target: (),
        key: |_target, verb| keys::pad_rpc(verb),
        parse: keys::parse_pad_rpc,
        wild: keys::pad_rpc_wild,
        rpcs: {
            /// Buzz a pad, so a person can tell which one it is. Replies
            /// once the buzz has started.
            Identify = "identify" (rpc::IdentifyRequest) -> rpc::Ack, write;
            /// Drop a remembered pad that isn't connected.
            Forget = "forget" (rpc::PadRequest) -> rpc::Ack, write;
        }
    }
}
