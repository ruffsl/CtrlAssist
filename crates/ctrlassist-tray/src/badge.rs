//! The tray's icon with a state drawn into it: the desktop theme's own
//! pad, and at its lower right a glyph of the theme's in the state's
//! color, joined into one SVG file that the tray names as its icon.
//!
//! The tray protocol has an overlay for this, and Plasma 6.8 was seen to
//! draw nothing of one sent by name over a theme's icon, and to blur the
//! icon under one sent as pixels (KDE bug 520235). A file of the tray's
//! own is drawn by the desktop from its vector, as sharp as any icon.
//!
//! Nothing is rendered here. Each theme file is read by usvg, which
//! takes what a theme may ship (compressed files, colors said by a
//! stylesheet, by `currentColor` or outright) and writes it back as
//! plain shapes, the glyph in the state's color; what it wrote of the
//! two is then joined as text, each under a group with its own scale.

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

/// A theme's file as usvg wrote it back: what is inside its root
/// element, and the side of the square its drawing fills.
struct Drawing {
    body: String,
    side: f32,
}

/// Reads an icon file as a drawing, every shape of it restyled by
/// `style` where there is one, and the ids in it starting `prefix`, so
/// that two drawings joined into one file don't share any. `None` for
/// what usvg can't read, a bitmap among them, and for a drawing with
/// nothing in it.
fn drawing(file: &[u8], style: Option<String>, prefix: &str) -> Option<Drawing> {
    let options = usvg::Options {
        style_sheet: style,
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(file, &options).ok()?;
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

/// The pad with `badge`'s glyph at its lower right, as one SVG file:
/// `None` if either file isn't a drawing to join.
pub(crate) fn compose(pad: &[u8], glyph: &[u8], badge: Badge) -> Option<String> {
    let pad = drawing(pad, None, "pad-")?;
    // The glyph in the badge's color, whatever the theme drew it in: a
    // rule over every shape, and over a color the shape itself says.
    let color = badge.color();
    let tinted = drawing(
        glyph,
        Some(format!(
            "* {{ fill: {color} !important; color: {color} !important }}"
        )),
        "glyph-",
    )?;
    let scale = BADGE / tinted.side;
    // And once more, wider and dark, to go under it as its edge.
    let edged = drawing(
        glyph,
        Some(format!(
            "* {{ fill: {EDGE_COLOR} !important; color: {EDGE_COLOR} !important; \
             stroke: {EDGE_COLOR} !important; stroke-width: {width} !important; \
             stroke-linejoin: round !important }}",
            width = EDGE / scale,
        )),
        "edge-",
    )?;
    let at = SIDE - BADGE;
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
}

impl Icons {
    /// Composes an icon for each badge from the desktop theme's files
    /// into `dir`, which is made if it isn't there. A badge whose glyph
    /// the theme hasn't, or whose file can't be joined or written, is
    /// left out, and the tray shows the plain pad for it.
    pub(crate) fn make(dir: &Path) -> Self {
        let pad = ctrlassist_icons::resolve(menu::icon::TRAY, Variant::Color);
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
        let Some(pad) = pad.and_then(|path| std::fs::read(path).ok()) else {
            return icons;
        };
        if std::fs::create_dir_all(dir).is_err() {
            return icons;
        }
        for badge in Badge::ALL {
            let composed = glyph(badge)
                .and_then(|path| std::fs::read(path).ok())
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
                // The pad at twice its size, the glyph at 30 of 64.
                assert!(svg.contains(r#"<g transform="scale(2)">"#));
                assert!(svg.contains(r#"<g transform="translate(34,34) scale(1.875)">"#));
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

    /// What isn't an SVG drawing, or has nothing drawn in it, joins
    /// nothing.
    #[test]
    fn a_drawing_is_read_from_an_svg_file_or_not_at_all() {
        assert!(drawing(PAD, None, "").is_some_and(|read| read.side == 32.0));
        let sized = br#"<svg height="22px" width="22px" xmlns="http://www.w3.org/2000/svg"><path d="M0 0h4v4z"/></svg>"#;
        assert!(drawing(sized, None, "").is_some_and(|read| read.side == 22.0));
        for not in [
            &b""[..],
            b"\x89PNG\r\n\x1a\n",
            b"<svg/>",
            br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><path"#,
        ] {
            assert!(drawing(not, None, "").is_none(), "{not:?}");
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
