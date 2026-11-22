//! What the tray's icon says when hovered: each setup with its mode, its
//! pads and its virtual pads, the profiles that could start, and with
//! nothing running, how many pads are connected. It marks the profile a
//! middle click on the icon starts or stops, which the wheel moves.
//! Plain text from a view, so it is tested without a desktop.

use ctrlassist::form::{self, Kind, PRESET};
use ctrlassist::{Pad, Phase, Profile, Setup};
use ctrlassist_protocol::profile::{PadRef, Profile as Checked};

use crate::menu::{self, capitalized, virtual_name};
use crate::state::View;

/// The mark before the profile a middle click acts on, as TisVlay's
/// tray marks what its wheel acts on; a profile without it is indented
/// as far.
const TARGET: &str = "►";

/// The mark before the pad that has control.
const HAS_CONTROL: &str = "●";

/// The most of a pad's name a line shows, in characters: a desktop
/// wraps a tooltip's long lines, which breaks the list's indenting.
const NAME_MAX: usize = 28;

/// A pad's name cut to fit a line, with "…" for what was cut.
fn short(name: &str) -> String {
    if name.chars().count() <= NAME_MAX {
        return name.to_string();
    }
    let kept: String = name.chars().take(NAME_MAX - 1).collect();
    format!("{}…", kept.trim_end())
}

fn marked(name: &str, target: Option<&str>) -> &'static str {
    if target == Some(name) { TARGET } else { " " }
}

/// The label of the mode `profile` is written in; nothing for one
/// written by hand, or before the daemon has said what the modes are.
fn mode(view: &View, profile: &Profile) -> Option<String> {
    let values = form::from_toml(&view.fields, &profile.toml)?;
    let value = values.get(PRESET)?;
    let field = view.fields.iter().find(|field| field.key == PRESET)?;
    let Kind::Choice { choices } = &field.kind else {
        return None;
    };
    choices
        .iter()
        .find(|choice| choice.value == *value)
        .map(|choice| choice.label.clone())
}

/// A setup's lines: its name, phase and mode, then why it failed, or
/// its pads and its virtual pads.
fn setup_lines(view: &View, setup: &Setup, target: Option<&str>, lines: &mut Vec<String>) {
    let (phase, _) = menu::phase_label(setup.phase);
    let mode = view
        .profiles
        .iter()
        .find(|profile| profile.name == setup.name)
        .and_then(|profile| mode(view, profile))
        .map(|mode| format!(" · {mode}"))
        .unwrap_or_default();
    lines.push(format!(
        "{} {}: {phase}{mode}",
        marked(&setup.name, target),
        setup.name
    ));
    if let Some(reason) = &setup.reason {
        lines.push(format!("    {reason}"));
        return;
    }
    for source in &setup.sources {
        // The pad a node of the setup has selected has control.
        let selected = setup
            .nodes
            .iter()
            .any(|node| node.choices.get(node.selected) == Some(&source.name));
        let mark = if selected && setup.phase == Phase::Running {
            HAS_CONTROL
        } else {
            " "
        };
        let pad = match (&source.pad, source.present) {
            (Some(pad), true) => short(view.pad_name(pad)),
            (Some(_), false) => "disconnected".to_string(),
            (None, _) => "waiting for a pad that fits".to_string(),
        };
        lines.push(format!("    {mark} {}: {pad}", capitalized(&source.name)));
    }
    for sink in &setup.sinks {
        let name = match sink.node {
            Some(_) => short(&virtual_name(view, &sink.identity)),
            None => "not made".to_string(),
        };
        lines.push(format!("      {}: {name}", capitalized(&sink.name)));
    }
}

/// Whether `pad` is one `wanted` means.
fn names(wanted: &PadRef, pad: &Pad) -> bool {
    match wanted {
        PadRef::Exact(id) => *id == pad.id,
        PadRef::Model(vendor, product) => (*vendor, *product) == (pad.vendor, pad.product),
    }
}

/// The kept profiles that could start now, as the window's "Ready": not
/// running, nothing wrong, and every pad they name connected and in no
/// setup.
#[must_use]
pub fn ready(view: &View) -> Vec<&str> {
    view.profiles
        .iter()
        .filter(|profile| profile.problem.is_none() && view.setup(&profile.name).is_none())
        .filter(|profile| {
            Checked::check(&profile.toml).is_ok_and(|plan| {
                plan.sources.iter().all(|source| {
                    view.pads
                        .iter()
                        .any(|pad| names(&source.pad, pad) && pad.connected && pad.setup.is_none())
                })
            })
        })
        .map(|profile| profile.name.as_str())
        .collect()
}

/// The profiles a middle click could act on, in the order of their
/// names: the ones that run, to stop, and the ones that could start.
#[must_use]
pub fn candidates(view: &View) -> Vec<&str> {
    let mut names: Vec<&str> = view
        .setups
        .iter()
        .map(|setup| setup.name.as_str())
        .chain(ready(view))
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

/// The profile a middle click acts on: the one `remembered`, the last
/// started, stopped or scrolled to, while it can be acted on; without
/// one, the only setup, or the only candidate. Nothing where that
/// leaves a choice, until the wheel makes it.
#[must_use]
pub fn target<'a>(view: &'a View, remembered: Option<&str>) -> Option<&'a str> {
    let candidates = candidates(view);
    if let Some(kept) = candidates.iter().find(|name| Some(**name) == remembered) {
        return Some(kept);
    }
    match (view.setups.as_slice(), candidates.as_slice()) {
        ([only], _) => Some(only.name.as_str()),
        (_, [only]) => Some(only),
        _ => None,
    }
}

