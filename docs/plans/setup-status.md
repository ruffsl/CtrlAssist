# Setup status in the tray's icon and the window's sidebar

## State

- **Status:** the tray's icon is built, with the state composed into it; the sidebar's block is next.
- **Next action:** the maintainer's word on reading the theme's files with usvg; then step 3.

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
  | Held, waiting for a pad | pause, blue | active |
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

## Found

- **Step 1, read from the staged tray over the session bus:** the icon is `input-gaming` running and held alike; the overlay is `media-playback-start` while the setup runs and `emblem-pause` once its assist's pad is taken away, with the status going from active to needs attention, as it then was. What Plasma draws of them hasn't been looked at.

- **What Plasma 6.8 draws of an overlay, looked at on the maintainer's panel at a scale of 2:**
  - *By name* (`OverlayIconName`): nothing. The pad is drawn alone, though the name is one the theme has.
  - *As pixels* (`OverlayIconPixmap`): the overlay is drawn at the icon's lower right, in the colors sent. But Plasma then paints the icon and the overlay together into bitmaps of 16, 22, 32 and 48 pixels, whatever the screen's scale (`StatusNotifierItemSource::overlayIcon`), and the panel stretches one: the pad comes out blurred beside the same pad without an overlay, which is drawn from its vector at the screen's own resolution. The overlay is 8 pixels a side at most sizes, and with a larger bitmap offered only its top left quarter is drawn at this scale; with the 8-pixel one alone it is whole, and blocky.
  - *Upstream:* the blur and the cut-off overlay are [KDE bug 520235](https://bugs.kde.org/show_bug.cgi?id=520235), confirmed, open since May 2026 and unassigned to a fix: Plasma's code and not this tray's. Its reporter finds an overlay by name drawn well, which this panel doesn't bear out: looked at twice, with `media-playback-start` and with `emblem-pause` over `input-gaming`, the name read back from the tray over the session bus each time, and no overlay drawn. The likely cause, from reading and not proven: Kirigami's icon item loads a named theme icon afresh by its name where Plasma's own theme is in use (`Icon::iconPixmap`, since kirigami `f673694e6f90`, August 2024), which leaves behind the overlay Plasma had put on it; the reporter's icon wasn't the theme's. No report of that was found.
  - So the protocol's overlay can't give a sharp icon here by either route. The state has to be part of the icon itself.

- **The composed icon, tried on the same panel:** one SVG written from the theme's `input-gaming` and its `media-playback-pause-symbolic`, the glyph in the state's blue with a dark edge at the pad's lower right, kept in a directory named to the tray as its icon theme path and named as its icon. Plasma draws it from the vector: the pad as sharp as the plain one beside it, the pause whole and in its color. The two files are joined as text, each one's contents under a group with its own scale, the glyph's color set on its group in place of the theme's stylesheet; nothing is rendered here. Looked at for a held setup; a running one's picture was missed, and no other desktop has been tried.

- **Reading the theme's files with usvg, tried apart from the tray** (usvg 0.49, the release the window's toolkit already brings into the lockfile, with its `svgz` and `writer` features and none of its text ones: 30 crates, most of them there already):
  - It reads every file tried, Breeze's and Adwaita's, a gzip-compressed one among them, and writes each back as plain SVG. Breeze's pad written back and rendered is the same picture as the original, pixel for pixel at 256.
  - A stylesheet handed to it colors a glyph whatever way the theme colored it. Adwaita's symbolic glyphs carry their color as a literal `fill="#2e3436"`, not as Breeze's do through `currentColor`: joined as text, as the tray does now, an Adwaita glyph would stay dark gray on the pad, since nothing set on the group above it overrides a fill of its own. Through usvg with `* { fill: … }` it comes out in the state's color.
  - That stylesheet doesn't reach a fill written in an element's own `style` attribute, which Breeze's disconnected plug has for its two colors; those stay as the theme drew them. And it would fill a shape the theme meant only to outline; none of the glyphs picked has one.
  - It joins nothing: a tree read can't be put inside another, so the two files would still be joined as text, from what usvg wrote.
  - Not tried: what a desktop draws of a file usvg wrote, and any theme but these two.

## Deviations

- **The composed icon is built in place of step 2,** and nothing in the tray asks for attention any more: a failed setup's icon says so by its badge and stays still, as the maintainer chose. The icons are made as the tray starts, beside the daemon's socket, from the theme the desktop has then; a theme changed while the tray runs is taken up at its next start.

- **The overlay is given up,** after the look at step 1 and what was found of Plasma: the tray sends none, and its icon is the pad in every state until the state is made part of the icon itself. That is tried next as one SVG composed at run time from the theme's own pad and a glyph of the theme's in the state's color, named to the tray as its icon, so that the desktop draws it from a vector. Pixels rendered here, and icon files drawn beforehand, are what is left if a desktop doesn't draw that well.

- **A held setup no longer asks for attention.** Plasma pulses the icon of a tray item that does, and the maintainer found the motion too distracting for a setup that is only waiting. A failed one still asks.
