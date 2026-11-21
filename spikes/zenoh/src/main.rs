//! The zenoh spike: the loop spike's input thread inside a daemon that
//! also runs a zenoh session on a Unix socket, in TisVlay's style, plus
//! the clients needed to exercise it.
//!
//! `mux` takes the same arguments and prints the same line as the loop
//! spike's, so that spike's bench can time it unchanged.

mod mux;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use anyhow::{Result, anyhow};
use clap::{Parser, Subcommand};
use zenoh::Config;

const STATUS: &str = "ctrlassist/status";
const PING: &str = "ctrlassist/rpc/ping";
const ALIVE: &str = "ctrlassist/alive";
const MIRROR: &str = "ctrlassist/mirror/0";

#[derive(Parser)]
struct Cli {
    /// The daemon's socket.
    #[arg(long, global = true, default_value = "/tmp/ctrlassist-spike.sock")]
    socket: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// The daemon: the input loop on its own thread, zenoh on a runtime.
    Mux {
        #[arg(long)]
        primary: PathBuf,
        #[arg(long)]
        assist: PathBuf,
        #[arg(long)]
        rt: bool,
    },
    /// One rpc and exit, as a CLI verb would.
    Ping,
    /// Subscribe to a key and count what arrives until killed.
    Watch {
        /// Subscribe to the pad mirror instead of the status.
        #[arg(long)]
        mirror: bool,
    },
}

pub(crate) fn make_realtime(priority: i32) -> bool {
    let param = libc::sched_param {
        sched_priority: priority,
    };
    // SAFETY: `param` is a valid sched_param for the duration of the call;
    // pid 0 means the calling thread.
    unsafe { libc::sched_setscheduler(0, libc::SCHED_FIFO, &raw const param) == 0 }
}

fn zerr(error: zenoh::Error) -> anyhow::Error {
    anyhow!(error)
}

fn config(mode: &str, key: &str, socket: &Path) -> Result<Config> {
    let mut config = Config::default();
    let endpoint = format!("[\"unixsock-stream/{}\"]", socket.display());
    for (key, value) in [
        ("mode", format!("\"{mode}\"")),
        ("scouting/multicast/enabled", "false".to_string()),
        ("scouting/gossip/enabled", "false".to_string()),
        (key, endpoint),
    ] {
        config.insert_json5(key, &value).map_err(zerr)?;
    }
    Ok(config)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Mux { primary, assist, rt } => daemon(&cli.socket, primary, assist, rt),
        Command::Ping => runtime(1)?.block_on(ping(&cli.socket)),
        Command::Watch { mirror } => runtime(1)?.block_on(watch(&cli.socket, mirror)),
    }
}

fn runtime(workers: usize) -> Result<tokio::runtime::Runtime> {
    // zenoh needs the multi-thread scheduler.
    Ok(tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()?)
}

fn daemon(socket: &Path, primary: PathBuf, assist: PathBuf, rt: bool) -> Result<()> {
    let _ = std::fs::remove_file(socket);
    let wanted = Arc::new(AtomicBool::new(false));
    let (samples, mut received) = tokio::sync::mpsc::channel::<(u16, i32)>(64);
    let mirror = mux::Mirror {
        wanted: Arc::clone(&wanted),
        samples,
    };

    // The input thread is created before the runtime and shares nothing
    // with it but the mirror: no lock, no allocation, no task.
    let input = std::thread::Builder::new()
        .name("input".into())
        .spawn(move || mux::run(&primary, &assist, rt, &mirror))?;

    let socket = socket.to_path_buf();
    runtime(2)?.block_on(async move {
        let session = zenoh::open(config("router", "listen/endpoints", &socket)?)
            .await
            .map_err(zerr)?;
        let _alive = session.liveliness().declare_token(ALIVE).await.map_err(zerr)?;
        let status = session.declare_publisher(STATUS).await.map_err(zerr)?;
        status.put("running").await.map_err(zerr)?;

        let ping = session.declare_queryable(PING).await.map_err(zerr)?;
        tokio::spawn(async move {
            while let Ok(query) = ping.recv_async().await {
                let _ = query.reply(PING, "pong").await;
            }
        });

        // The mirror publishes only while something subscribes: the
        // publisher's matching status switches the input thread's flag.
        let mirror = session.declare_publisher(MIRROR).await.map_err(zerr)?;
        let matching = mirror.matching_listener().await.map_err(zerr)?;
        tokio::spawn(async move {
            while let Ok(status) = matching.recv_async().await {
                wanted.store(status.matching(), Ordering::Relaxed);
            }
        });
        while let Some((code, value)) = received.recv().await {
            let _ = mirror.put(format!("{code}:{value}")).await;
        }
        anyhow::Ok(())
    })?;
    input.join().map_err(|_| anyhow!("the input thread panicked"))?
}

async fn ping(socket: &Path) -> Result<()> {
    let started = Instant::now();
    let session = zenoh::open(config("client", "connect/endpoints", socket)?)
        .await
        .map_err(zerr)?;
    let opened = started.elapsed();
    let replies = session.get(PING).await.map_err(zerr)?;
    let reply = replies.recv_async().await.map_err(|e| anyhow!("no reply: {e}"))?;
    let ok = reply.result().is_ok();
    // Printed before the session closes, so the time is to the answer.
    let answered = started.elapsed();
    session.close().await.map_err(zerr)?;
    let mut out = std::io::stdout();
    std::io::Write::write_fmt(
        &mut out,
        format_args!(
            "reply_ok={ok} open={}us answered={}us closed={}us\n",
            opened.as_micros(),
            answered.as_micros(),
            started.elapsed().as_micros()
        ),
    )?;
    Ok(())
}

async fn watch(socket: &Path, mirror: bool) -> Result<()> {
    let session = zenoh::open(config("client", "connect/endpoints", socket)?)
        .await
        .map_err(zerr)?;
    let key = if mirror { MIRROR } else { STATUS };
    let subscriber = session.declare_subscriber(key).await.map_err(zerr)?;
    let mut count = 0u64;
    let mut last = Instant::now();
    while subscriber.recv_async().await.is_ok() {
        count += 1;
        if last.elapsed().as_secs() >= 1 {
            let mut out = std::io::stderr();
            let _ = std::io::Write::write_fmt(&mut out, format_args!("{key}: {count} samples so far\n"));
            last = Instant::now();
        }
    }
    Ok(())
}
