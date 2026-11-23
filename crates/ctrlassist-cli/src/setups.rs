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

pub(crate) async fn update(socket: &Path, setup: &str, dry_run: bool) -> Result<()> {
    let (client, _watcher) = connect(socket).await?;
    let disruptive = client.update_setup(setup, !dry_run).await?;
    let said = match (dry_run, disruptive) {
        (true, true) => "would restart: games would see the virtual pads disconnect and connect",
        (true, false) => "would apply without a game noticing",
        (false, true) => "restarted: games saw the virtual pads disconnect and connect",
        (false, false) => "applied without a game noticing",
    };
    writeln!(io::stdout(), "{setup}: {said}")?;
    Ok(())
}

pub(crate) async fn reload(socket: &Path) -> Result<()> {
    let (client, _watcher) = connect(socket).await?;
    let reloaded = client.reload().await?;
    let mut out = io::stdout();
    if reloaded == Default::default() {
        writeln!(out, "nothing changed")?;
    }
    // A problem quotes the profile's own text, which a terminal would
    // obey; names are filtered too, as they come from files' names.
    for name in &reloaded.changed {
        writeln!(out, "{}: read again", printable(name))?;
    }
    for name in &reloaded.removed {
        writeln!(out, "{}: gone", printable(name))?;
    }
    for name in &reloaded.updating {
        writeln!(
            out,
            "{}: its running setup is taking it up",
            printable(name)
        )?;
    }
    for profile in &reloaded.wrong {
        if let Some(problem) = &profile.problem {
            writeln!(
                out,
                "{}: wrong, so it can't be run: {}",
                printable(&profile.name),
                printable(&problem.to_string())
            )?;
        }
    }
    Ok(())
}

pub(crate) async fn select(
    socket: &Path,
    setup: &str,
    node: &str,
    choice: Option<&str>,
) -> Result<()> {
    let (client, _watcher) = connect(socket).await?;
    client.select(setup, node, choice).await?;
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
        for node in &setup.nodes {
            let choices: Vec<String> = node
                .choices
                .iter()
                .enumerate()
                .map(|(index, choice)| {
                    if index == node.selected {
                        format!("[{choice}]")
                    } else {
                        choice.clone()
                    }
                })
                .collect();
            rows.push([
                String::new(),
                String::new(),
                format!("node {}", node.name),
                choices.join(" "),
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
