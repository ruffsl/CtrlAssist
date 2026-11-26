//! The Settings pane's tray side: the tray's notification switches, as
//! the SDK's tray client mirrors them, and the pane's two requests of
//! the tray. Couch mode, the pane's other group, is wired with the rest
//! of couch mode.

use ctrlassist::{ErrorCode, NotificationSettings, NotificationSettingsPatch, TrayClient};
use slint::ComponentHandle;
use tracing::warn;

use crate::toast::show_toast;
use crate::{AppWindow, Tray, TrayNotificationSettings};

/// The switches as the pane shows them.
#[must_use]
pub fn shown(settings: NotificationSettings) -> TrayNotificationSettings {
    TrayNotificationSettings {
        setup_started: settings.setup_started,
        setup_trouble: settings.setup_trouble,
        pad_connection: settings.pad_connection,
        refused: settings.refused,
    }
}

/// The change one switch of the pane asks for, by the key it passes:
/// the tray's own for that switch. Nothing for a key that isn't one.
#[must_use]
pub fn patch(key: &str, enabled: bool) -> Option<NotificationSettingsPatch> {
    let mut patch = NotificationSettingsPatch::default();
    let switch = match key {
        "setup_started" => &mut patch.setup_started,
        "setup_trouble" => &mut patch.setup_trouble,
        "pad_connection" => &mut patch.pad_connection,
        "refused" => &mut patch.refused,
        _ => return None,
    };
    *switch = Some(enabled);
    Some(patch)
}

/// What the toast says of a switch the tray didn't set. A settings file
/// it couldn't write is said plainly, with where the file is and why,
/// which the tray's refusal holds; anything else as the tray said it.
#[must_use]
pub fn not_set(error: &ctrlassist::Error) -> String {
    match error {
        ctrlassist::Error::Rpc {
            code: ErrorCode::SettingsUnwritable,
            message,
        } => format!(
            "The tray couldn't write its settings file, so the switch is as it was: {message}"
        ),
        _ => format!("The tray didn't take that: {error}"),
    }
}

/// Binds the pane's requests to the tray's rpcs. A switch only asks:
/// what it shows is what the tray publishes, which comes back through
/// the state loop, so nothing is set here ahead of the tray's own word.
/// A request that fails is said in a toast, and the switch, which never
/// moved, still shows the tray's setting.
pub(crate) fn wire(app: &AppWindow, tray: Option<&TrayClient>) {
    let Some(tray) = tray else {
        return;
    };
    let global = app.global::<Tray>();
    let (weak, client) = (app.as_weak(), tray.clone());
    global.on_set_notification(move |key, enabled| {
        let Some(patch) = patch(key.as_str(), enabled) else {
            warn!(key = key.as_str(), "the Settings pane named no switch");
            return;
        };
        let (weak, client) = (weak.clone(), client.clone());
        tokio::spawn(async move {
            if let Err(error) = client.update_notification_settings(patch).await {
                warn!(%error, "a notification switch wasn't set");
                show_toast(weak, &not_set(&error));
            }
        });
    });
    let (weak, client) = (app.as_weak(), tray.clone());
    global.on_send_test_notification(move || {
        let (weak, client) = (weak.clone(), client.clone());
        tokio::spawn(async move {
            match client.send_test_notification().await {
                Ok(()) => show_toast(weak, "The tray was asked to show one"),
                Err(error) => {
                    warn!(%error, "the test notification wasn't sent");
                    show_toast(weak, &format!("The tray didn't show one: {error}"));
                }
            }
        });
    });
}
