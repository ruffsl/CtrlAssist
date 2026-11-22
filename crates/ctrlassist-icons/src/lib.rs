//! Freedesktop icon-theme resolution shared by the GUI (which loads the
//! resolved path into a Slint image) and the tray (which hands the
//! resolved name to its desktop tray host to load and recolor).
//!
//! Resolution walks the active desktop's chosen theme first — KDE via
//! `~/.config/kdeglobals`, GNOME/GTK via `gsettings` — then Breeze /
//! Adwaita / hicolor as fallbacks, covering KDE Plasma + GNOME (Ubuntu's
//! Yaru, Fedora's Adwaita) without linking dconf. For each requested
//! name a preferred [`Variant`] is tried before the other form, so a
//! symbolic-only theme still resolves and color-carrying icons keep
//! their color where the theme ships it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use tracing::debug;

/// Which form of an icon name to try first.
#[derive(Clone, Copy, Debug)]
pub enum Variant {
    /// Prefer the monochrome `-symbolic` form (chrome, action glyphs).
    Symbolic,
    /// Prefer the plain colored form (identity, status).
    Color,
}

/// Resolve a request to the first icon file that exists across the theme
/// cascade — for a consumer that loads the file itself (the GUI loads it
/// into a Slint image). Returns `None` when no theme ships any candidate.
/// See `find` for the shared cascade walk.
pub fn resolve(names: &[&str], prefer: Variant) -> Option<PathBuf> {
    find(names, prefer).map(|(_, path)| path)
}

/// Resolve a request to the icon *name* that exists in the active theme,
/// rather than its path — for handing to a desktop component that
/// resolves and recolors the name itself (a tray host loading it from
/// its own theme). This keeps `-symbolic` glyphs recolorable to the
/// panel foreground, which a baked pixmap could not. Returns `None` when
/// no theme ships any candidate.
pub fn resolve_name(names: &[&str], prefer: Variant) -> Option<String> {
    find(names, prefer).map(|(name, _)| name)
}

/// The shared cascade walk behind [`resolve`] / [`resolve_name`]: return
/// the first `(name, path)` that exists, trying the preferred [`Variant`]
/// of every requested name before the other variant of any — so an
/// explicit fallback name never loses to a synthesized variant of an
/// earlier name.
///
/// The whole request is memoized — misses included. The resolver's own
/// cache only remembers *found* paths, so a request whose leading
/// candidate exists in no theme re-walks every theme directory on every
/// call (tens of milliseconds against a large icon installation), which
/// a per-state-sample render loop like the tray's cannot afford. The
/// theme list is frozen per process, so a memoized result never goes
/// stale within a run.
fn find(names: &[&str], prefer: Variant) -> Option<(String, PathBuf)> {
    type Memo = Mutex<HashMap<(Vec<String>, bool), Option<(String, PathBuf)>>>;
    static MEMO: OnceLock<Memo> = OnceLock::new();
    let key = (
        names.iter().map(ToString::to_string).collect(),
        matches!(prefer, Variant::Symbolic),
    );
    let memo = MEMO.get_or_init(Memo::default);
    // A poisoned memo is still a valid cache: the panicking holder could
    // only have been mid-lookup or mid-insert of one entry.
    if let Some(hit) = memo
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&key)
    {
        return hit.clone();
    }
    let result = walk(names, prefer);
    memo.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(key, result.clone());
    result
}

/// One uncached cascade walk for [`find`].
///
/// Size 22 is requested because Breeze's "actions" / "devices"
/// categories ship 16/22/24/32/64 buckets unevenly — 22 is the most
/// universally populated; scalable themes return their SVG regardless.
fn walk(names: &[&str], prefer: Variant) -> Option<(String, PathBuf)> {
    let themes = icon_themes();
    for candidate in icon_candidates(names, prefer) {
        for theme in themes {
            if let Some(path) = freedesktop_icons::lookup(&candidate)
                .with_size(22)
                .with_theme(theme)
                .with_cache()
                .find()
            {
                debug!(icon = candidate, theme, path = %path.display(), "icon resolved");
                return Some((candidate, path));
            }
        }
    }
    None
}

