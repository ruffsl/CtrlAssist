//! Session configuration for the daemon (a listener on a Unix socket)
//! and its clients (connect-only). Multicast and gossip scouting are
//! pinned off in every config: the topology is explicit endpoints only,
//! never LAN discovery.

use std::fmt;
use std::path::{Path, PathBuf};

use zenoh::Config;

/// A session config that couldn't be built.
#[derive(Debug)]
#[non_exhaustive]
pub enum ConfigError {
    /// The socket path isn't valid UTF-8, which a zenoh endpoint needs.
    NonUtf8Path(PathBuf),
    /// zenoh refused a config value.
    Rejected {
        /// The config key.
        key: String,
        /// The value it refused.
        value: String,
        /// zenoh's reason.
        source: zenoh::Error,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NonUtf8Path(path) => write!(
                f,
                "socket path {} isn't valid UTF-8, which a zenoh endpoint needs",
                path.display()
            ),
            Self::Rejected { key, value, .. } => write!(f, "zenoh config {key} = {value}"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NonUtf8Path(_) => None,
            Self::Rejected { source, .. } => Some(source.as_ref()),
        }
    }
}

type Result<T> = std::result::Result<T, ConfigError>;

/// The daemon's socket: `$XDG_RUNTIME_DIR/ctrlassist/zenoh.sock`, falling
/// back to the system temp dir when no runtime dir is available. The
/// runtime dir is the user's alone, which is what keeps other users out.
pub fn default_socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("ctrlassist")
        .join("zenoh.sock")
}

/// Router-mode config listening on `socket` — the daemon side. A
/// router relays what a client on a client-mode link serves to the
/// daemon's other clients, which is how the window reaches the tray.
/// Timestamping is on: the advanced-publisher caches order and replay
/// samples by timestamp.
pub fn router_config(socket: &Path) -> Result<Config> {
    let mut config = base_config()?;
    insert(&mut config, "mode", "\"router\"")?;
    insert(&mut config, "listen/endpoints", &endpoint_list(&[socket])?)?;
    insert(&mut config, "timestamping/enabled", "true")?;
    Ok(config)
}

/// Client-mode config connecting to `socket` — one-shot callers
/// (CLI, tests). Client `connect.timeout_ms` defaults to 0 (no
/// retry), so an absent daemon fails fast at `zenoh::open`.
///
pub fn client_config(socket: &Path) -> Result<Config> {
    let mut config = base_config()?;
    insert(&mut config, "mode", "\"client\"")?;
    insert(&mut config, "connect/endpoints", &endpoint_list(&[socket])?)?;
    Ok(config)
}

/// Config for long-running clients (the tray): `zenoh::open` returns
/// immediately and the link dials in the background forever, so no start
/// or restart order is assumed — liveliness tokens are the availability
/// signal.
///
/// Shape: a connect-only *peer* (client mode requires a live link at
/// open; peer mode backgrounds the dial).
pub fn client_config_resilient(socket: &Path) -> Result<Config> {
    let mut config = base_config()?;
    insert(&mut config, "mode", "\"peer\"")?;
    insert(&mut config, "listen/endpoints", "[]")?;
    insert(&mut config, "connect/endpoints", &endpoint_list(&[socket])?)?;
    insert(&mut config, "connect/exit_on_failure", "false")?;
    // Retry forever; don't gate open() on the first connect landing.
    insert(&mut config, "connect/timeout_ms", "-1")?;
    insert(
        &mut config,
        "open/return_conditions/connect_scouted",
        "false",
    )?;
    Ok(config)
}

fn base_config() -> Result<Config> {
    let mut config = Config::default();
    insert(&mut config, "scouting/multicast/enabled", "false")?;
    insert(&mut config, "scouting/gossip/enabled", "false")?;
    Ok(config)
}

fn insert(config: &mut Config, key: &str, value: &str) -> Result<()> {
    config
        .insert_json5(key, value)
        .map_err(|source| ConfigError::Rejected {
            key: key.to_owned(),
            value: value.to_owned(),
            source,
        })
}

/// JSON-encoded single-element endpoint list for `insert_json5`.
fn endpoint_list(sockets: &[&Path]) -> Result<String> {
    let endpoints = sockets
        .iter()
        .map(|socket| {
            let path = socket
                .to_str()
                .ok_or_else(|| ConfigError::NonUtf8Path(socket.to_path_buf()))?;
            Ok(format!("unixsock-stream/{path}"))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(serde_json::to_string(&endpoints)
        .expect("a list of strings serializes, because JSON can encode any string"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_list_embeds_absolute_path() {
        let list = endpoint_list(&[Path::new("/run/user/1000/ctrlassist/zenoh.sock")]).unwrap();
        assert_eq!(
            list,
            r#"["unixsock-stream//run/user/1000/ctrlassist/zenoh.sock"]"#
        );
    }

    #[test]
    fn configs_build() {
        let socket = Path::new("/tmp/ctrlassist-test.sock");
        router_config(socket).unwrap();
        client_config(socket).unwrap();
        client_config_resilient(socket).unwrap();
    }

    #[test]
    fn a_non_utf8_socket_path_is_a_typed_error() {
        use std::os::unix::ffi::OsStrExt;
        let socket = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff.sock"));
        assert!(matches!(
            client_config(socket),
            Err(ConfigError::NonUtf8Path(path)) if path == socket
        ));
    }
}
