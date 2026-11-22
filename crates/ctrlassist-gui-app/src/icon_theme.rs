//! Slint glue over the shared [`ctrlassist_icons`] resolver: resolve a
//! freedesktop icon name (active-desktop theme + symbolic/color cascade)
//! to a file path, then load it as a Slint `Image`.

use ctrlassist_icons::Variant;
use tracing::warn;

/// Load an action/chrome glyph, preferring the monochrome `-symbolic`
/// form (see [`ctrlassist_icons`]). Use [`load_color_icon`] for the handful
/// of icons whose color carries meaning (app identity).
pub(crate) fn load_symbolic_icon(names: &[&str]) -> slint::Image {
    load(names, Variant::Symbolic)
}

/// Load an icon preferring its plain colored form, falling back to
/// `-symbolic` only when no theme ships the color one. For icons whose
/// color is the point — the app-identity logo — where a monochrome
/// fallback would flatten meaning.
pub(crate) fn load_color_icon(names: &[&str]) -> slint::Image {
    load(names, Variant::Color)
}

/// Resolve then load. Returns the default empty image on total miss or a
/// Slint decode failure; Slint draws nothing for an empty source so the
/// layout degrades gracefully to label-only.
fn load(names: &[&str], prefer: Variant) -> slint::Image {
    let Some(path) = ctrlassist_icons::resolve(names, prefer) else {
        warn!(?names, ?prefer, "no icon resolved across theme cascade");
        return slint::Image::default();
    };
    match slint::Image::load_from_path(&path) {
        Ok(img) => img,
        Err(e) => {
            warn!(?names, path = %path.display(), error = ?e, "Slint couldn't load icon");
            slint::Image::default()
        }
    }
}
