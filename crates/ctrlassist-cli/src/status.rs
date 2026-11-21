//! `status`: the daemon's liveliness and release.

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use ctrlassist_protocol::caller::Caller;
use ctrlassist_protocol::endpoint::daemon;
use ctrlassist_protocol::{VERSION, bootstrap, compatible, keys};
use serde_json::json;

/// How long zenoh waits for the liveliness answer before the daemon is
/// taken as absent. The link is a local socket, so an answer is there in
/// well under this or not coming.
const ALIVE_WINDOW: Duration = Duration::from_secs(2);

/// What `status` found.
struct Found {
    alive: bool,
    release: Option<String>,
}

/// Asks the daemon on `socket`. A socket nothing listens on is a daemon
/// that isn't running, not an error.
async fn ask(socket: &Path) -> Result<Found> {
    let Ok(session) = zenoh::open(bootstrap::client_config(socket)?).await else {
        return Ok(Found {
            alive: false,
            release: None,
        });
    };
    let replies = session
        .liveliness()
        .get(keys::daemon_alive())
        .timeout(ALIVE_WINDOW)
        .await
        .map_err(|error| anyhow!("asking for the daemon's liveliness: {error}"))?;
    let alive = matches!(replies.recv_async().await, Ok(reply) if reply.result().is_ok());
    let release = if alive {
        Caller::new(session.clone())
            .call::<daemon::Info>(&(), &())
            .await
            .ok()
            .map(|info| info.version)
    } else {
        None
    };
    let _ = session.close().await;
    Ok(Found { alive, release })
}

pub(crate) async fn status(socket: &Path, json_out: bool) -> Result<()> {
    let Found { alive, release } = ask(socket).await?;
    let is_compatible = release
        .as_deref()
        .map(|release| compatible(VERSION, release));
    let healthy = alive && is_compatible != Some(false);
    if json_out {
        writeln!(
            io::stdout(),
            "{}",
            json!({
                "alive": alive,
                "socket": socket,
                "release": release,
                "compatible": is_compatible,
            })
        )?;
        if !healthy {
            #[expect(
                clippy::exit,
                reason = "`status --json` reports a missing or incompatible daemon as exit code 1 after printing its JSON, with no error text"
            )]
            std::process::exit(1);
        }
        return Ok(());
    }
    if !alive {
        bail!("no daemon is running on {}", socket.display());
    }
    writeln!(io::stdout(), "daemon alive at {}", socket.display())?;
    if let Some(release) = &release {
        writeln!(
            io::stdout(),
            "daemon release: {release} (this command: {VERSION})"
        )?;
    }
    if is_compatible == Some(false) {
        bail!(
            "the daemon runs release {}, which this command ({VERSION}) can't talk to; restart it or upgrade so they match",
            release.as_deref().unwrap_or_default()
        );
    }
    Ok(())
}