/// The tooltip's text for what the daemon publishes now, with `target`
/// marked.
#[must_use]
pub fn description(view: &View, target: Option<&str>) -> String {
    if !view.alive {
        return "The daemon isn't running".to_string();
    }
    if let Some(theirs) = &view.mismatch {
        return menu::mismatch_text(theirs);
    }
    let mut lines = Vec::new();
    if view.setups.is_empty() {
        lines.push("Nothing is running".to_string());
        let connected = view.pads.iter().filter(|pad| pad.connected).count();
        lines.push(format!("{connected} of {} pads connected", view.pads.len()));
    }
    for setup in &view.setups {
        setup_lines(view, setup, target, &mut lines);
    }
    // A line each, so the mark has somewhere to be.
    for name in ready(view) {
        lines.push(format!("{} {name}: Ready", marked(name, target)));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use ctrlassist::Hiding;

    use super::*;
    use crate::state::tests::view;

    const COUCH: &str =
        "mode = \"toggle\"\nprimary = \"model:054c:0ce6\"\nassist = \"model:045e:0b12\"\n";
    const DESK: &str =
        "mode = \"priority\"\nprimary = \"model:054c:0ce6\"\nassist = \"model:dead:beef\"\n";

    fn kept(phase: Option<Phase>) -> View {
        let mut view = view(phase, Hiding::Service);
        (view.pads[0].vendor, view.pads[0].product) = (0x054c, 0x0ce6);
        (view.pads[2].vendor, view.pads[2].product) = (0x045e, 0x0b12);
        view.profiles[0].toml = COUCH.to_string();
        view.profiles[1].toml = DESK.to_string();
        view
    }

    #[test]
    fn a_running_setup_says_its_mode_its_pads_and_who_has_control() {
        assert_eq!(
            description(&kept(Some(Phase::Running)), Some("couch")),
            "► couch: Running · Toggle\n      Assist: Xbox pad\n    ● Primary: DualSense\n      Virtual: Generic"
        );
        // Held, nobody has control, and the pad that left is said so.
        assert_eq!(
            description(&kept(Some(Phase::Held)), None),
            "  couch: Held · Toggle\n      Assist: disconnected\n      Primary: DualSense\n      Virtual: Generic"
        );
        // One that failed says why, in place of its parts.
        assert_eq!(
            description(&kept(Some(Phase::Failed)), None),
            "  couch: Failed · Toggle\n    a virtual pad refused a write"
        );
        // A virtual pad not made yet, and a profile written by hand.
        let mut odd = kept(Some(Phase::Starting));
        odd.profiles.clear();
        odd.setups[0].sinks[0].node = None;
        assert!(description(&odd, None).starts_with("  couch: Starting\n"));
        assert!(description(&odd, None).ends_with("      Virtual: not made"));
    }

    #[test]
    fn a_long_pad_name_is_cut_to_fit_a_line() {
        assert_eq!(short("Wireless Controller"), "Wireless Controller");
        assert_eq!(
            short("Sony Interactive Entertainment DualSense Wireless Controller"),
            "Sony Interactive Entertainm…"
        );
        let mut long = kept(Some(Phase::Running));
        long.pads[0].name = "Sony Interactive Entertainment DualSense".to_string();
        assert!(
            description(&long, None).contains("● Primary: Sony Interactive Entertainm…\n"),
            "{}",
            description(&long, None)
        );
    }

    #[test]
    fn a_middle_click_acts_on_the_remembered_profile_or_the_only_one() {
        // Two could start: there is a choice until one is remembered.
        let mut two = kept(None);
        two.profiles[1].toml = "mode = \"unicast\"\npad = \"model:054c:0ce6\"\n".to_string();
        assert_eq!(candidates(&two), ["couch", "desk"]);
        assert_eq!(target(&two, None), None);
        assert_eq!(target(&two, Some("desk")), Some("desk"));
        assert_eq!(target(&two, Some("gone")), None);
        assert_eq!(
            description(&two, Some("desk")),
            "Nothing is running\n3 of 3 pads connected\n  couch: Ready\n► desk: Ready"
        );
        // One could: that one. One runs: that one, to stop.
        assert_eq!(target(&kept(None), None), Some("couch"));
        assert_eq!(target(&kept(Some(Phase::Running)), None), Some("couch"));
    }

    #[test]
    fn with_nothing_running_it_counts_the_pads_and_names_what_could_start() {
        // `couch` names two free pads; `desk` names one nobody has.
        assert_eq!(
            description(&kept(None), None),
            "Nothing is running\n3 of 3 pads connected\n  couch: Ready"
        );
        let mut away = kept(None);
        away.pads[2].connected = false;
        assert_eq!(
            description(&away, None),
            "Nothing is running\n2 of 3 pads connected"
        );
        // A profile that is wrong is never ready.
        let mut wrong = kept(None);
        wrong.profiles[0].problem = Some(ctrlassist::Problem {
            at: String::new(),
            message: "changed".to_string(),
        });
        assert!(ready(&wrong).is_empty());

        let mut down = kept(None);
        down.alive = false;
        assert_eq!(description(&down, None), "The daemon isn't running");
    }
}
