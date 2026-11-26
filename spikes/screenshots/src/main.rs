//! Draws the settings window off screen with Slint's software renderer,
//! filled from made-up state, and writes what it drew as a PNG.

use std::rc::Rc;

use ctrlassist::{Hiding, Pad, Phase, Profile, Setup, SetupSource};
use ctrlassist_gui::state::Snapshot;
pub use ctrlassist_gui::*;
use slint::ComponentHandle;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};

// The window's own icon loading, which the library keeps to itself.
#[path = "../../../crates/ctrlassist-gui-app/src/icon_theme.rs"]
mod icon_theme;
#[path = "../../../crates/ctrlassist-gui-app/src/icons.rs"]
mod icons;

struct OffScreen {
    window: Rc<MinimalSoftwareWindow>,
    started: std::time::Instant,
}

impl slint::platform::Platform for OffScreen {
    fn create_window_adapter(
        &self,
    ) -> Result<Rc<dyn slint::platform::WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> std::time::Duration {
        self.started.elapsed()
    }
}

fn pad(id: &str, name: &str, vendor: u16, product: u16, connected: bool, setup: Option<&str>) -> Pad {
    Pad {
        id: id.to_string(),
        name: name.to_string(),
        vendor,
        product,
        connected,
        hidden: setup.is_some(),
        setup: setup.map(str::to_string),
        rumble: Some(true),
        bus: connected.then_some(5),
    }
}

fn snapshot() -> Snapshot {
    const DUALSENSE: &str = "address:0a:1b:2c:3d:4e:14";
    const XBOX: &str = "address:1a:2b:3c:4d:5e:da";
    let version = env!("CARGO_PKG_VERSION_OF_WINDOW").to_string();
    let kept = |name: &str, toml: &str| Profile {
        name: name.to_string(),
        toml: toml.to_string(),
        unset: Vec::new(),
        problem: None,
    };
    Snapshot {
        available: true,
        hydrated: true,
        version: Some(version.clone()),
        hiding: Some(Hiding::Service),
        hider: Some(version.clone()),
        config: "/home/player/.config/ctrlassist".to_string(),
        realtime: true,
        tray_available: true,
        tray: Some(version),
        tray_config: "/home/player/.config/ctrlassist/tray.toml".to_string(),
        pads: vec![
            pad(DUALSENSE, "DualSense Wireless Controller", 0x054c, 0x0ce6, true, Some("couch")),
            pad(XBOX, "Xbox Wireless Controller", 0x045e, 0x0b13, true, Some("couch")),
            pad("address:2a:3b:4c:5d:6e:0f", "8BitDo Pro 2", 0x2dc8, 0x6006, false, None),
        ],
        setups: vec![Setup {
            name: "couch".to_string(),
            start: 1,
            phase: Phase::Running,
            reason: None,
            sources: vec![
                SetupSource {
                    name: "primary".to_string(),
                    pad: Some(DUALSENSE.to_string()),
                    present: true,
                },
                SetupSource {
                    name: "assist".to_string(),
                    pad: Some(XBOX.to_string()),
                    present: true,
                },
            ],
            sinks: Vec::new(),
            nodes: Vec::new(),
            stale: false,
        }],
        profiles: vec![
            kept(
                "couch",
                &format!("mode = \"priority\"\nprimary = \"{DUALSENSE}\"\nassist = \"{XBOX}\"\n"),
            ),
            kept("desk", &format!("mode = \"unicast\"\npad = \"{DUALSENSE}\"\n")),
        ],
        fields: ctrlassist_protocol::form::fields(),
        ..Snapshot::default()
    }
}

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "pads_pane.png".to_string());
    let dark = std::env::args().nth(2).as_deref() != Some("light");
    // The size the window opens at, and how many pixels to each of its.
    let scale: f32 = std::env::args().nth(3).and_then(|scale| scale.parse().ok()).unwrap_or(1.0);
    let (width, height) = ((720.0 * scale) as u32, (480.0 * scale) as u32);

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(OffScreen {
        window: window.clone(),
        started: std::time::Instant::now(),
    }))
    .expect("no platform was set before this one");

    let app = AppWindow::new().expect("the window builds");
    app.window().dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged { scale_factor: scale });
    app.window().set_size(slint::PhysicalSize::new(width, height));
    // The scheme a desktop would report.
    i_slint_core::window::WindowInner::from_pub(app.window())
        .context()
        .set_color_scheme(if dark {
            i_slint_core::items::ColorScheme::Dark
        } else {
            i_slint_core::items::ColorScheme::Light
        });
    icons::load(&app);
    snapshot().show(&app);
    app.set_active_pane(Pane::Pads);
    app.global::<Pads>().set_selected_index(0);
    app.show().expect("the window shows");

    // Let what follows a change settle: bindings, and animations run to
    // their ends.
    for _ in 0..20 {
        slint::platform::update_timers_and_animations();
        window.draw_if_needed(|_| {});
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let pixels = app.window().take_snapshot().expect("the renderer snapshots");
    let file = std::fs::File::create(&out).expect("the file opens");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), pixels.width(), pixels.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut writer| writer.write_image_data(pixels.as_bytes()))
        .expect("the PNG is written");
}
