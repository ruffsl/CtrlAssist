//! The tray's icon with a state drawn into it: the desktop theme's own
//! pad, and at its lower right a glyph of the theme's in the state's
//! color, joined into one SVG file that the tray names as its icon.
//!
//! The tray protocol has an overlay for this, and Plasma 6.8 was seen to
//! draw nothing of one sent by name over a theme's icon, and to blur the
//! icon under one sent as pixels (KDE bug 520235). A file of the tray's
//! own is drawn by the desktop from its vector, as sharp as any icon.
//!
//! Nothing is rendered here. The two files are joined as text: each
//! one's contents under a group with its own scale, and the glyph's
//! color set on its group in place of the theme's stylesheet, which a
//! symbolic glyph takes its one color from.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use ctrlassist_icons::Variant;

use crate::menu::{self, Icon};

/// A state the tray's icon says with a badge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Badge {
    /// A setup runs: a green play.
    Running,
    /// A setup is held for a pad: a blue pause.
    Held,
    /// A setup failed: a red stop square.
    Failed,
    /// The daemon isn't running, or is another release: a gray
    /// disconnected plug.
    Unavailable,
}

impl Badge {
    pub(crate) const ALL: [Self; 4] = [Self::Running, Self::Held, Self::Failed, Self::Unavailable];

    /// The icon's name with this badge, as the tray names it.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Running => "ctrlassist-running",
            Self::Held => "ctrlassist-held",
            Self::Failed => "ctrlassist-failed",
            Self::Unavailable => "ctrlassist-unavailable",
        }
    }

    /// The theme's glyph for it, by the names a theme may have.
    const fn glyph(self) -> Icon {
        match self {
            Self::Running => &["media-playback-start"],
            Self::Held => &["media-playback-pause"],
            Self::Failed => &["media-playback-stop", "process-stop"],
            Self::Unavailable => &["network-disconnect", "network-offline", "dialog-cancel"],
        }
    }

    /// The color the glyph is drawn in.
    const fn color(self) -> &'static str {
        match self {
            Self::Running => "#2eb85c",
            Self::Held => "#3daee9",
            Self::Failed => "#da4453",
            Self::Unavailable => "#9a9ea3",
        }
    }

    const fn place(self) -> usize {
        match self {
            Self::Running => 0,
            Self::Held => 1,
            Self::Failed => 2,
            Self::Unavailable => 3,
        }
    }
}

/// The side of the composed icon, in its own units.
const SIDE: f32 = 64.0;
/// The side of the badge on it.
const BADGE: f32 = 30.0;
/// The width of the dark edge that keeps a badge apart from the pad
/// under it, and its color.
const EDGE: f32 = 2.4;
const EDGE_COLOR: &str = "#1b1e20";

/// An SVG file's contents apart from its root element, and the side of
/// the square its drawing fills.
struct Drawing<'a> {
    body: &'a str,
    side: f32,
}

/// The value of attribute `name` in a tag.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = tag;
    loop {
        let at = rest.find(name)?;
        let (before, after) = rest.split_at(at);
        let after = after.get(name.len()..)?;
        let whole = before.ends_with(|c: char| c.is_whitespace());
        if let (true, Some(value)) = (whole, after.trim_start().strip_prefix('=')) {
            let value = value.trim_start();
            let quote = value.chars().next().filter(|c| matches!(c, '"' | '\''))?;
            let value = value.get(1..)?;
            return value.get(..value.find(quote)?);
        }
        rest = after;
    }
}

/// Reads an SVG file as a drawing: what is inside its root element, and
/// its size from its view box, or its width where it has none. `None`
/// for anything else, a bitmap or a compressed file among them.
fn drawing(svg: &str) -> Option<Drawing<'_>> {
    let open = svg.find("<svg")?;
    let tag_end = open.checked_add(svg.get(open..)?.find('>')?)?;
    let tag = svg.get(open..tag_end)?;
    if tag.ends_with('/') {
        return None;
    }
    let body = svg.get(tag_end.checked_add(1)?..svg.rfind("</svg>")?)?;
    let side = attribute(tag, "viewBox")
        .and_then(|view| {
            view.split([' ', ','])
                .filter(|part| !part.is_empty())
                .nth(2)
        })
        .or_else(|| attribute(tag, "width"))
        .and_then(|side| side.trim_end_matches("px").parse::<f32>().ok())
        .filter(|side| side.is_finite() && *side > 0.0)?;
    Some(Drawing { body, side })
}

/// `body` without its `<style>` elements: a symbolic glyph's stylesheet
/// says the one color it is drawn in, which is set on its group instead.
fn without_styles(body: &str) -> String {
    let mut kept = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(at) = rest.find("<style") {
        let (before, style) = rest.split_at(at);
        kept.push_str(before);
        match style.find("</style>") {
            Some(end) => {
                rest = style
                    .get(end.saturating_add("</style>".len())..)
                    .unwrap_or("");
            }
            None => rest = "",
        }
    }
    kept.push_str(rest);
    kept
}

