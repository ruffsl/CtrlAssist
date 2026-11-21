//! `start`, `stop` and `setups`: running a kept profile, and what is
//! running.

use std::io::{self, Write};
use std::path::Path;

use anyhow::Result;

use crate::pads::{connect, hydrated, printable, table};

pub(crate) async fn start(socket: &Path, profile: &str) -> Result<()> {
    let (client, _watcher) = connect(socket).await?;
    client.start_setup(profile).await?;
    Ok(())
}

pub(crate) async fn stop(socket: &Path, setup: &str) -> Result<()> {
    let (client, _watcher) = connect(socket).await?;
    client.stop_setup(setup).await?;
    Ok(())
}

pub(crate) async fn setups(socket: &Path, json: bool) -> Result<()> {
    let setups = hydrated(socket).await?.list_setups()?;
    if json {
        writeln!(io::stdout(), "{}", serde_json::to_string(&setups)?)?;
        return Ok(());
    }
    if setups.is_empty() {
        writeln!(io::stdout(), "no setups exist")?;
        return Ok(());
    }
    // One row per pad and per virtual pad, under the setup they are in.
    let mut rows: Vec<[String; 4]> = Vec::new();
    for setup in &setups {
        let phase = match &setup.reason {
            Some(reason) => format!("{}: {}", setup.phase.as_str(), printable(reason)),
            None => setup.phase.as_str().to_string(),
        };
        rows.push([setup.name.clone(), phase, String::new(), String::new()]);
        for source in &setup.sources {
            let pad = match (&source.pad, source.present) {
                (Some(pad), true) => printable(pad),
                (Some(pad), false) => format!("{} (missing)", printable(pad)),
                (None, _) => "(waiting for a pad that fits)".to_string(),
            };
            rows.push([
                String::new(),
                String::new(),
                format!("pad {}", source.name),
                pad,
            ]);
        }
        for sink in &setup.sinks {
            let node = sink.node.as_deref().unwrap_or("(not made)");
            rows.push([
                String::new(),
                String::new(),
                format!("virtual pad {}", sink.name),
                format!("{} at {node}", sink.identity),
            ]);
        }
    }
    table(["SETUP", "PHASE", "PART", "IS"], &rows)?;
    Ok(())
}
