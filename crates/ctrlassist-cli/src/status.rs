//! `status`: the daemon's liveliness and release.

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use ctrlassist_protocol::caller::Caller;
use ctrlassist_protocol::endpoint::daemon;
use ctrlassist_protocol::{VERSION, bootstrap, compatible, keys};
use serde_json::json;
use zenoh::Session;

/// How long zenoh waits for the liveliness answer before the daemon is
/// taken as absent. The daemon, as the router, answers at once with the
/// tokens it holds and then says it has no more, which ends the wait;
/// this bounds only a link that stalls.
const ALIVE_WINDOW: Duration = Duration::from_secs(2);

/// Whether the daemon's liveliness token is present, as the daemon on
/// the other end of `session` says. A daemon that is stopping has
/// withdrawn its token while it may still answer rpcs. Asked on a session
/// in client mode with no liveliness subscription of its own: with one,
/// the daemon leaves out of the answer a token it has already sent.
async fn present(session: &Session) -> Result<bool> {
    let replies = session
        .liveliness()
        .get(keys::daemon_alive())
        .timeout(ALIVE_WINDOW)
        .await
        .map_err(|error| anyhow!("asking for the daemon's liveliness: {error}"))?;
    Ok(matches!(replies.recv_async().await, Ok(reply) if reply.result().is_ok()))
}

/// What `status` found.
struct Found {
    alive: bool,
    release: Option<String>,
    /// Why what was found is short of a running daemon's release, when
    /// something failed rather than nothing being there.
    error: Option<String>,
}

/// Asks the daemon on `socket`. A socket nothing listens on is a daemon
/// that isn't running, not an error.
async fn ask(socket: &Path) -> Found {
    let failed = |error: anyhow::Error| Found {
        alive: false,
        release: None,
        error: Some(format!("{error:#}")),
    };
    let config = match bootstrap::client_config(socket) {
        Ok(config) => config,
        Err(error) => return failed(error.into()),
    };
    let Ok(session) = zenoh::open(config).await else {
        return Found {
            alive: false,
            release: None,
            error: None,
        };
    };
    let found = match present(&session).await {
        Err(error) => failed(error),
        Ok(false) => Found {
            alive: false,
            release: None,
            error: None,
        },
        Ok(true) => match Caller::new(session.clone())
            .call::<daemon::Info>(&(), &())
            .await
        {
            Ok(info) => Found {
                alive: true,
                release: Some(info.version),
                error: None,
            },
            Err(error) => Found {
                alive: true,
                release: None,
                error: Some(format!("the daemon didn't say its release: {error}")),
            },
        },
    };
    let _ = session.close().await;
    found
}

pub(crate) async fn status(socket: &Path, json_out: bool) -> Result<()> {
    let Found {
        alive,
        release,
        error,
    } = ask(socket).await;
    let is_compatible = release
        .as_deref()
        .map(|release| compatible(VERSION, release));
    let healthy = alive && is_compatible == Some(true);
    if json_out {
        writeln!(
            io::stdout(),
            "{}",
            json!({
                "alive": alive,
                "socket": socket,
                "release": release,
                "compatible": is_compatible,
                "error": error,
            })
        )?;
        if !healthy {
            #[expect(
                clippy::exit,
                reason = "`status --json` reports a missing, silent or incompatible daemon as exit code 1 after printing its JSON, with no error text"
            )]
            std::process::exit(1);
        }
        return Ok(());
    }
    if !alive {
        match error {
            Some(error) => bail!("{error}"),
            None => bail!("no daemon is running on {}", socket.display()),
        }
    }
    writeln!(io::stdout(), "daemon alive at {}", socket.display())?;
    let Some(release) = &release else {
        bail!(
            "{}",
            error.unwrap_or_else(|| "the daemon didn't say its release".to_string())
        );
    };
    writeln!(
        io::stdout(),
        "daemon release: {release} (this command: {VERSION})"
    )?;
    if is_compatible == Some(false) {
        bail!(
            "the daemon runs release {release}, which this command ({VERSION}) can't talk to; restart it or upgrade so they match"
        );
    }
    Ok(())
}
