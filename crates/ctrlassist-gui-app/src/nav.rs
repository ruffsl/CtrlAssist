//! Browser-style back/forward navigation for the sidebar panes.
//!
//! Every pane change funnels through one Rust-owned history stack so
//! Alt+Left/Right (a top-level `FocusScope` in `main.slint`) and the
//! mouse back/forward buttons (a winit event hook in `lib.rs`) can
//! replay it. Each entry remembers the pane and where focus was when it
//! was left, so going back puts focus where it was. No pane has a
//! selection to remember yet; each brings its own as it is built, and
//! with it the click-throughs from one pane into another.
//!
//! [`wire`] returns the [`NavHandles`] the rest of the app needs: the
//! winit mouse-button hook replays history through `back` / `forward`.
//! The `apply` closure here is the only writer of
//! `AppWindow.active-pane`.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, SharedString};

use crate::{AppWindow, Nav, Pane};

/// The master/detail selection captured alongside a pane.
#[derive(Clone, PartialEq)]
enum NavSel {
    /// Nothing to restore.
    None,
}

struct NavEntry {
    pane: Pane,
    sel: NavSel,
    /// Where focus was when this page was left: the anchor of the stop
    /// that triggered the move (`Nav.focus-seq`, and its `accessible-id`,
    /// which survives the pane's re-creation; a link into another
    /// pane), and its column (`Nav.column`: sidebar, list or
    /// detail) for when that stop is gone, so coming back puts focus
    /// where it was rather than leaving a remote with nothing focused.
    anchor: i32,
    anchor_id: SharedString,
    column: i32,
    /// Reached through a click-through rather than the sidebar: Escape
    /// on the sidebar goes back from such a page.
    click_through: bool,
}

/// The focus position a history entry stores for its return.
struct Return {
    anchor: i32,
    anchor_id: SharedString,
    column: i32,
}

struct NavHistory {
    entries: Vec<NavEntry>,
    cursor: usize,
}

impl NavHistory {
    fn navigate(&mut self, app: &AppWindow, pane: Pane, sel: NavSel, click_through: bool) -> bool {
        let cur = self.cursor;
        let cur_pane = self.entries[cur].pane;
        self.entries[cur].sel = read_current_sel(app, cur_pane);
        self.entries[cur].leave_from(app);
        if self.entries[cur].pane == pane && self.entries[cur].sel == sel {
            return false;
        }
        self.entries.truncate(cur + 1);
        self.entries.push(NavEntry {
            pane,
            sel,
            anchor: -1,
            anchor_id: SharedString::default(),
            column: 0,
            click_through,
        });
        self.cursor = self.entries.len() - 1;
        true
    }

    /// Move one step through the history: the destination's pane,
    /// selection and the focus position to return to there.
    fn step(&mut self, app: &AppWindow, forward: bool) -> Option<(Pane, NavSel, Return)> {
        let cur = self.cursor;
        let next = if forward {
            (cur + 1 < self.entries.len()).then_some(cur + 1)?
        } else {
            cur.checked_sub(1)?
        };
        let cur_pane = self.entries[cur].pane;
        self.entries[cur].sel = read_current_sel(app, cur_pane);
        self.entries[cur].leave_from(app);
        self.cursor = next;
        let entry = &self.entries[next];
        Some((
            entry.pane,
            entry.sel.clone(),
            Return {
                anchor: entry.anchor,
                anchor_id: entry.anchor_id.clone(),
                column: entry.column,
            },
        ))
    }

    /// Whether Escape on the sidebar has a click-through to go back from.
    fn click_through_back(&self) -> bool {
        self.cursor > 0 && self.entries[self.cursor].click_through
    }
}

impl NavEntry {
    /// Record where focus is as this page is left. A page left through
    /// the sidebar keeps no stop: the item focused by then is the
    /// destination's (it navigates from its focus-in), and focusing it
    /// on the way back would navigate forward again. The column
    /// fallback focuses the active item instead.
    fn leave_from(&mut self, app: &AppWindow) {
        let nav = app.global::<Nav>();
        let column = nav.get_column();
        if column == 0 {
            self.anchor = -1;
            self.anchor_id = SharedString::default();
            self.column = 0;
            return;
        }
        self.anchor = nav.get_focus_seq();
        self.anchor_id = nav.get_focus_id();
        self.column = column;
    }
}

