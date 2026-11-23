//! The `ctrlassist` command.
//!
//! It writes with `writeln!`, never `println!` or `eprintln!`, which
//! panic on a closed pipe. Results are `writeln!(io::stdout(), …)?`, and
//! `main` ends a run whose reader has gone quietly, and says an error
//! with its control characters made printable.

mod pads;
mod profiles;
mod setups;
mod status;

use std::io::{self, Write};
use std::process::ExitCode;

use anyhow::Result;
use clap::{CommandFactory, Parser};
use clap_complete::CompleteEnv;
use ctrlassist_cli::{Cli, Command, Service, printable};

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        // A reader that stopped early (`ctrlassist status | head`) isn't a
        // failure: end quietly with status 0, so a `pipefail` pipeline
        // holds.
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe) =>
        {
            ExitCode::SUCCESS
        }
        Err(error) => {
            // As returning the error from `main` would say it, but with
            // each line made printable: an error can quote the daemon's
            // refusal, and that a profile's own text.
            let said: Vec<String> = format!("{error:?}").lines().map(printable).collect();
            let _ = writeln!(io::stderr(), "Error: {}", said.join("\n"));
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    // Answer completion requests before building a runtime. The dynamic
    // engine shells back into this program per <TAB>, and the value
    // completers ask the daemon on a runtime of their own, which would
    // panic under an ambient one. A normal run (COMPLETE unset) falls
    // straight through.
    CompleteEnv::with_factory(Cli::command)
        .completer("ctrlassist")
        .complete();
    let cli = Cli::parse();
    let command = match cli.command {
        // The process becomes that service, which builds its own runtime:
        // the daemon's input thread must exist before any runtime does.
        Command::Service(Service::Daemon(mut args)) => {
            args.socket = args.socket.or(cli.socket);
            return ctrlassist_daemon::service::main(args);
        }
        Command::Service(Service::Tray(mut args)) => {
            args.socket = args.socket.or(cli.socket);
            return ctrlassist_tray::service::main(args);
        }
        command => command,
    };
    let socket = cli
        .socket
        .unwrap_or_else(ctrlassist_protocol::bootstrap::default_socket_path);
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            match command {
                Command::Status { json } => status::status(&socket, json).await,
                Command::Pads { json } => pads::pads(&socket, json).await,
                Command::Identities { json } => pads::identities(&socket, json).await,
                Command::Identify { pad, millis } => pads::identify(&socket, &pad, millis).await,
                Command::Forget { pad } => pads::forget(&socket, &pad).await,
                Command::Profile(command) => profiles::run(&socket, command).await,
                Command::Start { profile } => setups::start(&socket, &profile).await,
                Command::Stop { setup } => setups::stop(&socket, &setup).await,
                Command::Setups { json } => setups::setups(&socket, json).await,
                Command::Update { setup, dry_run } => {
                    setups::update(&socket, &setup, dry_run).await
                }
                Command::Select {
                    setup,
                    node,
                    choice,
                } => setups::select(&socket, &setup, &node, choice.as_deref()).await,
                Command::Reload => setups::reload(&socket).await,
                Command::Service(_) => Ok(()),
            }
        })
}
