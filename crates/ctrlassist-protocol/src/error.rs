//! The rpc error contract: every `reply_err` payload is this shape,
//! clients branch on `code` and show `message`.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Declare the rpc-reply error codes: one line per code generates the
/// enum (serialized as the kebab-case string), `as_str`, `ALL` and the
/// reading of a code off the wire.
macro_rules! error_codes {
        ($( $(#[$meta:meta])* $name:ident = $code:literal, )*) => {
            /// Why the daemon refused an rpc: the `code` of every
            /// [`WireError`]. Serialized as its kebab-case string.
            ///
            /// A code this release doesn't know, which a later release
            /// of the daemon may add, reads as [`ErrorCode::Unknown`]
            /// with the code as it came, so the refusal and its message
            /// still reach the caller.
            #[non_exhaustive]
            #[derive(Debug, Clone, PartialEq, Eq, Hash)]
            pub enum ErrorCode {
                $( $(#[$meta])* $name, )*
                /// A code this release doesn't know, from a daemon of a
                /// later release: refused all the same, for a reason only
                /// the message says.
                Unknown(UnknownCode),
            }

            impl ErrorCode {
                /// Every code this release knows, in declaration order.
                pub const ALL: &[Self] = &[$(Self::$name),*];

                /// The code as it travels on the wire.
                #[must_use]
                pub fn as_str(&self) -> &str {
                    match self {
                        $( Self::$name => $code, )*
                        Self::Unknown(code) => code.as_str(),
                    }
                }

                /// The code `wire` names: one this release knows, or
                /// else [`ErrorCode::Unknown`] holding it.
                fn from_wire(wire: String) -> Self {
                    match wire.as_str() {
                        $( $code => Self::$name, )*
                        _ => Self::Unknown(UnknownCode(wire.into_boxed_str())),
                    }
                }
            }
        };
    }

/// A refusal code this release doesn't know, as it came on the wire.
/// Only reading a reply makes one, so it never holds a code that
/// [`ErrorCode`] names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UnknownCode(Box<str>);

impl UnknownCode {
    /// The code as it came on the wire.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Serialize for ErrorCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ErrorCode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::from_wire)
    }
}

error_codes! {
    /// The request body doesn't decode as the rpc's request type, or a
    /// value in it is refused.
    BadRequest = "bad-request",
    /// The query's key names no rpc of the family it reached.
    UnknownRpc = "unknown-rpc",
    /// No known pad goes by the name given.
    UnknownPad = "unknown-pad",
    /// More than one known pad goes by the name given.
    AmbiguousPad = "ambiguous-pad",
    /// The pad is known but not connected, and the request needs it.
    PadDisconnected = "pad-disconnected",
    /// The pad is connected, and the request is for one that isn't.
    PadConnected = "pad-connected",
    /// The pad has no rumble motors the kernel can drive.
    PadWithoutRumble = "pad-without-rumble",
    /// The pad couldn't be opened, or refused what was asked of it.
    PadRefused = "pad-refused",
    /// The profile is wrong; the message says where and what.
    InvalidProfile = "invalid-profile",
    /// No profile is kept under the name given.
    UnknownProfile = "unknown-profile",
    /// A setup of that name exists already.
    SetupExists = "setup-exists",
    /// No setup goes by the name given.
    UnknownSetup = "unknown-setup",
    /// The setup is between two phases, and the request has to wait for
    /// it to settle.
    SetupBusy = "setup-busy",
    /// The setup couldn't be started; the message says why.
    SetupFailed = "setup-failed",
    /// A pad the request needs is in use by a setup.
    PadInUse = "pad-in-use",
    /// The profile is the reserved default, which is always kept: it
    /// can't be deleted.
    ProfileReserved = "profile-reserved",
    /// A settings file is missing or can't be read as settings; the
    /// message says why.
    SettingsUnreadable = "settings-unreadable",
    /// A settings file couldn't be written, so the change asked for
    /// wasn't made; the message says which file and why.
    SettingsUnwritable = "settings-unwritable",
    /// The daemon stopped before it could answer.
    DaemonShuttingDown = "daemon-shutting-down",
    /// The daemon failed on its side; the request itself may be fine.
    Internal = "internal",
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Typed failure reason for a command reply — serialized into the
/// zenoh `reply_err` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireError {
    /// Why the daemon refused; clients branch on this.
    pub code: ErrorCode,
    /// The refusal for a person to read.
    pub message: String,
}

impl WireError {
    /// The request is malformed or names a refused value; `message`
    /// says which, for a person to read.
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::BadRequest,
            message: message.into(),
        }
    }

    /// No rpc of the family answers to `verb`: a query's verb, or its
    /// whole key when no verb splits out of it.
    pub fn unknown_rpc(verb: &str) -> Self {
        Self {
            code: ErrorCode::UnknownRpc,
            message: format!("no such rpc: {verb}"),
        }
    }

    /// A refusal with a code of its own.
    pub fn refusal(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// The daemon has stopped, so a filed rpc will never be answered.
    pub fn daemon_shutting_down() -> Self {
        Self {
            code: ErrorCode::DaemonShuttingDown,
            message: "the daemon is shutting down".into(),
        }
    }

    /// The daemon failed on its side (a task it needs is gone, a reply
    /// it built is malformed); the request itself may be fine.
    pub fn internal(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Internal,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_travels_as_its_kebab_case_string() {
        for code in ErrorCode::ALL {
            let wire = serde_json::to_string(code).unwrap();
            assert_eq!(wire, format!("\"{}\"", code.as_str()));
            assert_eq!(serde_json::from_str::<ErrorCode>(&wire).unwrap(), *code);
            assert_eq!(code.to_string(), code.as_str());
        }
    }

    /// A refusal with a code a later release added still reads, with its
    /// message and the code as it came, which travels on unchanged.
    #[test]
    fn a_code_this_release_does_not_know_keeps_the_refusal() {
        let wire = r#"{"code":"pad-on-fire","message":"the pad is on fire"}"#;
        let refusal: WireError = serde_json::from_str(wire).unwrap();
        assert!(matches!(refusal.code, ErrorCode::Unknown(_)));
        assert_eq!(refusal.code.as_str(), "pad-on-fire");
        assert_eq!(refusal.code.to_string(), "pad-on-fire");
        assert_eq!(refusal.message, "the pad is on fire");
        assert!(!ErrorCode::ALL.contains(&refusal.code));
        assert_eq!(serde_json::to_string(&refusal).unwrap(), wire);
        // A code is a string, and nothing else reads as one.
        assert!(serde_json::from_str::<ErrorCode>("7").is_err());
    }

    #[test]
    fn each_refusal_carries_its_code() {
        assert_eq!(WireError::bad_request("x").code, ErrorCode::BadRequest);
        assert_eq!(WireError::unknown_rpc("x").message, "no such rpc: x");
        assert_eq!(
            WireError::daemon_shutting_down().code,
            ErrorCode::DaemonShuttingDown
        );
        assert_eq!(WireError::internal("x").code, ErrorCode::Internal);
        let refusal = WireError::refusal(ErrorCode::UnknownPad, "no pad is called x");
        assert_eq!(refusal.code.as_str(), "unknown-pad");
        assert_eq!(refusal.message, "no pad is called x");
    }
}
