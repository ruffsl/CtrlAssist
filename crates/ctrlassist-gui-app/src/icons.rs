//! The window's icons, resolved against the user's freedesktop icon
//! theme by name, with fallbacks, so the window looks at home under
//! Breeze, Adwaita, Yaru and the rest without shipping glyphs of its
//! own. A name no theme has leaves an empty image, which Slint draws as
//! nothing: the label beside it still shows.

use slint::ComponentHandle;

use crate::icon_theme::{load_color_icon, load_symbolic_icon};
use crate::{AppWindow, Clipboard, Icons};

/// Loads the sidebar's icons and the action glyphs the panes share.
pub(crate) fn load(app: &AppWindow) {
    // `input-gaming` is CtrlAssist's own icon, the tray's too.
    app.set_pads_icon(load_symbolic_icon(&[
        "input-gaming",
        "input-gamepad",
        "applications-games",
    ]));
    app.set_setups_icon(load_symbolic_icon(&["media-playback-start", "system-run"]));
    app.set_profiles_icon(load_symbolic_icon(&[
        "view-list-details",
        "document-properties",
        "text-x-generic",
    ]));
    app.set_settings_icon(load_symbolic_icon(&[
        "configure",
        "settings-configure",
        "preferences-system",
    ]));
    app.set_about_icon(load_symbolic_icon(&["help-about", "dialog-information"]));

    // In color, for the About pane's header: the icon the desktop entry
    // and the tray carry.
    app.set_app_logo(load_color_icon(&["input-gaming", "applications-games"]));

    let icons = app.global::<Icons>();
    icons.set_shutdown(load_symbolic_icon(&["system-shutdown"]));
    icons.set_edit(load_symbolic_icon(&["document-edit", "edit"]));
    // `dialog-ok-apply` is the Breeze and Adwaita name for the check
    // mark; `window-close` is the cross most themes agree on, where
    // `dialog-cancel` is sometimes a colored stop sign.
    icons.set_save(load_symbolic_icon(&["dialog-ok-apply", "dialog-ok"]));
    icons.set_cancel(load_symbolic_icon(&["window-close", "dialog-cancel"]));
    icons.set_folder_open(load_symbolic_icon(&["document-open-folder", "folder-open"]));
    // The same icon the tray would use for the same action.
    icons.set_reload(load_symbolic_icon(&["view-refresh"]));
    icons.set_copy(load_symbolic_icon(&["edit-copy"]));
    icons.set_delete(load_symbolic_icon(&["edit-delete"]));
    // The name KDE's own help button uses first.
    icons.set_info(load_symbolic_icon(&[
        "help-contextual",
        "documentinfo",
        "dialog-information",
    ]));
    icons.set_open_link(load_symbolic_icon(&[
        "open-link",
        "external-link",
        "emblem-symbolic-link",
        "internet-web-browser",
        "applications-internet",
    ]));
    app.global::<Clipboard>()
        .set_copy_icon(load_symbolic_icon(&["edit-copy"]));
}
