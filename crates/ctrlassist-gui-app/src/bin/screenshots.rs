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
use ctrlassist::{
    Hiding, Identity, NotificationSettings, Pad, Phase, Profile, Setup, SetupNode, SetupSink,
    SetupSource,
};
use ctrlassist_gui::state::Snapshot;
use ctrlassist_gui::{AppWindow, Pads, Pane, Profiles, Theme};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter, WindowEvent};
use slint::{ComponentHandle as _, Model as _};

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
    Light,
}

/// The desktop's accent color, as red, green and blue: what the window
/// is told under Breeze Dark for a swatch of Plasma's color settings
/// (`AccentColorUI.qml`: `#3dd425`, `#926ee4`, `#e93d58`).
/// Plasma darkens a swatch for a dark scheme before it reports it, and
/// these are the reported colors, read off TisVlay's screenshots, which
/// were taken on such a desktop. Blue is the scheme's own, unchanged.
type Accent = (u8, u8, u8);
const BLUE: Accent = (0x3d, 0xae, 0xe9);
const GREEN: Accent = (0x30, 0x9b, 0x21);
const LAVENDER: Accent = (0x6c, 0x53, 0xa6);
const RED: Accent = (0xa9, 0x31, 0x44);

/// What a screenshot has the window show.
#[derive(Clone, Copy)]
enum View {
    /// The Pads page, on the pad with this name.
    Pad(&'static str),
    /// The Pads page, on the first virtual pad.
    VirtualPad,
    /// The Profiles page, on the profile with this name.
    Profile(&'static str),
    Settings,
    About,
}

/// One screenshot: its file, how the desktop looks, and the view.
struct Shot {
    file: &'static str,
    scheme: Scheme,
    accent: Accent,
    view: View,
}

/// The accents are those of TisVlay's screenshots, so the two projects'
/// pages look of a kind, but for its yellow, which is hard to read
/// under white text.
const SHOTS: &[Shot] = &[
    Shot {
        file: "pads_pane.png",
        scheme: Scheme::Dark,
        accent: BLUE,
        view: View::Pad(DUALSENSE_NAME),
    },
    Shot {
        file: "pads_pane_light.png",
        scheme: Scheme::Light,
        accent: BLUE,
        view: View::Pad(XBOX_NAME),
    },
    Shot {
        file: "pads_pane_virtual.png",
        scheme: Scheme::Dark,
        accent: LAVENDER,
        view: View::VirtualPad,
    },
    Shot {
        file: "profiles_pane.png",
        scheme: Scheme::Dark,
        accent: GREEN,
        view: View::Profile("couch"),
    },
    Shot {
        file: "profiles_pane_running.png",
        scheme: Scheme::Dark,
        accent: LAVENDER,
        view: View::Profile("handoff"),
    },
    Shot {
        file: "settings_pane.png",
        scheme: Scheme::Dark,
        accent: RED,
        view: View::Settings,
    },
    Shot {
        file: "about_pane.png",
        scheme: Scheme::Dark,
        accent: BLUE,
        view: View::About,
    },
];

const DUALSENSE: &str = "address:0a:1b:2c:3d:4e:14";
const DUALSENSE_NAME: &str = "DualSense Wireless Controller";
const XBOX: &str = "address:1a:2b:3c:4d:5e:da";
const XBOX_NAME: &str = "Xbox Wireless Controller";
const EIGHTBITDO: &str = "address:2a:3b:4c:5d:6e:0f";
const DUALSHOCK: &str = "address:3a:4b:5c:6d:7e:21";
const SWITCH: &str = "address:4a:5b:6c:7d:8e:3c";

fn pad(id: &str, name: &str, (vendor, product): (u16, u16), setup: Option<&str>) -> Pad {
    // A pad in no setup here is one that is away.
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

fn identity(id: &str, name: &str, vendor: u16, product: u16) -> Identity {
    Identity {
        id: id.to_string(),
        name: name.to_string(),
        vendor,
        product,
        available: true,
    }
}

fn running(
    name: &str,
    sources: Vec<SetupSource>,
    (identity, node): (&str, &str),
    nodes: Vec<SetupNode>,
) -> Setup {
    Setup {
        name: name.to_string(),
        start: 1,
        phase: Phase::Running,
        reason: None,
        sources,
        sinks: vec![SetupSink {
            name: "game".to_string(),
            identity: identity.to_string(),
            node: Some(node.to_string()),
        }],
        nodes,
        stale: false,
    }
}

/// What every screenshot shows of the daemon, so that they agree with
/// each other: two pads playing as one in `couch`, two more taking turns
/// in `handoff`, one remembered, and `desk` kept and not running.
/// Nobody's real pad, address or name.
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
            pad(
                DUALSHOCK,
                "DualShock 4 Wireless Controller",
                (0x054c, 0x09cc),
                Some("handoff"),
            ),
            pad(
                SWITCH,
                "Nintendo Switch Pro Controller",
                (0x057e, 0x2009),
                Some("handoff"),
            ),
            pad(EIGHTBITDO, "8BitDo Pro 2", (0x2dc8, 0x6006), None),
        ],
        setups: vec![
            running(
                "couch",
                vec![source("primary", DUALSENSE), source("assist", XBOX)],
                ("dualsense", "/dev/input/event27"),
                Vec::new(),
            ),
            running(
                "handoff",
                vec![source("primary", DUALSHOCK), source("assist", SWITCH)],
                ("xbox", "/dev/input/event28"),
                vec![SetupNode {
                    name: "mux".to_string(),
                    choices: vec!["primary".to_string(), "assist".to_string()],
                    selected: 0,
                }],
            ),
        ],
        profiles: vec![
            profile(
                "couch",
                format!(
                    "mode = \"priority\"\nprimary = \"{DUALSENSE}\"\nassist = \"{XBOX}\"\nidentity = \"dualsense\"\nassist-gyro = \"right-bumper\"\n"
                ),
            ),
            profile(
                "desk",
                format!("mode = \"unicast\"\npad = \"{EIGHTBITDO}\"\n"),
            ),
            profile(
                "handoff",
                format!(
                    "mode = \"toggle\"\nprimary = \"{DUALSHOCK}\"\nassist = \"{SWITCH}\"\nidentity = \"xbox\"\n"
                ),
            ),
        ],
        identities: vec![
            identity("generic", "CtrlAssist Virtual Gamepad", 0x1209, 0x5eed),
            identity(
                "xbox",
                "Microsoft Xbox Series S|X Controller",
                0x045e,
                0x0b12,
            ),
            identity("dualsense", "DualSense Wireless Controller", 0x054c, 0x0ce6),
        ],
        notifications: Some(NotificationSettings::default()),
        fields: ctrlassist_protocol::form::fields(),
    }
}

