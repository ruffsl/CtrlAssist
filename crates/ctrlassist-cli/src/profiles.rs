//! `profile`: the profiles the daemon keeps, read from its published
//! state, and written only through it.

use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use ctrlassist_cli::Profile;

use crate::pads::{connect, hydrated, printable, table};

fn read(file: &Path) -> Result<String> {
    std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))
}

pub(crate) async fn run(socket: &Path, command: Profile) -> Result<()> {
    match command {
        Profile::List { json } => {
            let profiles = hydrated(socket).await?.list_profiles()?;
            if json {
                writeln!(io::stdout(), "{}", serde_json::to_string(&profiles)?)?;
            } else if profiles.is_empty() {
                writeln!(io::stdout(), "no profiles are kept")?;
            } else {
                let rows: Vec<[String; 2]> = profiles
                    .iter()
                    .map(|profile| {
                        let problem = profile
                            .problem
                            .as_ref()
                            .map_or_else(String::new, |problem| printable(&problem.to_string()));
                        [profile.name.clone(), problem]
                    })
                    .collect();
                table(["NAME", "PROBLEM"], &rows)?;
            }
        }
        Profile::Show { name } => {
            let Some(profile) = hydrated(socket).await?.profile(&name)? else {
                bail!("no profile is called {name}");
            };
            write!(io::stdout(), "{}", profile.toml)?;
        }
        Profile::Validate { file } => {
            let toml = read(&file)?;
            let (client, _watcher) = connect(socket).await?;
            if let Some(problem) = client.validate_profile(&toml).await? {
                bail!("{}: {problem}", file.display());
            }
        }
        Profile::Put { file, name } => {
            let toml = read(&file)?;
            let name = match name {
                Some(name) => name,
                None => file
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        anyhow!(
                            "{} has no name to take; give one with --name",
                            file.display()
                        )
                    })?,
            };
            let (client, _watcher) = connect(socket).await?;
            client.put_profile(&name, &toml).await?;
        }
        Profile::Delete { name } => {
            let (client, _watcher) = connect(socket).await?;
            client.delete_profile(&name).await?;
        }
    }
    Ok(())
}