/// The pad with `badge`'s glyph at its lower right, as one SVG file:
/// `None` if either file isn't an SVG drawing to join.
pub(crate) fn compose(pad: &str, glyph: &str, badge: Badge) -> Option<String> {
    let pad = drawing(pad)?;
    let glyph = drawing(glyph)?;
    let glyph_body = without_styles(glyph.body);
    let scale = BADGE / glyph.side;
    let at = SIDE - BADGE;
    let color = badge.color();
    let mut svg = String::new();
    // Writing to a String can't fail.
    let _ = write!(
        svg,
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" "#,
            r#"viewBox="0 0 {side} {side}" width="{side}" height="{side}">"#,
            r#"<g transform="scale({pad_scale})">{pad}</g>"#,
            // The glyph twice: once wider and dark, for its edge, then
            // itself over that. One element with a stroke would do where
            // a renderer draws the stroke under the fill, and not every
            // one can be told to.
            r#"<g transform="translate({at},{at}) scale({scale})">"#,
            r#"<g color="{edge}" fill="{edge}" stroke="{edge}" stroke-width="{width}" stroke-linejoin="round">{glyph}</g>"#,
            r#"<g color="{color}" fill="{color}">{glyph}</g>"#,
            "</g></svg>"
        ),
        side = SIDE,
        pad_scale = SIDE / pad.side,
        pad = pad.body,
        at = at,
        scale = scale,
        edge = EDGE_COLOR,
        width = EDGE / scale,
        glyph = glyph_body,
        color = color,
    );
    Some(svg)
}

/// The composed icons, in a directory the tray names as its icon theme
/// path.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct Icons {
    dir: PathBuf,
    made: [bool; Badge::ALL.len()],
}

impl Icons {
    /// Composes an icon for each badge from the desktop theme's files
    /// into `dir`, which is made if it isn't there. A badge whose glyph
    /// the theme hasn't, or whose file can't be joined or written, is
    /// left out, and the tray shows the plain pad for it.
    pub(crate) fn make(dir: &Path) -> Self {
        let pad = ctrlassist_icons::resolve(menu::icon::PAD, Variant::Color);
        Self::from(dir, pad.as_deref(), |badge| {
            ctrlassist_icons::resolve(badge.glyph(), Variant::Symbolic)
        })
    }

    /// As [`make`](Self::make), from the pad's file and each glyph's.
    fn from(dir: &Path, pad: Option<&Path>, glyph: impl Fn(Badge) -> Option<PathBuf>) -> Self {
        let mut icons = Self {
            dir: dir.to_path_buf(),
            made: [false; Badge::ALL.len()],
        };
        let Some(pad) = pad.and_then(|path| std::fs::read_to_string(path).ok()) else {
            return icons;
        };
        if std::fs::create_dir_all(dir).is_err() {
            return icons;
        }
        for badge in Badge::ALL {
            let composed = glyph(badge)
                .and_then(|path| std::fs::read_to_string(path).ok())
                .and_then(|glyph| compose(&pad, &glyph, badge));
            let written = composed.is_some_and(|svg| {
                std::fs::write(dir.join(format!("{}.svg", badge.name())), svg).is_ok()
            });
            if let Some(made) = icons.made.get_mut(badge.place()) {
                *made = written;
            }
        }
        icons
    }

    /// Where the icons are, for the tray's icon theme path: empty if
    /// none was made.
    pub(crate) fn dir(&self) -> &Path {
        if self.made.contains(&true) {
            &self.dir
        } else {
            Path::new("")
        }
    }

