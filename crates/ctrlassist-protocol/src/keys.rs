//! Key-expression builders. One function per key family so the literal
//! shape exists in exactly one place.

/// Liveliness token declared by a running daemon.
pub fn daemon_alive() -> String {
    "ctrlassist/alive/daemon".to_string()
}

/// The daemon's own state, a [`Daemon`](crate::payload::Daemon): a cached
/// publish.
pub fn daemon() -> String {
    "ctrlassist/daemon".to_string()
}

/// A daemon rpc's key.
pub(crate) fn daemon_rpc(verb: &str) -> String {
    format!("ctrlassist/rpc/daemon/{verb}")
}

/// Wildcard the daemon's own rpc queryable is declared on.
pub fn daemon_rpc_wild() -> String {
    "ctrlassist/rpc/daemon/*".to_string()
}

/// Parse a [`daemon_rpc`] key into its verb (the rest of `key` after the
/// family's prefix); `None` when `key` lacks that prefix.
pub(crate) fn parse_daemon_rpc(key: &str) -> Option<&str> {
    key.strip_prefix("ctrlassist/rpc/daemon/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_rpc_key_parses_back_to_its_verb_and_matches_its_wildcard() {
        let key = daemon_rpc("info");
        assert_eq!(key, "ctrlassist/rpc/daemon/info");
        assert_eq!(parse_daemon_rpc(&key), Some("info"));
        assert_eq!(parse_daemon_rpc("ctrlassist/rpc/setup/start"), None);
        assert!(daemon_rpc_wild().starts_with("ctrlassist/rpc/daemon/"));
        assert_ne!(daemon_alive(), daemon());
    }
}
