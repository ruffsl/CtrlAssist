# Setup status in the tray's icon and the window's sidebar

## State

- **Status:** planned; nothing built.
- **Next action:** the maintainer's read of this plan, then step 1.

## Goal

Say what the running setups are doing, the same way in both places a person glances at, without opening anything.

Today the tray's icon is the desktop theme's gamepad whether nothing runs, a setup is starting, or one is running; a held setup replaces it with the theme's warning triangle and a failed one with its error icon, so the icon stops being CtrlAssist's just when something wants attention. The window's sidebar paints an amber or red dot beside **Profiles** for a setup held or failed, says nothing of one running, names no profile, and leads nowhere.

## Decided

- **The tray's icon stays the gamepad, with the state as an overlay.** The StatusNotifierItem protocol has an overlay icon beside the main one (`OverlayIconName`; `overlay_icon_name` in ksni), and Plasma draws it at the main icon's corner. This is the smallest change and keeps the desktop's theme. A host that ignores overlays shows the plain gamepad; the title and the tooltip still say the state, as now.
- **Pre-drawn icons are the fallback,** as Sunshine ships (`sunshine-playing`, `sunshine-pausing`, `sunshine-locked`: its own logo with the badge drawn in, one file a state). The maintainer looks at the overlay on their Plasma first; if it doesn't read well at the tray's size, the icons are drawn from `docs/artwork/icon_48.svg` and installed with the packages instead. Step 1 ends at that look.
- **The states the icon tells apart:**

  | The worst of the setups | Overlay | Tray status |
  |---|---|---|
  | None running | none | passive |
  | Starting | none | active |
  | Running | play, green | active |
  | Held, waiting for a pad | pause, blue | needs attention |
  | Failed | error, red | needs attention |
  | The daemon isn't running, or is another release | unavailable, gray | passive |

  Held is blue, not amber: it is waiting, not a fault. What colors and shapes a theme's overlay names really give is found at step 1; the table is what is asked for.
- **The worst state wins,** as now: failed, then held, then starting, then running.
- **The sidebar gets a status block above Reload,** one row for each setup: the state's glyph, the profile's name, and the state in words where it isn't plainly running. Nothing is shown when nothing runs. The glyphs and colors are the tray's.
- **A row is a link and nothing more:** it opens **Profiles** on that profile, from any pane. Stopping stays on the profile's page.
- **A row is a focus stop,** reached by keys, a remote and a gamepad like the rail's items above it, couch mode included.
- **The dot beside Profiles goes,** the block saying the same and more.

## Open

- **Which theme names the overlays are.** Breeze has `emblem-pause`, `emblem-error`, `emblem-warning`, `emblem-unavailable` and `emblem-checked` among its emblems, and no play emblem; `media-playback-start` is the play glyph both Breeze and Adwaita have, in the theme's one color. Step 1 tries them and writes down what each theme and host shows.
- **More setups than the block has room for.** A window at its smallest height fits a few rows above Reload. Proposed: the block scrolls past four; to be looked at in step 3.

## Steps

1. **The overlay, on the tray.** `overlay_icon_name` from the worst state, the gamepad kept as the icon in every state, the status as in the table. Verified by the tray's unit tests over each state, and read back from the dev tray over the session bus. **Stops for the maintainer's look on Plasma,** with `just docs-tray-stage` to put a running and a held setup in front of them; overlay or pre-drawn is decided there.
2. **Pre-drawn icons,** only if step 1's look says so: the artwork, its installation in the packages and the Nix module, and the tray naming them. Verified by the package checks listing the files, and by the same look.
3. **The sidebar's status block.** Rows from the setups the window already mirrors, the link through the callback the Pads page already opens a profile with, the dot removed. Verified by the window's tests (rows for each state, the link's target, focus order) and a screenshot from `just docs-screenshots`.
4. **The guide.** GUI & Tray says what the icon's overlays and the block's rows mean; the screenshots are made again.

## Deviations

None yet.
