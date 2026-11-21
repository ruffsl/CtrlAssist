//! The `ctrlassist` command.
//!
//! It writes with `writeln!`, never `println!` or `eprintln!`, which
//! panic on a closed pipe. Results are `writeln!(io::stdout(), …)?`, and
//! `main` ends a run whose reader has gone quietly.

mod pads;
mod status;

use std::io;

use anyhow::Result;
use clap::Parser;
use ctrlassist_cli::{Cli, Command, Service};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let command = match cli.command {
        // The process becomes that service, which builds its own runtime:
        // the daemon's input thread must exist before any runtime does.
        Command::Service(Service::Daemon(mut args)) => {
            args.socket = args.socket.or(cli.socket);
            return ctrlassist_daemon::service::main(args);
        }
        command => command,
    };
    let socket = cli
        .socket
        .unwrap_or_else(ctrlassist_protocol::bootstrap::default_socket_path);
    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            match command {
                Command::Status { json } => status::status(&socket, json).await,
                Command::Pads { json } => pads::pads(&socket, json).await,
                Command::Identities { json } => pads::identities(&socket, json).await,
                Command::Identify { pad, millis } => pads::identify(&socket, &pad, millis).await,
                Command::Forget { pad } => pads::forget(&socket, &pad).await,
                Command::Service(_) => Ok(()),
            }
        });
    // A reader that stopped early (`ctrlassist status | head`) isn't a
    // failure: end quietly with status 0, so a `pipefail` pipeline holds.
    match result {
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|e| e.kind() == io::ErrorKind::BrokenPipe) =>
        {
            Ok(())
        }
        result => result,
    }
}
