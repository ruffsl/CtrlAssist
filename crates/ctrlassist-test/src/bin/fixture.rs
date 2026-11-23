//! The fixture the virtual machine checks drive: fake pads that hold until
//! killed, and one-line observations a test script can read.

#![expect(
    clippy::print_stdout,
    reason = "this binary's output is what the test scripts read"
)]

use std::fs::File;
use std::io::Read;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use ctrlassist_core::gamepad;
use ctrlassist_core::{
    Graph, GraphSpec, Input, Layout, Mode, MuxConfig, NodeId, NodeSpec, SourceId, SourceSpec,
};
use ctrlassist_device::{
    Change, Identity, InputThread, Monitor, Pad, Report, Setup, SetupId, VirtualPad,
};
use ctrlassist_hider::{Client, Hidden};
use ctrlassist_test::{FakeHidPad, FakePad, NodeKind, can_open, count_reads, pads};
use rustix::event::{PollFd, PollFlags, poll};

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
    /// Create a virtual DualSense from data of the test crate's own and
    /// hold it, answering the kernel; with --pulse, press a button on
    /// that period in milliseconds. It has the address of one of the
    /// daemon's own virtual pads unless --real, which gives it one a
    /// real pad could have, so that it stands in for a connected one.
    FakeDualsense {
        #[arg(long)]
        real: bool,
        #[arg(long, default_value_t = 1)]
        number: u8,
        #[arg(long, default_value_t = 0)]
        pulse: u64,
    },
    /// List pads, one per line: name, uniq, vendor:product, then each node
    /// with whether the caller can open it.
    Pads,
    /// Ask the hiding service to hide the pad at `node`, then hold the
    /// connection and take orders from files in `control`: `reopen`,
    /// `restore` or `count` appearing there is carried out, and the
    /// outcome written beside it as `<order>.done`.
    Hide {
        node: PathBuf,
        #[arg(long)]
        socket: PathBuf,
        #[arg(long)]
        control: PathBuf,
    },
    /// Ask the hiding service its release, and print it.
    Version {
        #[arg(long)]
        socket: PathBuf,
    },
    /// Ask the hiding service for a pad's raw HID node, and say what it
    /// answered.
    Raw {
        node: PathBuf,
        #[arg(long)]
        socket: PathBuf,
    },
    /// Read a pad as the daemon does and print each state it reports,
    /// stopping after `frames` of them.
    Read {
        node: PathBuf,
        #[arg(long, default_value_t = 4)]
        frames: usize,
    },
    /// Read a pad and write what it holds to a new virtual pad, until
    /// killed. Prints the virtual pad's event node first.
    Forward {
        node: PathBuf,
        /// Present the virtual pad as an Xbox pad, not a generic one.
        #[arg(long)]
        xbox: bool,
    },
    /// Run the input thread with a priority mux of two pads into one
    /// virtual pad, until killed. Prints the virtual pad's event node.
    Mux { primary: PathBuf, assist: PathBuf },
    /// Count reads on a node for a window, printing the count each second
    /// and whether the node was cut off.
    Listen {
        node: PathBuf,
        #[arg(long, default_value_t = 1000)]
        millis: u64,
    },
    /// Watch pads come and go as the daemon hears them, through the
    /// device crate's monitor, and print each change it reports, one per
    /// line in the order it reports them, until killed. Prints
    /// `watching` once it listens.
    Watch {
        /// Ask this receive buffer of the kernel, in bytes, for the
        /// monitor's socket, which the kernel doubles and raises to its
        /// least, so that a few events overrun it.
        #[arg(long)]
        receive_buffer: Option<usize>,
    },
}

