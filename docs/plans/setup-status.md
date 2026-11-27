# Setup status in the tray's icon and the window's sidebar

## State

- **Status:** built, the guide's page and its screenshots with it; in review.
- **Next action:** the rest of what the review of the pull request (#65) found. Then a look at a panel, where nothing has been looked at since the pad became `applications-games` and the glyphs were recolored shape by shape: every state's icon, and a desktop other than Plasma.

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

- **Which theme names the overlays are.** Moot: the overlay was given up, and the glyphs are named in `badge.rs`.
- **More setups than the block has room for.** The rows scroll within the room they have, and one comes into view as it takes focus. Not yet looked at with more than fit.

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
  - That stylesheet doesn't reach a fill written in an element's own `style` attribute, which Breeze's disconnected plug has for its two colors; those stay as the theme drew them. And it would fill a shape the theme meant only to outline; none of the glyphs picked has one. (As first built, a rule marked important did reach them. The review found what else a rule over every shape does, and the glyph is now recolored in what usvg wrote, shape by shape: see Deviations.)
  - It joins nothing: a tree read can't be put inside another, so the two files would still be joined as text, from what usvg wrote.
  - Not tried then: what a desktop draws of a file usvg wrote, and any theme but these two.

## Deviations

- **From the review of the pull request,** by three reviewers each given a part of it:
  - The tray is active while the daemon is away or another release, where a desktop hid a passive one and the mark with it, and says nothing of the daemon until its first view is taken.
  - A theme with the pad in symbolic form alone gets no composed icons, its one color being dark on a dark panel once written into a file.
  - A glyph is recolored in what usvg wrote of it, each fill and stroke that is one; the rule over every shape filled a box left unfilled, kept a stroke's color and filled what a mask cuts with.
  - The icons' directory is made for the user alone, each file moved into place whole, and what can't be made is logged.
  - The staged scene's tray has a settings file of its own; it read, and could have written, the machine's.
  - The pad of the desktop entry and the Nix package is `applications-games` too.
- **One pad, `applications-games`,** for the tray's icon, its notifications, the window's logo and every glyph of a pad, where the plan and the window's had `input-gaming`: Breeze draws the first alike in color and symbolic, and the other's two forms are a shaded pad and a heavier glyph of another shape. It is a theme's icon for games as a kind of program, and a theme may draw it as something other than a pad.
- **Also from the maintainer's looks at the running window:** a list's rows are 52 high, by a token, for 64; its captions say what before how many; and the first row of a pad's STATUS card and a profile's is one height, so the rows under them are level across the panes.

- **Beyond the plan, from the maintainer's looks at the running window,** and kept in this branch: a profile's header gives its name a row of its own, with its buttons at the end of the row beneath and its mode said without a caption (Custom for one written by hand), at a pad's header's height; how a profile is sits above how it starts; a pad's model and tag sit level with the words beside them; the lists of pads and of profiles scroll their two sections apart, so that the virtual pads, or the profiles not running, can't be pushed out of sight by a long first section, the shorter shown whole up to half the room; and an installed tray's menu has no Quit, as TisVlay's has none.

- **A long name slides once and stays,** where it first went to its end and back for as long as its row was hovered or had focus. Looked into at the maintainer's asking: Chrome's guidance for a line cut short is this, on hover and focus; tvOS scrolls a focused item's label across; and motion that a hover or a focus starts and that goes on past five seconds wants a way to stop it (WCAG 2.2.2), which one that ends by itself doesn't. The widget is `SlideText`, and the names in the lists of pads and of profiles, and the title over a pad's details, take it too. A profile's title, which wrapped so that it could be read whole without a pointer and came out broken mid-word beside the header's buttons, is one line that slides under the pointer as a pad's is, at the maintainer's choice: the name in the list beside it slides on focus. The desktop's setting for less motion isn't read yet.

- **The sidebar's rows, after the maintainer's look:** they take the room between the last pane and Reload, between two thin lines, and scroll past it, in place of four rows dropped above Reload. A row says its state by its glyph alone; the words are its tip and its accessible label. A name too long to show whole travels to its end and back while its row is hovered or has focus, by a text widget made to be shared: the lists of pads and of profiles, and the titles over their details, are to take it next, once it has been seen moving.

- **The theme's files are read with usvg,** as the maintainer chose after what was found of it: each written back as plain shapes, the glyph in the state's color by a rule over every shape, then joined as text. An icon made that way is drawn sharp by Plasma, looked at for a running setup. The stop square marks a failed setup and an amber disconnected plug a daemon that isn't there, the gray first tried being lost on the pad.

- **The composed icon is built in place of step 2,** and nothing in the tray asks for attention any more: a failed setup's icon says so by its badge and stays still, as the maintainer chose. The icons are made as the tray starts, beside the daemon's socket, from the theme the desktop has then; a theme changed while the tray runs is taken up at its next start.

- **The overlay is given up,** after the look at step 1 and what was found of Plasma: the tray sends none, and its icon is the pad in every state until the state is made part of the icon itself. That is tried next as one SVG composed at run time from the theme's own pad and a glyph of the theme's in the state's color, named to the tray as its icon, so that the desktop draws it from a vector. Pixels rendered here, and icon files drawn beforehand, are what is left if a desktop doesn't draw that well.

- **A held setup no longer asks for attention.** Plasma pulses the icon of a tray item that does, and the maintainer found the motion too distracting for a setup that is only waiting. A failed one still asked then; it no longer does, by the entry under this one.
