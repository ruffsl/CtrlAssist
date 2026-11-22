//! User accent-color sync via xdg-desktop-portal's Settings namespace
//! (`org.freedesktop.appearance.accent-color`). One-shot initial read
//! plus a long-lived listener that pushes changes into `Theme.accent`.
//! Failure modes (no portal, no preference, out-of-range RGB) are
//! logged and silently fall through to the theme's hardcoded default.

use slint::ComponentHandle;
use tracing::debug;

use crate::{AppWindow, Theme, on_ui};

/// Convert the portal's RGB tuple (each in `[0, 1]`) into a Slint
/// color, or `None` if any component is out of range — the
/// xdg-desktop-portal Settings spec says out-of-range values should
/// be treated as "no preference set".
fn ashpd_color_to_slint(c: ashpd::desktop::Color) -> Option<slint::Color> {
    let (r, g, b) = (c.red(), c.green(), c.blue());
    if !(0.0..=1.0).contains(&r) || !(0.0..=1.0).contains(&g) || !(0.0..=1.0).contains(&b) {
        return None;
    }
    Some(slint::Color::from_rgb_f32(r as f32, g as f32, b as f32))
}

/// Push a new accent color into `Theme.accent`. Slint marshals the
/// closure onto the event loop; the closure runs synchronously
/// against the AppWindow's globals.
fn push_accent(app_weak: &slint::Weak<AppWindow>, color: slint::Color) {
    on_ui(app_weak, move |app| {
        app.global::<Theme>().set_accent(color);
    });
}

/// Subscribe to the user's DE accent color via xdg-desktop-portal's
/// `org.freedesktop.appearance.accent-color` Settings key. One-shot
/// initial read followed by a long-lived listener for
/// `SettingChanged`. Failure modes (portal absent, no user preference,
/// out-of-range RGB) are logged at debug level and we just keep the
/// theme's hardcoded Breeze blue default.
pub(crate) async fn watch_accent_color(app_weak: slint::Weak<AppWindow>) {
    use futures_util::StreamExt;

    let settings = match ashpd::desktop::settings::Settings::new().await {
        Ok(s) => s,
        Err(e) => {
            debug!(error = ?e, "xdg-desktop-portal Settings unavailable; keeping default accent");
            return;
        }
    };

    if let Ok(color) = settings.accent_color().await
        && let Some(slint_color) = ashpd_color_to_slint(color)
    {
        debug!(?slint_color, "initial accent color from portal");
        push_accent(&app_weak, slint_color);
    }

    let mut stream = match settings.receive_accent_color_changed().await {
        Ok(s) => s,
        Err(e) => {
            debug!(error = ?e, "couldn't subscribe to accent-color changes");
            return;
        }
    };
    while let Some(color) = stream.next().await {
        if let Some(slint_color) = ashpd_color_to_slint(color) {
            debug!(?slint_color, "accent color changed");
            push_accent(&app_weak, slint_color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ashpd_color_to_slint;

    #[test]
    fn portal_color_accepts_inclusive_rgb_range() {
        assert!(ashpd_color_to_slint(ashpd::desktop::Color::new(0.0, 0.5, 1.0)).is_some());
        assert!(ashpd_color_to_slint(ashpd::desktop::Color::new(1.0, 0.0, 0.0)).is_some());
    }

    #[test]
    fn portal_color_rejects_out_of_range_components() {
        for color in [
            ashpd::desktop::Color::new(-0.01, 0.5, 0.5),
            ashpd::desktop::Color::new(0.5, 1.01, 0.5),
            ashpd::desktop::Color::new(0.5, 0.5, -0.01),
        ] {
            assert!(ashpd_color_to_slint(color).is_none());
        }
    }
}
