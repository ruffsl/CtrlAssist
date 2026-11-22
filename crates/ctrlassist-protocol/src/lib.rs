//! What CtrlAssist's daemon and its clients agree on: the profile, and
//! the control plane's keys, payloads and rpcs.
//!
//! The control plane's patterns are TisVlay's: rpcs declared once in a
//! table, matching-gated calls, typed wire errors, and a `major.minor`
//! check between client and daemon.

pub mod bootstrap;
pub mod caller;
pub mod endpoint;
pub mod error;
pub mod form;
pub mod keys;
pub mod payload;
pub mod profile;
pub mod rpc;
pub mod serve;

/// This crate's release: the protocol the daemon and every client built
/// from it speak. The daemon reports it in `rpc::ServiceInfo::version`;
/// clients compare it with [`compatible`].
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Whether a peer at release `theirs` speaks the protocol of one at
/// `ours`: the same `major.minor`. Releases change the protocol only in a
/// minor bump, so patch releases stay compatible. A pre-release (say
/// `0.5.0-rc.1`) may still change before its final release, so it's
/// compatible only with exactly itself. Build metadata (`+…`) is ignored,
/// and a release that isn't valid semver is incompatible.
pub fn compatible(ours: &str, theirs: &str) -> bool {
    let (Some(ours), Some(theirs)) = (Release::parse(ours), Release::parse(theirs)) else {
        return false;
    };
    if ours.pre.is_some() || theirs.pre.is_some() {
        return ours == theirs;
    }
    (ours.major, ours.minor) == (theirs.major, theirs.minor)
}

/// A semver release, build metadata dropped.
#[derive(PartialEq, Eq)]
struct Release<'a> {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Option<&'a str>,
}

impl<'a> Release<'a> {
    /// Parse `major.minor.patch[-pre][+build]` strictly: numbers without
    /// leading zeros, and dot-separated identifiers of ASCII
    /// alphanumerics and hyphens, a numeric pre-release one again
    /// without leading zeros.
    fn parse(release: &'a str) -> Option<Self> {
        let (release, build) = match release.split_once('+') {
            Some((release, build)) => (release, Some(build)),
            None => (release, None),
        };
        if build.is_some_and(|build| !identifiers(build, false)) {
            return None;
        }
        let (core, pre) = match release.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (release, None),
        };
        if pre.is_some_and(|pre| !identifiers(pre, true)) {
            return None;
        }
        let mut numbers = core.split('.').map(number);
        let release = Self {
            major: numbers.next()??,
            minor: numbers.next()??,
            patch: numbers.next()??,
            pre,
        };
        numbers.next().is_none().then_some(release)
    }
}

/// A numeric version part: digits, no leading zero.
fn number(part: &str) -> Option<u64> {
    let digits = !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let leading_zero = part.len() > 1 && part.starts_with('0');
    if digits && !leading_zero {
        part.parse().ok()
    } else {
        None
    }
}

/// Dot-separated, non-empty identifiers of ASCII alphanumerics and
/// hyphens; for a pre-release, numeric ones carry no leading zero.
fn identifiers(list: &str, pre_release: bool) -> bool {
    list.split('.').all(|id| {
        let charset = !id.is_empty() && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        let numeric = id.bytes().all(|b| b.is_ascii_digit());
        charset && !(pre_release && numeric && number(id).is_none())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_release_line_is_compatible() {
        assert!(compatible("0.4.0", "0.4.7"));
        assert!(compatible("0.4.0", "0.4.7+build.5"));
        assert!(!compatible("0.4.0", "0.5.0"));
        assert!(!compatible("0.4.0", "1.4.0"));
        assert!(compatible(VERSION, VERSION));
    }

    /// A pre-release may still change before its final release, so it
    /// talks only to exactly itself (build metadata aside).
    #[test]
    fn a_pre_release_is_compatible_only_with_itself() {
        assert!(compatible("0.5.0-rc.1", "0.5.0-rc.1"));
        assert!(compatible("0.5.0-rc.1", "0.5.0-rc.1+abc"));
        assert!(!compatible("0.5.0-rc.1", "0.5.0"));
        assert!(!compatible("0.5.3", "0.5.0-rc.1"));
        assert!(!compatible("0.5.0-rc.1", "0.5.0-rc.2"));
    }

    /// Anything that isn't valid semver is incompatible, never guessed at.
    #[test]
    fn malformed_releases_are_incompatible() {
        for bad in [
            "",
            "garbage",
            "0.4",
            "0.4.0.1",
            "0-4",
            "1+2",
            "00.4.0",
            "0.04.0",
            "0.4.0-",
            "0.4.0-rc..1",
            "0.4.0-rc.01",
            "0.4.0+",
            "0.4.0+bu ild",
            "v0.4.0",
        ] {
            assert!(!compatible("0.4.0", bad), "{bad:?} should be incompatible");
            assert!(!compatible(bad, "0.4.0"), "{bad:?} should be incompatible");
        }
    }

    /// The handshake surface is frozen: a `ServiceInfo` with fields a
    /// later release added still decodes.
    #[test]
    fn service_info_decodes_with_fields_it_doesnt_know() {
        let info: rpc::ServiceInfo =
            serde_json::from_str(r#"{"version":"0.9.0","added_later":true}"#).unwrap();
        assert_eq!(info.version, "0.9.0");
    }
}
