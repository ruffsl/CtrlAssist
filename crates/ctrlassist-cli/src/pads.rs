//! `pads`, `identities`, `identify` and `forget`: what the daemon knows
//! of pads, read from its published state, and the two things to ask of
//! one.

use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use ctrlassist::{DaemonClient, Error, StateEvent, StateWatcher, StateWatcherConfig};
pub(crate) use ctrlassist_cli::printable;

/// Connects to the daemon on `socket`, waits until the client has taken
/// it up, and checks it can be talked to.
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
    // The watcher before anything is asked, so the daemon going away
    // after that can't be missed.
    let mut config = StateWatcherConfig::default();
    config.fallback_poll_interval = Duration::ZERO;
    let mut watcher = StateWatcher::new(&client, None, config);
    match client.check_release().await {
        Ok(()) => {}
        Err(Error::Unavailable { .. }) => bail!("no daemon is running on {}", socket.display()),
        Err(Error::Stopping { .. }) => bail!("the daemon on {} is stopping", socket.display()),
        Err(error) => return Err(error.into()),
    }
    // Answered, on a session in client mode, after the daemon's token
    // reached the client if it held one: a daemon declares it before it
    // answers anything, and withdraws it only as it goes, so one that
    // answered without it is going away, and is no daemon to a command.
    if !client.token_present() {
        bail!("no daemon is running on {}", socket.display());
    }
    // It has, and the client takes it up, or sees it going.
    while !client.is_available() {
        match watcher.next().await {
            Some(StateEvent::DaemonDisappeared) | None => {
                bail!("the daemon on {} went away", socket.display())
            }
            Some(_) => {}
        }
    }
    Ok((client, watcher))
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

/// Whether `pad` can be rumbled now, as the RUMBLE column says it: a
/// Steam Controller its kernel driver gives none is rumbled by a running
/// profile, through its raw node, and says so rather than `no`.
fn rumble_word(pad: &ctrlassist::Pad) -> &'static str {
    match pad.rumble {
        Some(false) if pad.is_steam_controller() => "in a profile",
        rumble => rumble.map_or("unknown", yes_no),
    }
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
                rumble_word(pad).to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_free_steam_controller_without_rumble_says_when_it_has_it() {
        let pad = |vendor, product, rumble| ctrlassist::Pad {
            id: "address:FX12345678".to_string(),
            name: "Pad".to_string(),
            vendor,
            product,
            connected: true,
            hidden: false,
            setup: None,
            rumble,
            bus: None,
        };
        assert_eq!(
            rumble_word(&pad(0x28de, 0x1302, Some(false))),
            "in a profile"
        );
        assert_eq!(rumble_word(&pad(0x28de, 0x1302, Some(true))), "yes");
        assert_eq!(rumble_word(&pad(0x054c, 0x0ce6, Some(false))), "no");
        assert_eq!(rumble_word(&pad(0x054c, 0x0ce6, None)), "unknown");
    }
}
