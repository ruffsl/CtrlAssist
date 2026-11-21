//! `pads`, `identities`, `identify` and `forget`: what the daemon knows
//! of pads, read from its published state, and the two things to ask of
//! one.

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ctrlassist::{DaemonClient, Error, StateEvent, StateWatcher, StateWatcherConfig};
pub(crate) use ctrlassist_cli::printable;

/// Connects to the daemon on `socket` and checks it can be talked to.
pub(crate) async fn connect(socket: &Path) -> Result<(DaemonClient, StateWatcher)> {
    // A session that doesn't keep dialing: a command that runs once says
    // at once when nothing is listening.
    let config = ctrlassist_protocol::bootstrap::client_config(socket)?;
    let Ok(session) = zenoh::open(config).await else {
        bail!("no daemon is running on {}", socket.display());
    };
    let client = DaemonClient::with_session(session)
        .await
        .context("following the daemon's state")?;
    // The watcher before the first call, so the daemon going away after
    // that call can't be missed.
    let mut config = StateWatcherConfig::default();
    config.fallback_poll_interval = Duration::ZERO;
    let watcher = StateWatcher::new(&client, config);
    match client.check_release().await {
        Ok(()) => Ok((client, watcher)),
        Err(Error::Unavailable) => bail!("no daemon is running on {}", socket.display()),
        Err(error) => Err(error.into()),
    }
}

/// As [`connect`], then waits until the daemon's published state has
/// arrived, so that what is read next is all of it.
pub(crate) async fn hydrated(socket: &Path) -> Result<DaemonClient> {
    let (client, mut watcher) = connect(socket).await?;
    while !client.is_hydrated() {
        match watcher.next().await {
            Some(StateEvent::DaemonDisappeared) | None => {
                bail!("the daemon on {} went away", socket.display())
            }
            Some(_) => {}
        }
    }
    Ok(client)
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// Writes rows as columns, each as wide as its widest cell.
pub(crate) fn table<const N: usize>(headings: [&str; N], rows: &[[String; N]]) -> io::Result<()> {
    let mut widths = headings.map(|heading| heading.chars().count());
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut out = io::stdout().lock();
    let mut line = |cells: [&str; N]| {
        let mut text = String::new();
        for (index, (cell, width)) in cells.iter().zip(widths).enumerate() {
            if index + 1 == N {
                text.push_str(cell);
            } else {
                text.push_str(&format!("{cell:<width$}  "));
            }
        }
        writeln!(out, "{}", text.trim_end())
    };
    line(headings)?;
    for row in rows {
        line(std::array::from_fn(|index| row[index].as_str()))?;
    }
    Ok(())
}

pub(crate) async fn pads(socket: &Path, json: bool) -> Result<()> {
    let pads = hydrated(socket).await?.list_pads()?;
    if json {
        writeln!(io::stdout(), "{}", serde_json::to_string(&pads)?)?;
        return Ok(());
    }
    if pads.is_empty() {
        writeln!(io::stdout(), "no pads are known")?;
        return Ok(());
    }
    let rows: Vec<[String; 5]> = pads
        .iter()
        .map(|pad| {
            [
                printable(&pad.id),
                printable(&pad.name),
                format!("{:04x}:{:04x}", pad.vendor, pad.product),
                yes_no(pad.connected).to_string(),
                pad.rumble.map_or("unknown", yes_no).to_string(),
            ]
        })
        .collect();
    table(["ID", "NAME", "MODEL", "CONNECTED", "RUMBLE"], &rows)?;
    Ok(())
}

pub(crate) async fn identities(socket: &Path, json: bool) -> Result<()> {
    let identities = hydrated(socket).await?.list_identities()?;
    if json {
        writeln!(io::stdout(), "{}", serde_json::to_string(&identities)?)?;
        return Ok(());
    }
    let rows: Vec<[String; 4]> = identities
        .iter()
        .map(|identity| {
            [
                identity.id.clone(),
                identity.name.clone(),
                format!("{:04x}:{:04x}", identity.vendor, identity.product),
                yes_no(identity.available).to_string(),
            ]
        })
        .collect();
    table(["ID", "NAME", "MODEL", "AVAILABLE"], &rows)?;
    Ok(())
}

pub(crate) async fn identify(socket: &Path, pad: &str, millis: u32) -> Result<()> {
    let (client, _watcher) = connect(socket).await?;
    client
        .identify_pad(pad, Duration::from_millis(u64::from(millis)))
        .await?;
    Ok(())
}

pub(crate) async fn forget(socket: &Path, pad: &str) -> Result<()> {
    let (client, _watcher) = connect(socket).await?;
    client.forget_pad(pad).await?;
    Ok(())
}
