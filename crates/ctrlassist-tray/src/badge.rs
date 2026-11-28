//! The tray's icon with a state drawn into it: the desktop theme's own
//! pad, and at its lower right a glyph of the theme's in the state's
//! color, joined into one SVG file that the tray names as its icon.
//!
//! The tray protocol has an overlay for this, and Plasma 6.8 was seen to
//! draw nothing of one sent by name over a theme's icon, and to blur the
//! icon under one sent as pixels (KDE bug 520235). A file of the tray's
//! own is drawn by the desktop from its vector, as sharp as any icon.
//!
//! The pad alone is written the same way, for when there is no state to
//! say. Named in the theme, it is the file a desktop picks for its
//! panel's size, and Breeze's small one is a line drawing where the one
//! the states are drawn into is in color.
//!
//! Nothing is rendered here. Each theme file is read by usvg, which
//! takes what a theme may ship (compressed files, colors said by a
//! stylesheet, by `currentColor` or outright) and writes it back as
//! plain shapes, each with its color on it. The glyph's are then set to
//! the state's, and what usvg wrote of the two is joined as text, each
//! under a group with its own scale.

use std::fmt::Write as _;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::{Path, PathBuf};

use ctrlassist_icons::Variant;
use tracing::debug;

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
    /// The daemon isn't running, or is another release: an amber
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
            Self::Unavailable => "#fdbc4b",
        }
    }

    /// The side of the badge on the icon, in the icon's units. The plug
    /// is a thin drawing, hard to read at the size the others are.
    const fn side(self) -> f32 {
        match self {
            Self::Running | Self::Held | Self::Failed => 30.0,
            Self::Unavailable => 45.0,
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
/// The width of the dark edge that keeps a badge apart from the pad
/// under it, and its color.
const EDGE: f32 = 2.4;
const EDGE_COLOR: &str = "#1b1e20";

/// The name of the pad alone, composed as the others are.
const PLAIN: &str = "ctrlassist-pad";

/// A theme's file as usvg wrote it back: what is inside its root
/// element, and the side of the square its drawing fills.
struct Drawing {
    body: String,
    side: f32,
}

/// Reads an icon file as a drawing, the ids in it starting `prefix`, so
/// that two drawings joined into one file don't share any. `None` for
/// what usvg can't read, a bitmap among them, and for a drawing with
/// nothing in it.
fn drawing(file: &[u8], prefix: &str) -> Option<Drawing> {
    let tree = usvg::Tree::from_data(file, &usvg::Options::default()).ok()?;
    let side = tree.size().width().max(tree.size().height());
    let written = tree.to_string(&usvg::WriteOptions {
        id_prefix: Some(prefix.to_string()),
        ..usvg::WriteOptions::default()
    });
    // What usvg writes is one root element around the shapes.
    let open = written.find("<svg")?;
    let tag_end = open.checked_add(written.get(open..)?.find('>')?)?;
    let body = written.get(tag_end.checked_add(1)?..written.rfind("</svg>")?)?;
    (side.is_finite() && side > 0.0 && !body.trim().is_empty()).then(|| Drawing {
        body: body.to_string(),
        side,
    })
}

/// The value of attribute `name` in a tag usvg wrote, and where it is.
fn value<'a>(tag: &'a str, name: &str) -> Option<(std::ops::Range<usize>, &'a str)> {
    let opening = format!(" {name}=\"");
    let start = tag.find(&opening)?.checked_add(opening.len())?;
    let end = start.checked_add(tag.get(start..)?.find('"')?)?;
    Some((start..end, tag.get(start..end)?))
}

/// A tag with every fill and stroke it has in `color`. What it leaves
/// unfilled stays so: a theme's glyph may carry a box of no fill around
/// it, which would come out a solid square. With `edge`, a shape that is
/// filled and has no stroke gets one that wide, for the dark edge under
/// a badge.
fn recolored_tag(tag: &str, color: &str, edge: Option<f32>) -> String {
    let mut tag = tag.to_string();
    let filled = value(&tag, "fill").is_some_and(|(_, fill)| fill != "none");
    if let (true, Some((place, _))) = (filled, value(&tag, "fill")) {
        tag.replace_range(place, color);
    }
    match (value(&tag, "stroke"), edge) {
        (Some((_, "none")), Some(width)) if filled => {
            tag = tag.replacen(
                " stroke=\"none\"",
                &format!(" stroke=\"{color}\" stroke-width=\"{width}\" stroke-linejoin=\"round\""),
                1,
            );
        }
        (Some((_, "none")) | None, _) => {}
        (Some((place, _)), _) => tag.replace_range(place, color),
    }
    tag
}

/// A drawing usvg wrote, with every fill and stroke in `color`, whatever
/// way the theme colored it: usvg has written each as a plain value on
/// its shape by then. What is inside a mask is left as it is, a mask's
/// colors being what it lets through and not what is seen.
fn recolored(body: &str, color: &str, edge: Option<f32>) -> String {
    let mut out = String::with_capacity(body.len().saturating_add(128));
    let mut rest = body;
    let mut masks = 0usize;
    while let Some(open) = rest.find('<') {
        let (before, tag_on) = rest.split_at(open);
        out.push_str(before);
        let Some(close) = tag_on.find('>') else {
            out.push_str(tag_on);
            return out;
        };
        let (tag, after) = tag_on.split_at(close.saturating_add(1));
        if tag.starts_with("</mask") {
            masks = masks.saturating_sub(1);
        }
        if masks == 0 && !tag.starts_with("</") {
            out.push_str(&recolored_tag(tag, color, edge));
        } else {
            out.push_str(tag);
        }
        if tag.starts_with("<mask") && !tag.ends_with("/>") {
            masks = masks.saturating_add(1);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

/// The pad alone at the composed icons' size, as one SVG file: `None`
/// if the file isn't a drawing.
fn alone(pad: &[u8]) -> Option<String> {
    let pad = drawing(pad, "pad-")?;
    Some(format!(
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" "#,
            r#"viewBox="0 0 {side} {side}" width="{side}" height="{side}">"#,
            r#"<g transform="scale({scale})">{pad}</g></svg>"#
        ),
        side = SIDE,
        scale = SIDE / pad.side,
        pad = pad.body,
    ))
}

/// The pad with `badge`'s glyph at its lower right, as one SVG file:
/// `None` if either file isn't a drawing to join.
pub(crate) fn compose(pad: &[u8], glyph: &[u8], badge: Badge) -> Option<String> {
    let pad = drawing(pad, "pad-")?;
    // The glyph twice, each with ids of its own: in the badge's color,
    // and under that once more, wider and dark, as its edge.
    let tinted = drawing(glyph, "glyph-")?;
    let under = drawing(glyph, "edge-")?;
    let scale = badge.side() / tinted.side;
    let tinted = Drawing {
        body: recolored(&tinted.body, badge.color(), None),
        side: tinted.side,
    };
    let edged = Drawing {
        body: recolored(&under.body, EDGE_COLOR, Some(EDGE / scale)),
        side: under.side,
    };
    let at = SIDE - badge.side();
    let mut svg = String::new();
    // Writing to a String can't fail.
    let _ = write!(
        svg,
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" "#,
            r#"viewBox="0 0 {side} {side}" width="{side}" height="{side}">"#,
            r#"<g transform="scale({pad_scale})">{pad}</g>"#,
            r#"<g transform="translate({at},{at}) scale({scale})">{edge}{glyph}</g>"#,
            "</svg>"
        ),
        side = SIDE,
        pad_scale = SIDE / pad.side,
        pad = pad.body,
        at = at,
        scale = scale,
        edge = edged.body,
        glyph = tinted.body,
    );
    Some(svg)
}

/// The composed icons, in a directory the tray names as its icon theme
/// path.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct Icons {
    dir: PathBuf,
    made: [bool; Badge::ALL.len()],
    plain: bool,
}

impl Icons {
    /// Composes an icon for each badge from the desktop theme's files
    /// into `dir`, which is made if it isn't there, for this user alone.
    /// A badge whose glyph the theme hasn't, or whose file can't be
    /// joined or written, is left out, and the tray shows the plain pad
    /// for it.
    ///
    /// A theme with the pad in symbolic form alone gets none: a desktop
    /// draws that in its panel's own color, light on a dark panel, and
    /// joined into a file here it would keep the one color its theme
    /// wrote it in, dark on that panel beside a bright badge.
    pub(crate) fn make(dir: &Path) -> Self {
        let pad = ctrlassist_icons::resolve(menu::icon::PAD, Variant::Color).filter(|path| {
            let symbolic = path
                .file_stem()
                .is_some_and(|stem| stem.to_string_lossy().ends_with("-symbolic"));
            if symbolic {
                debug!(
                    pad = %path.display(),
                    "the theme has the pad in symbolic form alone, so the tray's icon is the plain pad in every state"
                );
            }
            !symbolic
        });
        Self::from(dir, pad.as_deref(), |badge| {
            ctrlassist_icons::resolve(badge.glyph(), Variant::Symbolic)
        })
    }

    /// As [`make`](Self::make), from the pad's file and each glyph's.
    fn from(dir: &Path, pad: Option<&Path>, glyph: impl Fn(Badge) -> Option<PathBuf>) -> Self {
        // Whole, since a desktop reads the path from where it is itself.
        let dir = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
        let mut icons = Self {
            dir,
            made: [false; Badge::ALL.len()],
            plain: false,
        };
        let Some(pad) = pad.and_then(|path| std::fs::read(path).ok()) else {
            debug!(
                "the theme has no pad to draw a state into, so the tray's icon is the plain pad"
            );
            return icons;
        };
        // The user's alone, as the daemon makes the directory above it,
        // which this may be the first to make.
        let made = std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&icons.dir);
        if let Err(error) = made {
            debug!(dir = %icons.dir.display(), %error, "the tray's icons have nowhere to be kept");
            return icons;
        }
        // Written beside its name and moved onto it, so that a desktop
        // never reads half a file.
        let write = |name: &str, svg: Option<String>| {
            let file = icons.dir.join(format!("{name}.svg"));
            let draft = icons.dir.join(format!("{name}.svg.new"));
            svg.is_some_and(|svg| {
                std::fs::write(&draft, svg).is_ok() && std::fs::rename(&draft, &file).is_ok()
            })
        };
        // The pad alone too, from the same file: named in the theme, a
        // desktop picks the file for its panel's size, which in Breeze
        // is a line drawing where the one the states are drawn into is
        // in color, and the icon would change its looks as a setup
        // starts.
        let plain = write(PLAIN, alone(&pad));
        if !plain {
            debug!("the pad alone couldn't be made, and is the theme's own");
        }
        for badge in Badge::ALL {
            let composed = glyph(badge)
                .and_then(|path| std::fs::read(path).ok())
                .and_then(|glyph| compose(&pad, &glyph, badge));
            let written = write(badge.name(), composed);
            if !written {
                debug!(
                    badge = badge.name(),
                    "this state's icon couldn't be made, and is the plain pad"
                );
            }
            if let Some(made) = icons.made.get_mut(badge.place()) {
                *made = written;
            }
        }
        icons.plain = plain;
        icons
    }

    /// Where the icons are, for the tray's icon theme path: empty if
    /// none was made.
    pub(crate) fn dir(&self) -> &Path {
        if self.plain || self.made.contains(&true) {
            &self.dir
        } else {
            Path::new("")
        }
    }

    /// The name of the pad alone, if it was made.
    pub(crate) fn plain(&self) -> Option<&'static str> {
        self.plain.then_some(PLAIN)
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

    const PAD: &[u8] = br##"<svg viewBox="0 0 32 32" xmlns="http://www.w3.org/2000/svg"><rect width="32" height="16" fill="#888"/></svg>"##;
    /// A glyph colored as Breeze colors one: by a stylesheet's class.
    const GLYPH: &[u8] = br##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
    <style type="text/css" id="current-color-scheme">.ColorScheme-Text { color: #fcfcfc; } </style>
    <path d="m2 2v12h4v-12z" class="ColorScheme-Text" fill="currentColor"/>
</svg>"##;
    /// One colored as Adwaita colors one, outright, and one with a
    /// color in a shape's own style, as Breeze has too.
    const OUTRIGHT: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16px" height="16px" viewBox="0 0 16 16"><path fill="#2e3436" d="M2 2v12l12-6z"/></svg>"##;
    const STYLED: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><style>.Bad { color: #da4453; }</style><path style="fill:currentColor" class="Bad" d="M2 2v12l12-6z"/></svg>"##;

    /// The fills and strokes a file names, apart from `none`.
    fn colors(svg: &str) -> Vec<&str> {
        let mut found: Vec<&str> = ["fill=\"", "stroke=\""]
            .into_iter()
            .flat_map(|attribute| {
                svg.match_indices(attribute).filter_map(move |(at, _)| {
                    let value = svg.get(at + attribute.len()..)?;
                    value.get(..value.find('"')?)
                })
            })
            .filter(|color| *color != "none")
            .collect();
        found.sort_unstable();
        found.dedup();
        found
    }

    /// The pad fills the icon in its own colors, and the glyph is at
    /// its lower right in the badge's color over a dark edge, however
    /// the theme colored it. What is written is an SVG file again.
    #[test]
    fn the_pad_and_the_glyph_are_joined_into_one_file() {
        for glyph in [GLYPH, OUTRIGHT, STYLED] {
            for badge in Badge::ALL {
                let svg = compose(PAD, glyph, badge).unwrap();
                assert!(svg.starts_with("<svg "));
                assert!(svg.ends_with("</g></svg>"));
                assert_eq!(svg.matches("<svg").count(), 1);
                assert!(!svg.contains("<style"));
                assert!(svg.contains(r#"viewBox="0 0 64 64""#));
                // The pad at twice its size, the glyph at 30 of 64, and
                // the plug at 45.
                assert!(svg.contains(r#"<g transform="scale(2)">"#));
                let placed = if badge == Badge::Unavailable {
                    r#"<g transform="translate(19,19) scale(2.8125)">"#
                } else {
                    r#"<g transform="translate(34,34) scale(1.875)">"#
                };
                assert!(svg.contains(placed));
                // The pad's gray, the edge's dark and the badge's color,
                // and none of what the theme had the glyph in.
                assert_eq!(
                    colors(&svg),
                    {
                        let mut expected = vec!["#888888", EDGE_COLOR, badge.color()];
                        expected.sort_unstable();
                        expected
                    },
                    "{svg}"
                );
                assert!(svg.contains(&format!(r#"stroke="{EDGE_COLOR}""#)));
                // Read back, it is a drawing of the icon's size.
                let read = usvg::Tree::from_data(svg.as_bytes(), &usvg::Options::default());
                assert!(read.is_ok_and(|tree| tree.size().width() == SIDE));
            }
        }
    }

    /// A glyph's unfilled box stays unfilled, its strokes take the
    /// badge's color, what is in a mask is left alone, and ids two files
    /// share are told apart in the one they are joined into.
    #[test]
    fn a_glyph_is_recolored_shape_by_shape_and_ids_are_kept_apart() {
        const BOXED: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><rect width="16" height="16" fill="none"/><path fill="#2e3436" d="M2 2v12l12-6z"/><path fill="none" stroke="#2e3436" stroke-width="2" d="M1 15h14"/></svg>"##;
        let svg = compose(PAD, BOXED, Badge::Running).unwrap();
        // The pad's gray, the edge's dark and the badge's green: the
        // theme's own color is gone from fills and strokes alike.
        let mut expected = vec!["#888888", EDGE_COLOR, Badge::Running.color()];
        expected.sort_unstable();
        assert_eq!(colors(&svg), expected, "{svg}");
        // The box and the line are unfilled still, in each of the
        // glyph's two copies, and one shape of each copy is filled.
        assert_eq!(svg.matches(r#"fill="none""#).count(), 4, "{svg}");
        let green = format!(r#"fill="{}""#, Badge::Running.color());
        assert_eq!(svg.matches(green.as_str()).count(), 1, "{svg}");

        let masked = recolored(
            r##"<mask id="m"><path fill="#ffffff" d="M0 0"/></mask><path fill="#222222" stroke="none" d="M1 1"/>"##,
            "#00ff00",
            None,
        );
        assert_eq!(
            masked,
            r##"<mask id="m"><path fill="#ffffff" d="M0 0"/></mask><path fill="#00ff00" stroke="none" d="M1 1"/>"##
        );
        let edged = recolored(
            r##"<path fill="#222222" stroke="none" d="M1 1"/>"##,
            "#000000",
            Some(1.5),
        );
        assert_eq!(
            edged,
            r##"<path fill="#000000" stroke="#000000" stroke-width="1.5" stroke-linejoin="round" d="M1 1"/>"##
        );

        // A pad and a glyph that each name a gradient `a`.
        const SHADED: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><linearGradient id="a"><stop offset="0" stop-color="#111"/><stop offset="1" stop-color="#eee"/></linearGradient><path fill="url(#a)" d="M0 0h16v16H0z"/></svg>"##;
        let svg = compose(SHADED, SHADED, Badge::Held).unwrap();
        let mut ids: Vec<&str> = svg
            .match_indices(" id=\"")
            .filter_map(|(at, opening)| {
                let id = svg.get(at + opening.len()..)?;
                id.get(..id.find('"')?)
            })
            .collect();
        assert!(ids.iter().any(|id| id.starts_with("pad-")), "{ids:?}");
        let all = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), all, "an id is used twice: {svg}");
        assert!(svg.contains("url(#pad-"), "{svg}");
    }

    /// What isn't an SVG drawing, or has nothing drawn in it, joins
    /// nothing.
    #[test]
    fn a_drawing_is_read_from_an_svg_file_or_not_at_all() {
        assert!(drawing(PAD, "").is_some_and(|read| read.side == 32.0));
        let sized = br#"<svg height="22px" width="22px" xmlns="http://www.w3.org/2000/svg"><path d="M0 0h4v4z"/></svg>"#;
        assert!(drawing(sized, "").is_some_and(|read| read.side == 22.0));
        for not in [
            &b""[..],
            b"\x89PNG\r\n\x1a\n",
            b"<svg/>",
            br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path"#,
        ] {
            assert!(drawing(not, "").is_none(), "{not:?}");
            assert!(compose(not, GLYPH, Badge::Running).is_none());
            assert!(compose(PAD, not, Badge::Running).is_none());
        }
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
        // The pad alone is made beside them, at the same size.
        assert_eq!(icons.plain(), Some("ctrlassist-pad"));
        let plain = std::fs::read_to_string(out.join("ctrlassist-pad.svg")).unwrap();
        assert_eq!(plain, alone(PAD).unwrap());
        assert!(plain.contains(r#"viewBox="0 0 64 64""#));
        assert!(plain.contains(r#"<g transform="scale(2)">"#));
        assert!(held.starts_with(plain.trim_end_matches("</svg>")));

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
            assert_eq!(none.plain(), None);
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