/// Tells Slint which scheme the desktop has. A backend does this for a
/// window on screen; a platform made here has no public way to, so it is
/// done through Slint's own context, at the release the workspace pins.
fn set_scheme(app: &AppWindow, scheme: Scheme) {
    let scheme = match scheme {
        Scheme::Dark => i_slint_core::items::ColorScheme::Dark,
        Scheme::Light => i_slint_core::items::ColorScheme::Light,
    };
    i_slint_core::window::WindowInner::from_pub(app.window())
        .context()
        .set_color_scheme(scheme);
}

/// The place in a model of the first row `wanted` is true of.
fn place<Row>(rows: &slint::ModelRc<Row>, what: &str, wanted: impl Fn(&Row) -> bool) -> Result<i32>
where
    Row: Clone + 'static,
{
    let at = rows
        .iter()
        .position(|row| wanted(&row))
        .with_context(|| format!("the made-up state has no {what}"))?;
    i32::try_from(at).context("a list longer than a selection can name")
}

/// Has the window show a view.
fn show(app: &AppWindow, view: View) -> Result<()> {
    match view {
        View::Pad(name) => {
            let pads = app.global::<Pads>();
            let at = place(&pads.get_rows(), name, |row| {
                !row.is_virtual && row.name == name
            })?;
            app.set_active_pane(Pane::Pads);
            pads.set_selected_index(at);
        }
        View::VirtualPad => {
            let pads = app.global::<Pads>();
            let at = place(&pads.get_rows(), "virtual pad", |row| row.is_virtual)?;
            app.set_active_pane(Pane::Pads);
            pads.set_selected_index(at);
        }
        View::Profile(name) => {
            let profiles = app.global::<Profiles>();
            let at = place(&profiles.get_rows(), name, |row| row.name == name)?;
            app.set_active_pane(Pane::Profiles);
            profiles.set_selected_index(at);
        }
        View::Settings => app.set_active_pane(Pane::Settings),
        View::About => app.set_active_pane(Pane::About),
    }
    Ok(())
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
    let (red, green, blue) = shot.accent;
    app.global::<Theme>()
        .set_accent(slint::Color::from_rgb_u8(red, green, blue));
    show(app, shot.view)?;
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
    // What the window's own start reads of itself and of the command on
    // PATH: here every part is of one release, whatever is installed.
    let release = env!("CARGO_PKG_VERSION");
    app.set_gui_version(release.into());
    app.set_cli_version(release.into());
    made_up().show(&app);
    app.show().context("the window couldn't be shown")?;
    for shot in SHOTS {
        capture(&app, &window, shot, &args.out)?;
    }
    Ok(())
}