/// After a history step, put focus back where it was on that page:
/// the stop that triggered the move, through its anchor, or its column
/// when that stop is gone (the shell's anchor fallback). A tick later:
/// the pane was just re-created, and a stop created in the same pass
/// as the request wouldn't see it.
fn restore_return(app: &AppWindow, ret: &Return) {
    let weak = app.as_weak();
    let (anchor, anchor_id, column) = (ret.anchor, ret.anchor_id.clone(), ret.column);
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        if let Some(app) = weak.upgrade() {
            app.global::<Nav>()
                .invoke_request_anchor(anchor, anchor_id, column);
        }
    });
}

/// Tell the shell whether Escape on the sidebar goes back.
fn publish_back(app: &AppWindow, history: &NavHistory) {
    app.global::<Nav>()
        .set_click_through_back(history.click_through_back());
}

// `Rc<dyn Fn>` aliases for the navigation closures — readable call sites
// with clippy's `type_complexity` lint satisfied in one place.
type ApplyFn = Rc<dyn Fn(&AppWindow, Pane, &NavSel)>;
type GoFn = Rc<dyn Fn(&AppWindow, Pane, NavSel, bool)>;
type StepFn = Rc<dyn Fn(&AppWindow, bool)>;
type NavActionFn = Rc<dyn Fn(&AppWindow)>;

/// Navigation entry points used by `lib.rs`: the winit mouse-button
/// hook replays history through `back` / `forward`.
pub struct NavHandles {
    pub(crate) back: NavActionFn,
    pub(crate) forward: NavActionFn,
}

impl std::fmt::Debug for NavHandles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NavHandles").finish_non_exhaustive()
    }
}

/// Wire the `navigate` / `nav-back` / `nav-forward` Slint callbacks to a
/// fresh history seeded at the current pane, and hand back the entry
/// points the rest of the app drives navigation through.
pub fn wire(app: &AppWindow) -> NavHandles {
    let history = Rc::new(RefCell::new(NavHistory {
        entries: vec![NavEntry {
            pane: app.get_active_pane(),
            sel: NavSel::None,
            anchor: -1,
            anchor_id: SharedString::default(),
            column: 0,
            click_through: false,
        }],
        cursor: 0,
    }));

    // Set the pane and restore its selection. The sole writer of
    // `active-pane`; never touches history (callers own that).
    let apply: ApplyFn = Rc::new(move |app: &AppWindow, pane: Pane, sel: &NavSel| {
        app.set_active_pane(pane);
        match sel {
            NavSel::None => {}
        }
    });

    // Record a new destination and apply it.
    let go: GoFn = {
        let history = history.clone();
        let apply = apply.clone();
        Rc::new(
            move |app: &AppWindow, pane: Pane, sel: NavSel, click_through: bool| {
                let mut history = history.borrow_mut();
                if !history.navigate(app, pane, sel.clone(), click_through) {
                    return;
                }
                apply(app, pane, &sel);
                publish_back(app, &history);
                // A click-through opens a detail on the new pane with
                // focus still on the stop that clicked, which the pane
                // switch destroys: land on the detail, a tick later,
                // once the new pane exists to answer (no anchor to
                // match, so the shell's fallback asks for the column).
                if click_through {
                    app.global::<Nav>()
                        .invoke_request_anchor(-1, SharedString::default(), 2);
                }
            },
        )
    };

    // Move one step back (`forward == false`) or forward through the
    // history, capturing the page being left so its selection is current.
    let step: StepFn = {
        let apply = apply.clone();
        Rc::new(move |app: &AppWindow, forward: bool| {
            let mut history = history.borrow_mut();
            if let Some((pane, sel, ret)) = history.step(app, forward) {
                apply(app, pane, &sel);
                restore_return(app, &ret);
                publish_back(app, &history);
            }
        })
    };

    {
        let go = go.clone();
        let weak = app.as_weak();
        app.on_navigate(move |pane| {
            if let Some(app) = weak.upgrade() {
                let sel = read_current_sel(&app, pane);
                go(&app, pane, sel, false);
            }
        });
    }
    {
        let step = step.clone();
        let weak = app.as_weak();
        app.on_nav_back(move || {
            if let Some(app) = weak.upgrade() {
                step(&app, false);
            }
        });
    }
    {
        let step = step.clone();
        let weak = app.as_weak();
        app.on_nav_forward(move || {
            if let Some(app) = weak.upgrade() {
                step(&app, true);
            }
        });
    }

    let back: NavActionFn = {
        let step = step.clone();
        Rc::new(move |app| step(app, false))
    };
    let forward: NavActionFn = {
        let step = step.clone();
        Rc::new(move |app| step(app, true))
    };

    NavHandles { back, forward }
}