    /// The icon's name with `badge`, if it was made.
    pub(crate) fn name(&self, badge: Badge) -> Option<&'static str> {
        self.made
            .get(badge.place())
            .copied()
            .unwrap_or(false)
            .then(|| badge.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAD: &str = r##"<svg viewBox="0 0 32 32" xmlns="http://www.w3.org/2000/svg"><rect id="pad" width="32" height="16" fill="#888"/></svg>"##;
    const GLYPH: &str = r##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
    <style type="text/css" id="current-color-scheme">.ColorScheme-Text { color: #fcfcfc; } </style>
    <path d="m2 2v12h4v-12z" class="ColorScheme-Text" fill="currentColor"/>
</svg>"##;

    /// The pad fills the icon, and the glyph is at its lower right in
    /// the badge's color over a dark edge, with no stylesheet left to
    /// say another.
    #[test]
    fn the_pad_and_the_glyph_are_joined_into_one_file() {
        let svg = compose(PAD, GLYPH, Badge::Held).unwrap();
        assert!(svg.starts_with("<svg "));
        assert!(svg.ends_with("</g></svg>"));
        assert!(svg.contains(r#"viewBox="0 0 64 64""#));
        // The pad at twice its size, whole.
        assert!(svg.contains(
            r##"<g transform="scale(2)"><rect id="pad" width="32" height="16" fill="#888"/></g>"##
        ));
        // The glyph at 30 of 64, from 34 across and down.
        assert!(svg.contains(r#"<g transform="translate(34,34) scale(1.875)">"#));
        assert!(svg.contains(r##"<g color="#3daee9" fill="#3daee9">"##));
        assert!(svg.contains(r##"stroke="#1b1e20" stroke-width="1.28"##));
        assert_eq!(svg.matches("m2 2v12h4v-12z").count(), 2);
        assert!(!svg.contains("<style"));
        assert!(!svg.contains("<?xml"));
        assert_eq!(svg.matches("<svg").count(), 1);
        // Each badge in its own color.
        for badge in Badge::ALL {
            let svg = compose(PAD, GLYPH, badge).unwrap();
            assert!(svg.contains(&format!(r#"<g color="{0}" fill="{0}">"#, badge.color())));
        }
    }

    /// A file's size is its view box's, or its width's, and what isn't
    /// an SVG drawing joins nothing.
    #[test]
    fn a_drawing_is_read_from_an_svg_file_or_not_at_all() {
        let sized = r#"<svg height="22px" width="22px" xmlns="x"><path d=""/></svg>"#;
        assert!(drawing(sized).is_some_and(|read| read.side == 22.0));
        let quoted = "<svg viewBox='0,0,24,24'><path/></svg>";
        assert!(drawing(quoted).is_some_and(|read| read.side == 24.0 && read.body == "<path/>"));
        for not in [
            "",
            "\u{89}PNG\r\n",
            "<svg/>",
            "<svg><path/></svg>",
            r#"<svg viewBox="0 0 0 0"><path/></svg>"#,
            r#"<svg viewBox="0 0 x x"><path/></svg>"#,
            r#"<svg viewBox="0 0 16 16"><path/>"#,
            r#"<svg data-viewBox="0 0 16 16"><path/></svg>"#,
        ] {
            assert!(drawing(not).is_none(), "{not}");
            assert!(compose(not, GLYPH, Badge::Running).is_none());
            assert!(compose(PAD, not, Badge::Running).is_none());
        }
        assert_eq!(
            without_styles("a<style>b</style>c<style x>d</style>e"),
            "ace"
        );
        assert_eq!(without_styles("a<style>b"), "a");
    }

    /// An icon is made for each badge whose glyph there is, in the
    /// directory, and only those are named.
    #[test]
    fn an_icon_is_made_for_each_badge_whose_glyph_there_is() {
        let dir = tempfile::tempdir().unwrap();
        let pad = dir.path().join("pad.svg");
        let glyph = dir.path().join("glyph.svg");
        let bitmap = dir.path().join("bitmap.png");
        std::fs::write(&pad, PAD).unwrap();
        std::fs::write(&glyph, GLYPH).unwrap();
        std::fs::write(&bitmap, b"\x89PNG").unwrap();
        let out = dir.path().join("made/icons");

        let icons = Icons::from(&out, Some(&pad), |badge| match badge {
            Badge::Running | Badge::Held => Some(glyph.clone()),
            Badge::Failed => Some(bitmap.clone()),
            Badge::Unavailable => None,
        });
        assert_eq!(icons.dir(), out);
        assert_eq!(icons.name(Badge::Running), Some("ctrlassist-running"));
        assert_eq!(icons.name(Badge::Held), Some("ctrlassist-held"));
        assert_eq!(icons.name(Badge::Failed), None);
        assert_eq!(icons.name(Badge::Unavailable), None);
        let held = std::fs::read_to_string(out.join("ctrlassist-held.svg")).unwrap();
        assert_eq!(held, compose(PAD, GLYPH, Badge::Held).unwrap());
        assert!(!out.join("ctrlassist-failed.svg").exists());

        // Without a pad to join to, or a directory to write in, none is
        // made, and no path is named.
        for none in [
            Icons::from(&out, None, |_| Some(glyph.clone())),
            Icons::from(&out, Some(&bitmap), |_| Some(glyph.clone())),
            Icons::from(&pad.join("under-a-file"), Some(&pad), |_| {
                Some(glyph.clone())
            }),
            Icons::default(),
        ] {
            assert_eq!(none.dir(), Path::new(""));
            assert!(Badge::ALL.iter().all(|badge| none.name(*badge).is_none()));
        }
    }

    /// From the desktop's own theme, whatever this machine has: each
    /// icon named is a file that was written.
    #[test]
    fn the_icons_made_from_the_theme_are_the_ones_named() {
        let dir = tempfile::tempdir().unwrap();
        let icons = Icons::make(dir.path());
        for badge in Badge::ALL {
            let file = dir.path().join(format!("{}.svg", badge.name()));
            assert_eq!(icons.name(badge).is_some(), file.exists(), "{badge:?}");
        }
    }
}
