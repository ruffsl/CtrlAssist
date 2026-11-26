//! Makes the user guide's screenshots of the settings window: the real
//! window, filled from made-up state, drawn off screen by Slint's
//! software renderer and written as PNG files. No display, no daemon, and
//! nobody at the mouse, so they can be made again whenever the window
//! changes (`just docs-screenshots`).
//!
//! The fonts and the icon theme are the machine's, so two machines don't
//! make the same bytes; nothing compares them.

use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use clap::Parser;
use ctrlassist::{Hiding, Pad, Phase, Profile, Setup, SetupSource};
use ctrlassist_gui::state::Snapshot;
use ctrlassist_gui::{AppWindow, Pads, Pane};
use slint::ComponentHandle as _;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter, WindowEvent};

/// The size the window opens at, in its own pixels.
const SIZE: (f32, f32) = (720.0, 480.0);
/// How many pixels of the picture to each of the window's.
const SCALE: f32 = 2.0;
/// How many times Slint's timers and animations are stepped before a
/// capture, there being no event loop to run them: enough for a change
/// of page to finish moving.
const SETTLE_STEPS: u32 = 20;
/// The time between two such steps.
const SETTLE_STEP: Duration = Duration::from_millis(20);

#[derive(Parser, Debug)]
#[command(name = "ctrlassist-screenshots", about)]
struct Args {
    /// The directory the screenshots are written to.
    #[arg(default_value = "docs/screenshots")]
    out: PathBuf,
}

/// A platform with one window that is drawn only when asked.
struct OffScreen {
    window: Rc<MinimalSoftwareWindow>,
    started: Instant,
}

impl Platform for OffScreen {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }

    fn duration_since_start(&self) -> Duration {
        self.started.elapsed()
    }
}

/// The scheme a desktop would report, which decides the window's theme.
#[derive(Clone, Copy)]
enum Scheme {
    Dark,
}

/// One screenshot: its file, its theme, and what the window shows.
struct Shot {
    file: &'static str,
    scheme: Scheme,
    pane: Pane,
    /// Which row of the pane's list is selected.
    selected: i32,
}

const SHOTS: &[Shot] = &[Shot {
    file: "pads_pane.png",
    scheme: Scheme::Dark,
    pane: Pane::Pads,
    selected: 0,
}];

const DUALSENSE: &str = "address:0a:1b:2c:3d:4e:14";
const XBOX: &str = "address:1a:2b:3c:4d:5e:da";
const EIGHTBITDO: &str = "address:2a:3b:4c:5d:6e:0f";

fn pad(id: &str, name: &str, (vendor, product): (u16, u16), setup: Option<&str>) -> Pad {
    // The one pad not in a setup is the one that is away.
    let connected = setup.is_some();
    Pad {
        id: id.to_string(),
        name: name.to_string(),
        vendor,
        product,
        connected,
        hidden: connected,
        setup: setup.map(str::to_string),
        rumble: Some(true),
        // Bluetooth.
        bus: connected.then_some(5),
    }
}

fn source(name: &str, pad: &str) -> SetupSource {
    SetupSource {
        name: name.to_string(),
        pad: Some(pad.to_string()),
        present: true,
    }
}

fn profile(name: &str, toml: String) -> Profile {
    Profile {
        name: name.to_string(),
        toml,
        unset: Vec::new(),
        problem: None,
    }
}

/// What every screenshot shows of the daemon, so that they agree with
/// each other: two pads playing as one in `couch`, a third remembered,
/// and `desk` kept and not running. Nobody's real pad, address or name.
fn made_up() -> Snapshot {
    let release = env!("CARGO_PKG_VERSION").to_string();
    Snapshot {
        available: true,
        hydrated: true,
        version: Some(release.clone()),
        hiding: Some(Hiding::Service),
        hider: Some(release.clone()),
        config: "/home/player/.config/ctrlassist".to_string(),
        realtime: true,
        tray_available: true,
        tray: Some(release),
        tray_config: "/home/player/.config/ctrlassist/tray.toml".to_string(),
        pads: vec![
            pad(
                DUALSENSE,
                "DualSense Wireless Controller",
                (0x054c, 0x0ce6),
                Some("couch"),
            ),
            pad(
                XBOX,
                "Xbox Wireless Controller",
                (0x045e, 0x0b13),
                Some("couch"),
            ),
            pad(EIGHTBITDO, "8BitDo Pro 2", (0x2dc8, 0x6006), None),
        ],
        setups: vec![Setup {
            name: "couch".to_string(),
            start: 1,
            phase: Phase::Running,
            reason: None,
            sources: vec![source("primary", DUALSENSE), source("assist", XBOX)],
            sinks: Vec::new(),
            nodes: Vec::new(),
            stale: false,
        }],
        profiles: vec![
            profile(
                "couch",
                format!("mode = \"priority\"\nprimary = \"{DUALSENSE}\"\nassist = \"{XBOX}\"\n"),
            ),
            profile(
                "desk",
                format!("mode = \"unicast\"\npad = \"{DUALSENSE}\"\n"),
            ),
        ],
        fields: ctrlassist_protocol::form::fields(),
        ..Snapshot::default()
    }
}

/// Tells Slint which scheme the desktop has. A backend does this for a
/// window on screen; a platform made here has no public way to, so it is
/// done through Slint's own context, at the release the workspace pins.
fn set_scheme(app: &AppWindow, scheme: Scheme) {
    let scheme = match scheme {
        Scheme::Dark => i_slint_core::items::ColorScheme::Dark,
    };
    i_slint_core::window::WindowInner::from_pub(app.window())
        .context()
        .set_color_scheme(scheme);
}

/// Steps what an event loop would run, until a change has finished.
fn settle(window: &MinimalSoftwareWindow) {
    for _ in 0..SETTLE_STEPS {
        slint::platform::update_timers_and_animations();
        window.draw_if_needed(|_| {});
        std::thread::sleep(SETTLE_STEP);
    }
}

fn capture(app: &AppWindow, window: &MinimalSoftwareWindow, shot: &Shot, out: &Path) -> Result<()> {
    set_scheme(app, shot.scheme);
    app.set_active_pane(shot.pane);
    if shot.pane == Pane::Pads {
        app.global::<Pads>().set_selected_index(shot.selected);
    }
    settle(window);
    let pixels = app
        .window()
        .take_snapshot()
        .map_err(|error| anyhow::anyhow!("the window couldn't be drawn: {error}"))?;
    let path = out.join(shot.file);
    let file = File::create(&path).with_context(|| format!("couldn't make {}", path.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), pixels.width(), pixels.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(pixels.as_bytes()))
        .with_context(|| format!("couldn't write {}", path.display()))?;
    writeln!(std::io::stdout(), "{}", path.display())?;
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();
    std::fs::create_dir_all(&args.out)
        .with_context(|| format!("couldn't make {}", args.out.display()))?;

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(OffScreen {
        window: window.clone(),
        started: Instant::now(),
    }))
    .map_err(|error| anyhow::anyhow!("Slint already has a platform: {error}"))?;

    let app = AppWindow::new().context("the window couldn't be built")?;
    app.window()
        .dispatch_event(WindowEvent::ScaleFactorChanged {
            scale_factor: SCALE,
        });
    app.window()
        .set_size(slint::LogicalSize::new(SIZE.0, SIZE.1));
    ctrlassist_gui::load_icons(&app);
    made_up().show(&app);
    app.show().context("the window couldn't be shown")?;
    for shot in SHOTS {
        capture(&app, &window, shot, &args.out)?;
    }
    Ok(())
}