/// One line for each change the monitor reports.
fn said(change: Change) -> String {
    let joined = |paths: &[PathBuf]| {
        let paths: Vec<String> = paths
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        if paths.is_empty() {
            "-".to_string()
        } else {
            paths.join(",")
        }
    };
    match change {
        Change::Arrived(found) => format!(
            "arrived {} {} anchor={}",
            found.node.display(),
            found.syspath.display(),
            found.anchor.map_or_else(
                || "-".to_string(),
                |anchor| anchor.path.display().to_string()
            )
        ),
        Change::Left { node, syspath } => {
            format!("left {} {}", node.display(), syspath.display())
        }
        Change::Unplugged(device) => format!("unplugged {}", device.display()),
        Change::Rebinding { anchor, present } => {
            format!(
                "rebinding {} present={}",
                anchor.display(),
                joined(&present)
            )
        }
        Change::Overran => "overran".to_string(),
        other => format!("other {other:?}"),
    }
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

/// How many reads the service's descriptor yields in half a second.
fn count_from(hidden: &Hidden) -> Result<usize> {
    let mut file = File::from(hidden.event.try_clone()?);
    let mut buffer = [0u8; 4096];
    let mut count = 0;
    for _ in 0..50 {
        // The descriptor is non-blocking: a read with nothing waiting
        // returns at once.
        while matches!(file.read(&mut buffer), Ok(read) if read > 0) {
            count += 1;
        }
        sleep(Duration::from_millis(10));
    }
    Ok(count)
}

fn hide(node: &Path, socket: &Path, control: &Path) -> Result<()> {
    std::fs::create_dir_all(control)?;
    let done = |order: &str, outcome: String| -> Result<()> {
        // Written whole, then renamed, so a reader never sees half of it.
        println!("{order}: {outcome}");
        let partial = control.join(format!("{order}.partial"));
        std::fs::write(&partial, outcome)?;
        std::fs::rename(partial, control.join(format!("{order}.done")))?;
        Ok(())
    };
    let client = Client::connect(socket).context("connecting to the hiding service")?;
    // The pad is named by a descriptor this process could open itself.
    let pad = File::open(node).with_context(|| format!("opening {}", node.display()))?;
    let mut hidden = match client.hide(pad.as_fd()) {
        Ok(hidden) => hidden,
        Err(error) => return done("hide", format!("refused: {error}")),
    };
    drop(pad);
    done(
        "hide",
        format!("ok token={} dislodged={}", hidden.token.0, hidden.dislodged),
    )?;
    loop {
        for order in ["reopen", "restore", "raw", "count"] {
            let trigger = control.join(order);
            if !trigger.exists() {
                continue;
            }
            std::fs::remove_file(&trigger)?;
            let outcome = match order {
                "reopen" => match client.reopen(hidden.token) {
                    Ok(again) => {
                        hidden = again;
                        "ok".to_string()
                    }
                    Err(error) => format!("refused: {error}"),
                },
                "restore" => match client.restore(hidden.token) {
                    Ok(()) => "ok".to_string(),
                    Err(error) => format!("refused: {error}"),
                },
                "raw" => match client.raw_of(hidden.token) {
                    Ok(_) => "ok".to_string(),
                    Err(error) => format!("refused: {error}"),
                },
                _ => format!("reads={}", count_from(&hidden)?),
            };
            done(order, outcome)?;
        }
        sleep(Duration::from_millis(50));
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
        Command::FakeDualsense {
            real,
            number,
            pulse,
        } => {
            let address = if real {
                [0x02, 0xca, 0x7e, 0x00, 0x00, number]
            } else {
                ctrlassist_device::own_address(u32::from(number))
            };
            let data = ctrlassist_test::synthetic_dualsense(false);
            let mut pad =
                VirtualPad::dualsense(&data, address).context("creating the virtual DualSense")?;
            println!("node {}", pad.event_node().display());
            let mut pressed = false;
            loop {
                // Whatever reads the pad's raw node is answered at once;
                // the press waits its turn.
                let wait = (pulse > 0).then(|| rustix::event::Timespec {
                    tv_sec: (pulse / 1000) as i64,
                    tv_nsec: ((pulse % 1000) * 1_000_000) as i64,
                });
                let mut fds = [PollFd::new(&pad, PollFlags::IN)];
                let ready = poll(&mut fds, wait.as_ref())?;
                pad.serve(|_| Ok(()))?;
                if ready == 0 {
                    pressed = !pressed;
                    let state = ctrlassist_core::PadState::NEUTRAL
                        .with(gamepad::SOUTH, f32::from(u8::from(pressed)));
                    pad.write(&state)?;
                }
            }
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
        Command::Hide {
            node,
            socket,
            control,
        } => hide(&node, &socket, &control),
        Command::Version { socket } => {
            let client = Client::connect(&socket).context("connecting to the hiding service")?;
            match client.version() {
                Ok(release) => println!("version: {release}"),
                Err(error) => println!("version: refused: {error}"),
            }
            Ok(())
        }
        Command::Raw { node, socket } => {
            let client = Client::connect(&socket).context("connecting to the hiding service")?;
            let pad = File::open(&node).with_context(|| format!("opening {}", node.display()))?;
            match client.raw(pad.as_fd()) {
                Ok(_) => println!("raw: ok"),
                Err(error) => println!("raw: refused: {error}"),
            }
            Ok(())
        }
        Command::Read { node, frames } => {
            let file = File::open(&node).with_context(|| format!("opening {}", node.display()))?;
            let mut pad = Pad::from_fd(file.into()).context("reading the pad")?;
            let identity = pad.identifiers();
            println!(
                "pad name={:?} id={:04x}:{:04x} address={:?} serial={:?}",
                identity.name, identity.vendor, identity.product, identity.address, identity.serial
            );
            let mut seen = 0;
            while seen < frames {
                poll(&mut [PollFd::new(&pad, PollFlags::IN)], None)?;
                pad.read(|state| {
                    seen += 1;
                    println!(
                        "frame south={} left_x={:.2}",
                        state.get(gamepad::SOUTH),
                        state.get(gamepad::LEFT_X)
                    );
                })
                .context("reading the pad")?;
            }
            Ok(())
        }
        Command::Forward { node, xbox } => {
            let file = File::open(&node).with_context(|| format!("opening {}", node.display()))?;
            let mut pad = Pad::from_fd(file.into()).context("reading the pad")?;
            let identity = if xbox {
                Identity::Xbox
            } else {
                Identity::Generic
            };
            let mut sink = VirtualPad::create(identity).context("creating the virtual pad")?;
            println!("virtual node={}", sink.event_node().display());
            sink.write(pad.state())?;
            loop {
                let (from_pad, from_game) = {
                    let mut fds = [
                        PollFd::new(&pad, PollFlags::IN),
                        PollFd::new(&sink, PollFlags::IN),
                    ];
                    poll(&mut fds, None)?;
                    (!fds[0].revents().is_empty(), !fds[1].revents().is_empty())
                };
                if from_pad {
                    let mut written = Ok(());
                    pad.read(|state| written = sink.write(state))
                        .context("reading the pad")?;
                    written.context("writing the virtual pad")?;
                }
                if from_game {
                    // Every request is accepted and goes nowhere.
                    sink.serve(|_| Ok(())).context("serving the virtual pad")?;
                }
            }
        }
        Command::Mux { primary, assist } => {
            let open = |node: &Path| -> Result<Pad> {
                let file =
                    File::open(node).with_context(|| format!("opening {}", node.display()))?;
                Pad::from_fd(file.into()).context("reading the pad")
            };
            let source = || SourceSpec {
                deadzone: 0.1,
                layout: Layout::gamepad(),
            };
            let graph = Graph::build(GraphSpec {
                sources: vec![source(), source()],
                nodes: vec![NodeSpec::Mux {
                    inputs: vec![Input::Source(SourceId(0)), Input::Source(SourceId(1))],
                    config: MuxConfig::new(Mode::Priority, &Layout::gamepad()),
                }],
                sinks: vec![Input::Node(NodeId(0), 0)],
            })
            .context("building the graph")?;
            let sink = VirtualPad::create(Identity::Generic).context("creating the virtual pad")?;
            println!("virtual node={}", sink.event_node().display());
            // Watching from before the pads are opened, so that one
            // leaving and returning can't fall between the two.
            let mut monitor = Monitor::new()?;
            let pads = [open(&primary)?, open(&assist)?];
            let ids = pads.each_ref().map(|pad| pad.identifiers().id());
            let setup = Setup::new(graph, pads.map(Some).into(), vec![sink]);
            let (input, reports) = InputThread::spawn()?;
            println!("realtime={}", input.is_realtime());
            input.send(ctrlassist_device::Command::Start(
                SetupId(0),
                Box::new(setup),
            ))?;
            // Holds until killed, saying what the input thread reports,
            // and giving a pad that went its place back when it returns.
            for report in reports {
                println!("report {report:?}");
                let Report::PadGone { source, .. } = report else {
                    continue;
                };
                let pad = loop {
                    poll(&mut [PollFd::new(&monitor, PollFlags::IN)], None)?;
                    let mut back = None;
                    monitor.read(|change| {
                        if let Change::Arrived(found) = change
                            && found.identifiers.id() == ids[source.0]
                        {
                            // An arrival from before it left names a node
                            // that is gone, and doesn't open.
                            back = open(&found.node).ok().or(back.take());
                        }
                    });
                    if let Some(pad) = back {
                        break pad;
                    }
                };
                input.send(ctrlassist_device::Command::PadBack {
                    setup: SetupId(0),
                    source,
                    pad: Box::new(pad),
                })?;
                println!("pad back source={}", source.0);
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
        Command::Watch { receive_buffer } => {
            let mut monitor = Monitor::new().context("listening to udev")?;
            if let Some(bytes) = receive_buffer {
                rustix::net::sockopt::set_socket_recv_buffer_size(&monitor, bytes)
                    .context("setting the monitor's receive buffer")?;
            }
            println!("watching");
            loop {
                poll(&mut [PollFd::new(&monitor, PollFlags::IN)], None)?;
                monitor.read(|change| println!("{}", said(change)));
            }
        }
    }
}