/// The ordered, deduplicated lookup names for a request: the preferred
/// variant of every requested name first, then the other variant of
/// every name. Dedup preserves order and drops the redundant candidate a
/// name like `foo-symbolic` produces.
fn icon_candidates(names: &[&str], prefer: Variant) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::with_capacity(names.len() * 2);
    let want_symbolic = matches!(prefer, Variant::Symbolic);
    for preferred_pass in [true, false] {
        for name in names {
            let (symbolic, color) = name_variants(name);
            let candidate = if want_symbolic == preferred_pass {
                symbolic
            } else {
                color
            };
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    candidates
}

/// Split a requested name into its `(symbolic, color)` forms. A name
/// already ending in `-symbolic` yields itself plus the stripped base;
/// anything else yields the `-symbolic` suffixed form plus itself.
fn name_variants(name: &str) -> (String, String) {
    match name.strip_suffix("-symbolic") {
        Some(base) => (name.to_string(), base.to_string()),
        None => (format!("{name}-symbolic"), name.to_string()),
    }
}

/// The ordered icon-theme search list, resolved once. Leads with the
/// active desktop's chosen theme(s) so lookups land in the theme the
/// user actually sees, then the hardcoded fallbacks. The `freedesktop`
/// resolver follows each theme's `Inherits` chain, so a single detected
/// theme (e.g. `Yaru-dark`) already cascades through its parents.
fn icon_themes() -> &'static [String] {
    static THEMES: OnceLock<Vec<String>> = OnceLock::new();
    THEMES.get_or_init(|| {
        let mut themes: Vec<String> = Vec::new();
        let mut push = |t: String| {
            if !t.is_empty() && !themes.contains(&t) {
                themes.push(t);
            }
        };
        if let Some(t) = detect_kde_theme() {
            push(t);
        }
        if let Some(t) = detect_gnome_theme() {
            push(t);
        }
        for fallback in ["breeze", "Adwaita", "hicolor"] {
            push(fallback.to_string());
        }
        debug!(?themes, "icon theme search order");
        themes
    })
}

/// KDE writes its icon-theme choice to `kdeglobals` `[Icons] Theme=...`
/// in the user's config dir (`$XDG_CONFIG_HOME`, else `~/.config`), over
/// the distro default in `/etc/xdg`. Returns `None` when neither file
/// sets it.
fn detect_kde_theme() -> Option<String> {
    let user_config = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    user_config
        .into_iter()
        .chain([PathBuf::from("/etc/xdg")])
        .find_map(|dir| kde_icon_theme(&dir.join("kdeglobals")))
}

/// The `[Icons] Theme=` value in the kdeglobals file at `path`.
fn kde_icon_theme(path: &std::path::Path) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut in_icons_section = false;
    for line in content.lines() {
        let line = line.trim();
        if line == "[Icons]" {
            in_icons_section = true;
        } else if line.starts_with('[') {
            in_icons_section = false;
        } else if in_icons_section && let Some(value) = line.strip_prefix("Theme=") {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// GNOME/GTK keep the icon theme in gsettings (dconf-backed). Shell out
/// to the `gsettings` CLI rather than linking glib — best-effort, so a
/// missing binary or non-GNOME session just yields `None` and we fall
/// through the hardcoded list.
fn detect_gnome_theme() -> Option<String> {
    let output = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8(output.stdout).ok()?;
    // gsettings prints the GVariant-quoted value, e.g. `'Yaru-dark'\n`.
    let theme = raw.trim().trim_matches(['\'', '"']).trim();
    (!theme.is_empty()).then(|| theme.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_variants_splits_symbolic_and_color() {
        assert_eq!(
            name_variants("edit-copy"),
            ("edit-copy-symbolic".into(), "edit-copy".into())
        );
        assert_eq!(
            name_variants("help-contextual-symbolic"),
            ("help-contextual-symbolic".into(), "help-contextual".into())
        );
    }

    #[test]
    fn symbolic_first_prefers_symbolic_across_all_names() {
        // Every name's symbolic form is tried before any plain form, so
        // a theme that only ships symbolics resolves the first name.
        assert_eq!(
            icon_candidates(&["document-edit", "edit"], Variant::Symbolic),
            [
                "document-edit-symbolic",
                "edit-symbolic",
                "document-edit",
                "edit"
            ]
        );
    }

    #[test]
    fn color_first_prefers_plain_across_all_names() {
        // The colored form of a later fallback (video-display) wins over
        // a synthesized symbolic of an earlier name (video-television).
        assert_eq!(
            icon_candidates(&["video-television", "video-display"], Variant::Color),
            [
                "video-television",
                "video-display",
                "video-television-symbolic",
                "video-display-symbolic",
            ]
        );
    }

    #[test]
    fn already_symbolic_name_does_not_double_suffix() {
        // A `-symbolic` name yields itself (once) plus its stripped base;
        // no `foo-symbolic-symbolic`, and dedup keeps each form once.
        assert_eq!(
            icon_candidates(
                &["network-connect-symbolic", "network-connect"],
                Variant::Symbolic
            ),
            ["network-connect-symbolic", "network-connect"]
        );
    }
}
