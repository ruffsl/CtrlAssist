//! Key-expression builders. One function per key family so the literal
//! shape exists in exactly one place.

/// Liveliness token declared by a running daemon.
pub fn daemon_alive() -> String {
    "ctrlassist/alive/daemon".to_string()
}

/// Liveliness token declared by a running tray's settings service, on
/// the client-mode link it serves on, which the daemon relays to its
/// other clients.
pub fn tray_alive() -> String {
    "ctrlassist/alive/tray".to_string()
}

/// The tray's notification switches, a
/// [`NotificationSettings`](crate::rpc::NotificationSettings): a cached
/// publish, by the tray.
pub fn tray_settings() -> String {
    "ctrlassist/tray/settings".to_string()
}

/// A tray rpc's key.
pub(crate) fn tray_rpc(verb: &str) -> String {
    format!("ctrlassist/rpc/tray/{verb}")
}

/// Wildcard the tray's rpc queryable is declared on.
pub fn tray_rpc_wild() -> String {
    "ctrlassist/rpc/tray/*".to_string()
}

/// Parse a [`tray_rpc`] key into its verb.
pub(crate) fn parse_tray_rpc(key: &str) -> Option<&str> {
    key.strip_prefix("ctrlassist/rpc/tray/")
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

/// A pad's state, a [`Pad`](crate::payload::Pad): a cached publish.
/// `id` is [`pad_id`] of the pad's name.
pub fn pad(id: &str) -> String {
    format!("ctrlassist/pad/{id}")
}

/// Wildcard over every pad's state.
pub fn pad_wild() -> String {
    "ctrlassist/pad/*".to_string()
}

/// Parse a [`pad`] key into its id.
pub fn parse_pad(key: &str) -> Option<&str> {
    segment(key.strip_prefix("ctrlassist/pad/")?)
}

/// The key segment for the pad known as `name`. A pad's name is the
/// pad's own claim, free text that may hold anything, so it never goes
/// into a key: this is 64 bits of it, hashed (FNV-1a) and written as 16
/// hex digits. The same name always gives the same segment, so the
/// daemon and a client agree on it without asking.
pub fn pad_id(name: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in name.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// An identity's state, an [`Identity`](crate::payload::Identity): a
/// cached publish. `id` is the identity's name as a profile writes it.
pub fn identity(id: &str) -> String {
    format!("ctrlassist/identity/{id}")
}

/// Wildcard over every identity's state.
pub fn identity_wild() -> String {
    "ctrlassist/identity/*".to_string()
}

/// Parse an [`identity`] key into its id.
pub fn parse_identity(key: &str) -> Option<&str> {
    segment(key.strip_prefix("ctrlassist/identity/")?)
}

/// A pad rpc's key.
pub(crate) fn pad_rpc(verb: &str) -> String {
    format!("ctrlassist/rpc/pad/{verb}")
}

/// Wildcard the pad rpcs' queryable is declared on.
pub fn pad_rpc_wild() -> String {
    "ctrlassist/rpc/pad/*".to_string()
}

/// Parse a [`pad_rpc`] key into its verb.
pub(crate) fn parse_pad_rpc(key: &str) -> Option<&str> {
    key.strip_prefix("ctrlassist/rpc/pad/")
}

/// A profile's state, a [`Profile`](crate::payload::Profile): a cached
/// publish. `name` is the profile's name, which is key-safe by rule.
pub fn profile(name: &str) -> String {
    format!("ctrlassist/profile/{name}")
}

/// Wildcard over every profile's state.
pub fn profile_wild() -> String {
    "ctrlassist/profile/*".to_string()
}

/// Parse a [`profile`] key into the profile's name.
pub fn parse_profile(key: &str) -> Option<&str> {
    segment(key.strip_prefix("ctrlassist/profile/")?)
}

/// A profile rpc's key.
pub(crate) fn profile_rpc(verb: &str) -> String {
    format!("ctrlassist/rpc/profile/{verb}")
}

/// Wildcard the profile rpcs' queryable is declared on.
pub fn profile_rpc_wild() -> String {
    "ctrlassist/rpc/profile/*".to_string()
}

/// Parse a [`profile_rpc`] key into its verb.
pub(crate) fn parse_profile_rpc(key: &str) -> Option<&str> {
    key.strip_prefix("ctrlassist/rpc/profile/")
}

/// A setup's state, a [`Setup`](crate::payload::Setup): a cached publish
/// for as long as the setup exists. `name` is the profile it runs.
pub fn setup(name: &str) -> String {
    format!("ctrlassist/setup/{name}")
}

/// Wildcard over every setup's state.
pub fn setup_wild() -> String {
    "ctrlassist/setup/*".to_string()
}

/// Parse a [`setup`] key into the setup's name.
pub fn parse_setup(key: &str) -> Option<&str> {
    segment(key.strip_prefix("ctrlassist/setup/")?)
}

/// A setup rpc's key.
pub(crate) fn setup_rpc(verb: &str) -> String {
    format!("ctrlassist/rpc/setup/{verb}")
}

/// Wildcard the setup rpcs' queryable is declared on.
pub fn setup_rpc_wild() -> String {
    "ctrlassist/rpc/setup/*".to_string()
}

/// Parse a [`setup_rpc`] key into its verb.
pub(crate) fn parse_setup_rpc(key: &str) -> Option<&str> {
    key.strip_prefix("ctrlassist/rpc/setup/")
}

/// `rest` if it is exactly one key segment.
fn segment(rest: &str) -> Option<&str> {
    (!rest.is_empty() && !rest.contains('/')).then_some(rest)
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

        let key = tray_rpc("update");
        assert_eq!(key, "ctrlassist/rpc/tray/update");
        assert_eq!(parse_tray_rpc(&key), Some("update"));
        assert_eq!(parse_tray_rpc(&daemon_rpc("info")), None);
        assert_eq!(parse_daemon_rpc(&key), None);
        assert!(tray_rpc_wild().starts_with("ctrlassist/rpc/tray/"));
        assert_ne!(tray_alive(), tray_settings());

        let key = pad_rpc("identify");
        assert_eq!(parse_pad_rpc(&key), Some("identify"));
        assert_eq!(parse_pad_rpc(&daemon_rpc("info")), None);
        assert!(pad_rpc_wild().starts_with("ctrlassist/rpc/pad/"));

        let key = setup_rpc("start");
        assert_eq!(parse_setup_rpc(&key), Some("start"));
        assert_eq!(parse_setup_rpc(&profile_rpc("put")), None);
        assert!(setup_rpc_wild().starts_with("ctrlassist/rpc/setup/"));

        let key = profile_rpc("put");
        assert_eq!(parse_profile_rpc(&key), Some("put"));
        assert_eq!(parse_profile_rpc(&pad_rpc("forget")), None);
        assert!(profile_rpc_wild().starts_with("ctrlassist/rpc/profile/"));
    }

    #[test]
    fn a_state_key_parses_back_to_its_id() {
        let id = pad_id("address:0c:27:56:4f:bd:14");
        assert_eq!(parse_pad(&pad(&id)), Some(id.as_str()));
        assert_eq!(parse_pad("ctrlassist/pad/"), None);
        assert_eq!(parse_pad("ctrlassist/pad/a/b"), None);
        assert_eq!(parse_pad(&identity("xbox")), None);
        assert_eq!(parse_identity(&identity("xbox")), Some("xbox"));
        assert_eq!(parse_identity(&pad(&id)), None);
        assert!(pad(&id).starts_with(pad_wild().trim_end_matches('*')));
        assert_eq!(parse_setup(&setup("couch")), Some("couch"));
        assert_eq!(parse_setup(&profile("couch")), None);
        assert!(setup("couch").starts_with(setup_wild().trim_end_matches('*')));
        assert_eq!(parse_profile(&profile("couch")), Some("couch"));
        assert_eq!(parse_profile(&identity("xbox")), None);
        assert!(profile("couch").starts_with(profile_wild().trim_end_matches('*')));
        assert!(identity("xbox").starts_with(identity_wild().trim_end_matches('*')));
    }

    /// Whatever a pad calls itself, its key segment is 16 hex digits, the
    /// same each time, and different for a different name.
    #[test]
    fn a_pad_s_key_segment_is_safe_whatever_its_name() {
        let hostile = pad_id("model:1234:5678:a/b**/$*\u{0} pad");
        assert_eq!(hostile.len(), 16);
        assert!(hostile.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(hostile, pad_id("model:1234:5678:a/b**/$*\u{0} pad"));
        assert_ne!(pad_id("address:aa"), pad_id("address:ab"));
        // The published FNV-1a test vectors.
        assert_eq!(pad_id(""), "cbf29ce484222325");
        assert_eq!(pad_id("a"), "af63dc4c8601ec8c");
    }
}