/// The selection a pane holds now, to record as it is left.
fn read_current_sel(_app: &AppWindow, _pane: Pane) -> NavSel {
    NavSel::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> AppWindow {
        i_slint_backend_testing::init_no_event_loop();
        AppWindow::new().expect("AppWindow::new")
    }

    fn history_for(app: &AppWindow) -> NavHistory {
        NavHistory {
            entries: vec![NavEntry {
                pane: app.get_active_pane(),
                sel: NavSel::None,
                anchor: -1,
                anchor_id: SharedString::default(),
                column: 0,
                click_through: false,
            }],
            cursor: 0,
        }
    }

    #[test]
    fn escape_goes_back_only_from_a_click_through_and_returns_to_its_anchor() {
        let app = app();
        let mut history = history_for(&app);
        assert!(
            !history.click_through_back(),
            "nothing to go back to at the start"
        );

        // A sidebar switch isn't a click-through.
        assert!(history.navigate(&app, Pane::About, NavSel::None, false));
        assert!(!history.click_through_back());
        app.set_active_pane(Pane::About);
        app.global::<Nav>()
            .invoke_stop_focused(2, 0, SharedString::from("a-link"));
        let anchor = app.global::<Nav>().get_focus_seq();
        assert!(history.navigate(&app, Pane::Profiles, NavSel::None, true));
        assert!(history.click_through_back());

        // Back lands on the anchor of the stop that clicked through.
        let (pane, _, ret) = history.step(&app, false).unwrap();
        assert!(pane == Pane::About);
        assert_eq!(ret.anchor, anchor);
        assert_eq!(ret.anchor_id, "a-link");
        assert_eq!(ret.column, 2);
        assert!(!history.click_through_back());
    }

    #[test]
    fn repeated_destination_is_ignored_and_new_navigation_truncates_forward_history() {
        let app = app();
        let mut history = history_for(&app);

        assert!(history.step(&app, false).is_none());
        assert!(history.navigate(&app, Pane::About, NavSel::None, false));
        app.set_active_pane(Pane::About);
        assert!(!history.navigate(&app, Pane::About, NavSel::None, false));
        assert_eq!(history.entries.len(), 2);

        assert!(history.step(&app, false).is_some());
        app.set_active_pane(Pane::Pads);
        assert!(history.navigate(&app, Pane::Profiles, NavSel::None, false));
        assert_eq!(history.entries.len(), 2);
        assert!(history.step(&app, true).is_none());
    }

    #[test]
    fn a_page_left_through_the_sidebar_returns_to_its_active_item() {
        let app = app();
        let mut history = history_for(&app);
        // The Profiles item has focus, and has reported itself, by the
        // time its focus-in navigates.
        app.global::<Nav>()
            .invoke_stop_focused(0, 0, SharedString::from("sidebar-profiles"));
        assert!(history.navigate(&app, Pane::Profiles, NavSel::None, false));

        // Back names no stop: focusing the Profiles item would navigate
        // forward again. The column fallback focuses the active item.
        let (pane, _, ret) = history.step(&app, false).unwrap();
        assert!(pane == Pane::Pads);
        assert_eq!(ret.anchor, -1);
        assert_eq!(ret.anchor_id, "");
        assert_eq!(ret.column, 0);
    }
}
