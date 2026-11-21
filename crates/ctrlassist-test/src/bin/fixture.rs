//! The fixture the virtual machine checks drive: fake pads that hold until
//! killed, and one-line observations a test script can read.

#![expect(
    clippy::print_stdout,
    reason = "this binary's output is what the test scripts read"
)]

use std::path::PathBuf;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use ctrlassist_test::{FakeHidPad, FakePad, NodeKind, can_open, count_reads, pads};

fn parse_hex(text: &str) -> Result<u16, std::num::ParseIntError> {
    u16::from_str_radix(text.trim_start_matches("0x"), 16)
}

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a uinput pad and hold it; with --pulse, press a button on
    /// that period in milliseconds.
    FakePad {
        #[arg(long, default_value = "Fixture Fake Pad")]
        name: String,
        #[arg(long, value_parser = parse_hex, default_value = "1209")]
        vendor: u16,
        #[arg(long, value_parser = parse_hex, default_value = "5eed")]
        product: u16,
        #[arg(long, default_value_t = 0)]
        pulse: u64,
    },
    /// Create a uhid pad and hold it; with --pulse, press a button on that
    /// period in milliseconds.
    FakeHid {
        #[arg(long, default_value = "Fixture Fake HID Pad")]
        name: String,
        #[arg(long, default_value = "02:00:00:00:00:01")]
        uniq: String,
        #[arg(long, value_parser = parse_hex, default_value = "1209")]
        vendor: u16,
        #[arg(long, value_parser = parse_hex, default_value = "5eee")]
        product: u16,
        #[arg(long, default_value_t = 0)]
        pulse: u64,
    },
    /// List pads, one per line: name, uniq, vendor:product, then each node
    /// with whether the caller can open it.
    Pads,
    /// Count reads on a node for a window, printing the count each second
    /// and whether the node was cut off.
    Listen {
        node: PathBuf,
        #[arg(long, default_value_t = 1000)]
        millis: u64,
    },
}

/// Holds forever, calling `edge` on each pulse if there is one.
fn hold(pulse: u64, mut edge: impl FnMut(bool) -> std::io::Result<()>) -> Result<()> {
    let mut pressed = false;
    loop {
        if pulse == 0 {
            sleep(Duration::from_secs(3600));
        } else {
            pressed = !pressed;
            edge(pressed)?;
            sleep(Duration::from_millis(pulse));
        }
    }
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::FakePad {
            name,
            vendor,
            product,
            pulse,
        } => {
            let mut pad =
                FakePad::new(&name, vendor, product).context("creating the uinput pad")?;
            println!("node {}", pad.event_node().display());
            hold(pulse, |pressed| pad.south(pressed))
        }
        Command::FakeHid {
            name,
            uniq,
            vendor,
            product,
            pulse,
        } => {
            let mut pad =
                FakeHidPad::new(&name, &uniq, vendor, product).context("creating the uhid pad")?;
            println!("created");
            hold(pulse, |pressed| pad.report(u8::from(pressed), 0, 0))
        }
        Command::Pads => {
            for pad in pads()? {
                let nodes: Vec<String> = pad
                    .nodes
                    .iter()
                    .map(|(kind, node)| {
                        let kind = match kind {
                            NodeKind::Event => "event",
                            NodeKind::Js => "js",
                            NodeKind::Hidraw => "hidraw",
                        };
                        let access = if can_open(node, false) {
                            "open"
                        } else {
                            "denied"
                        };
                        format!("{kind}={}:{access}", node.display())
                    })
                    .collect();
                println!(
                    "pad name={:?} uniq={:?} id={}:{} hid={} {}",
                    pad.name,
                    pad.uniq,
                    pad.vendor,
                    pad.product,
                    pad.hid.as_deref().unwrap_or("-"),
                    nodes.join(" "),
                );
            }
            Ok(())
        }
        Command::Listen { node, millis } => {
            let reads = count_reads(
                &node,
                Duration::from_millis(millis),
                &mut |seconds, count| {
                    println!("t={seconds}s reads={count}");
                },
            )
            .with_context(|| format!("listening on {}", node.display()))?;
            println!("reads={} cut_off={}", reads.count, reads.cut_off);
            Ok(())
        }
    }
}
