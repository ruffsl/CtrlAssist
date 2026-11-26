# Slint notes — what I learned while building ctrlassist-gui

Kept hard-wrapped, as TisVlay's is, so the two can be diffed.

Self-notes for future me. Copied from TisVlay's `.claude/slint.md` with
the window, whose Slint code is TisVlay's: what was found there holds
here, and the names are this workspace's. The upstream Slint skill at
[docs/skills/slint/SKILL.md](https://github.com/slint-ui/slint/blob/d5364974c658785f04bc99cdf426bd40f83d6387/docs/skills/slint/SKILL.md)
covers the basics; this file captures the things I stumbled on that
the skill doesn't say outright.

## MCP server

Slint ships an in-process MCP server (the `slint/mcp` feature, 1.17+).
It speaks JSON-RPC over Streamable HTTP at `http://127.0.0.1:<port>/mcp`.
There's no separate process — the GUI binary itself listens. Fastest
launch: `just gui-start` against `just daemon-start` (it wires the env
vars + feature flag below unless `GUI_MCP=false`), then attach via the
project-root `.mcp.json`.

### Setup gotchas

- **Enable it on the command line, not in `Cargo.toml`.** Pass
  `--features slint/mcp` to `cargo run`; do *not* declare a crate
  feature for it. It's a dependency feature (pulls the server via
  `i-slint-backend-selector/mcp` → `i-slint-backend-testing/mcp`), so
  keeping it off our own feature list means plain `cargo build` /
  `--all-features` never drag in its prost/protox stack.

- **No explicit init call.** `BackendSelector::select()` auto-starts the
  server when the binary is built with `slint/mcp` and `SLINT_MCP_PORT`
  is set (`i-slint-backend-selector`'s `select_internal` →
  `init_testing_backends` → `mcp_server::init()`). Absent the env var,
  init is a no-op — so a `slint/mcp` build without the port is inert.

- **Element ids must be compiled in.** The element tree is dead
  without them: `get_element_tree` returns only the root and
  `query_element_descendants` returns empty arrays. ctrlassist-gui's
  build.rs emits them for every non-release profile and overrides
  `SLINT_EMIT_DEBUG_INFO`, so a `--release` build has none. Build with
  the test (or dev) profile and set the port at runtime:
  ```sh
  SLINT_MCP_PORT=9315 \
      cargo run --profile test -p ctrlassist-gui-app --features slint/mcp
  ```

- **Disabling Slint's `accessibility` default feature does NOT break
  MCP introspection.** I read the source at
  [internal/backends/testing/mcp_server.rs](https://github.com/slint-ui/slint/blob/d5364974c658785f04bc99cdf426bd40f83d6387/internal/backends/testing/mcp_server.rs)
  to confirm. `accessibility` is required only for
  `invoke_accessibility_action` and the `matchElementAccessibleRole`
  query filter — neither has been load-bearing. Element trees,
  screenshots, and `click_element` work without it. Drop accessibility
  with confidence if it's causing zbus-tokio panics in `accesskit_unix`.

### Initialize handshake

Three calls in order, each as a separate POST:

```sh
curl http://localhost:9315/mcp -X POST \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","id":1,"method":"initialize",
       "params":{"protocolVersion":"2025-06-18",
                 "capabilities":{},
                 "clientInfo":{"name":"claude-code","version":"1"}}}'

curl http://localhost:9315/mcp -X POST \
  -H 'Content-Type: application/json' \
  -H 'Accept: application/json, text/event-stream' \
  -d '{"jsonrpc":"2.0","method":"notifications/initialized"}'

# Then any tool/call.
```

### Read text trees, not screenshots

This is the biggest win and I kept forgetting it. **Before reading
pixel coordinates out of a screenshot, dump the element tree as text.**
Sizes, absolute positions, and parent-child relationships are exact —
no parsing of fuzzy PNG data, no guessing at scale factors.

```sh
HANDLE=$(curl ... query_element_descendants ... | jq -c '.result.content[0].text | fromjson | .elementHandles[0]')
curl ... get_element_tree elementHandle=$HANDLE maxElements=300 \
  | jq -r '.result.content[0].text' \
  | jq '.elements | map({type: .typeNamesAndIds[0].typeName, abs: .absolutePosition, size: .size})'
```

Use screenshots only to verify *visual* qualities (color, alignment,
font weight) that the tree can't express.

### Handle invalidation

Element handles have an `index` AND a `generation` field. Each property
write or model rebuild bumps the generation, invalidating prior
handles. **Chain query → use in a single curl pipeline.** Don't store
a handle in a shell var and reuse later — it goes stale within
milliseconds when the GUI is doing anything.

```sh
# Good — fetch handle, immediately use it:
HANDLE=$(curl ... query_element_descendants ...)
curl ... click_element elementHandle=$HANDLE
```

If a handle goes stale you'll get `Element handle ... refers to element
that was destroyed`. Re-query.

### Query syntax — the gotcha

`matchElementTypeNameOrBase` does NOT mean "match including subclasses";
it matches **either the type name OR a known base type**, and only
works for built-in element types (`Rectangle`, `TouchArea`, `Text`,
etc.). For user-defined components like `IconBtn` or `ListRow`, use
`matchElementTypeName` (exact). The `OrBase` form returned `{}` for
those — confused me for a while.

```json
// User component:
{"matchElementTypeName": "IconBtn"}

// Built-in:
{"matchElementTypeNameOrBase": "TouchArea"}

// Qualified user component id:
{"matchElementId": "IconBtn::touch"}
```

### click_element on custom components

A user component must contain a `TouchArea` for clicks to register;
the MCP server doesn't synthesize clicks from raw geometry. Two
patterns that work:

1. Component **inherits Rectangle**; `TouchArea` is a child (TouchArea
   inherits the Rectangle's size). This is what `IconBtn`, `ListRow`,
   `SidebarItem` use, through `Focusable`, which is Rectangle-rooted.
2. Component **wraps content + TouchArea inside a layout**, with
   explicit `width: 100%; height: 100%` on the TouchArea.

What does NOT work: a non-Rectangle component with a Rectangle child
and a sibling TouchArea — the TouchArea's preferred size is 0, click
events miss it. (A button widget was fixed by switching it to `inherits Rectangle`.)

### Limitations to plan around

- **`set_element_value` only works on Slint inputs** (TextInput,
  Slider, Spinbox, etc.). It can't toggle a `bool` property on
  AppWindow or set a model. For programmatic state changes from MCP,
  the workaround is to fire a callback (e.g., via
  `invoke_accessibility_action` on a button that has the right
  callback), or have the user click for you.

- **Sidebar nav doesn't always click via MCP** in my custom
  `SidebarItem`. It worked in some configurations and not others. I
  haven't fully diagnosed it — possibly ScrollView interactions or
  layout-collapse issues. Workaround: `dispatch_key_event` (Tab to
  focus, Enter), or set the navigation property directly via a Rust
  handler invoked through some other path.

- **Animated transitions can land your screenshot mid-frame.**
  `take_screenshot` is synchronous but the UI may still be
  transitioning. Insert a `sleep 1` after a click before screenshotting
  if state should have settled.

## Slint-side patterns I committed to

### Fluent + unfocused = washed out chrome — build themed widgets

Slint's std-widgets defer entirely to the platform style. On Linux
that's Fluent. **Fluent dims chrome heavily on unfocused windows AND
ignores user-supplied Theme tokens.** MCP screenshots are typically
captured against an unfocused window, so std-widget Buttons /
LineEdits / ComboBoxes / CheckBoxes appear nearly invisible.

Two consequences:
1. You'll be tempted to dismiss the unreadability as "MCP screenshot
   artifact." It's *partly* that, but Fluent also fights design
   fidelity even when focused.
2. The right answer for any input that the design pins down is a
   custom themed widget — not a wrapper around the std-widget. See
   [crates/ctrlassist-gui/ui/widgets.slint](../crates/ctrlassist-gui/ui/widgets.slint):
   `IconBtn` and `ThemedSelect` (the latter in
   `ui/globals/select.slint`). Both drive every color from
   `Theme.*` directly, no platform style in the loop.

`ThemedSelect` was the hardest — std `ComboBox`'s dropdown is
non-trivial to replicate. Its list is an in-window overlay at the
AppWindow root, not a `PopupWindow`, because a popup clips at the
window's edge (docs/concerns/slint-combobox-popup-clipped.md).

### Layout collapse pitfalls

- **Slint Rectangles don't lay out children.** A layout
  (HorizontalBox, VerticalBox, etc.) inside a non-layout Rectangle
  takes its *preferred* size, not the Rectangle's. Either make the
  wrapper inherit Rectangle so the layout fills, or use `width: 100%`
  on the inner layout (safe inside Rectangle — see next bullet for the
  caveat).

- **`width: 100%` on a layout child is dangerous and almost always
  wrong.** Setting `width: <percent>` on any element triggers
  `fixed_width=true` AND records `min_width = <percent>` (per
  [layout.rs:254](https://github.com/slint-ui/slint/blob/d5364974c658785f04bc99cdf426bd40f83d6387/internal/compiler/layout.rs#L254)),
  i.e. `min_percent: 100`. That value bubbles upward through every
  wrapping layout's `box_layout_info_ortho` merge (which takes `.max()`
  of `min_percent`), and the grandparent's `solve_box_layout` resolves
  it as an absolute min equal to the full content width. The wrapping
  layout then claims the entire row, overshooting fixed siblings by
  their widths.

  Layouts fill their parent's available width by default — `width:
  100%` on a layout inside another layout is unnecessary and harmful.
  Symptom: a stretch column that overshoots a sibling row by exactly
  the fixed sibling's width. Diagnosis: grep ancestors for `width:
  100%`. Fix: delete it; rely on default fill.

  Rectangle BLOCKS this propagation — its layout-info is hardcoded
  (per [layout.rs:790](https://github.com/slint-ui/slint/blob/d5364974c658785f04bc99cdf426bd40f83d6387/internal/compiler/layout.rs#L790)),
  ignoring children. So `width: 100%` *inside a Rectangle wrapper* is
  fine. The bug only triggers when the chain is layout → layout → ...
  → stretch allocator without a Rectangle in between.

- **Slint's box-layout stretch math itself is CORRECT.** Verified via
  minimal repro: a HorizontalLayout with three Rectangle children
  (fixed-width, `horizontal-stretch: 1`, fixed-width) allocates the
  middle child the exact remainder. If you observe stretch overshoot,
  the cause is almost always the `width: 100%` propagation above, not
  the stretch algorithm.

- **`ScrollView` allows horizontal overflow by default.** Its
  `content-width` auto-sizes to children's preferred width; if a
  child's preferred exceeds visible-width, content scrolls
  horizontally and clips at the window edge instead of wrapping
  inside the card. For vertical-only scroll (the typical case), pin
  it: `ScrollView { content-width: self.visible-width; ... }`.

- **Per-side borders don't exist.** No `border-bottom-width` etc. Use
  a thin Rectangle as a separator at `y: parent.height - 1px`.

- **`alignment: center` on a Layout is MAIN-axis (horizontal for
  HorizontalLayout).** Don't use it expecting cross-axis (vertical)
  centering — that's the default for cross-axis already.

- **`width: Npx` does not set `preferred_width`.** It sets
  `fixed_width=true`, `min_width=N`, `max_width=N`, and the user's
  `width` binding. The solver clamps the cell's preferred to
  `[min,max] = [N,N]` so the math still works. But there's no
  cooperation with `preferred-width` queries — if you read
  `elem.preferred-width` it returns 0, not N.

### Send / Slint thread bridge

`ModelRc<T>` wraps an `Rc`, so it's `!Send`. Don't try to ship one
through `slint::invoke_from_event_loop`. Pattern: build a plain
`Vec<T>` (`Send`) on the tokio side, transfer it through the closure,
and hand it to the model *inside* the event-loop closure. Here that
means `models.rs`'s `sync`, which updates an existing `VecModel`'s rows
in place rather than building a new one, so focus and scroll survive a
change.

For struct fields read from a `ModelRc` row: don't bind them through
deeply nested `<=>` chains. Slint's struct-from-model bindings have
been unreliable across step transitions in my testing. Publish the
fields you need as separate properties on the AppWindow and set them
explicitly from Rust on selection / data change. Tedious but
predictable.

### Properties + callbacks chain

When wiring a property through three levels (e.g. AppWindow →
ProfilesPane → a detail component), forgetting to forward at any layer silently turns
the inner property into a default-valued in property. The compiler
won't complain. Symptom: data set in Rust shows as "—" / empty in the
inner component. Trace the chain in the slint sources end-to-end when
this happens — `grep -nE '<property-name>' ui/` is enough to
spot the missing layer.

### ComboBox: `current-index` is the canonical state, not `current-value`

Std `ComboBox` exposes both `current-index: int` and `current-value:
string`, and they look interchangeable for two-way state. They aren't.
[`combobox-base.slint`](https://github.com/slint-ui/slint/blob/master/internal/compiler/widgets/common/combobox-base.slint)
defines `current-value` as a default-bound expression off
`current-index`/`model`, but `update-current-value()` *imperatively
overwrites* it on every user select. So a binding like
`current-value: my-derived-expression(...)` works on first render and
then dies the moment the user clicks an item — the imperative write
clobbers the binding, and `current-value` becomes plain state divorced
from whatever you derived it from.

Symptom: display reads correctly until the user picks an item, then
the ComboBox shows the picked label but downstream state advances
with whatever the binding *would have* computed for the previous
selection. Hard to spot because the visible state looks consistent
in isolation.

Bind `current-index <=> some-int-property` for two-way state. Read
the id / display / whatever-derived data via `model[some-int-property]`.
The struct-model pattern (`[Option { id, name }]` + `current-index`
selection + `model[i].field` reads) is the
[idiomatic upstream shape](https://github.com/slint-ui/slint/blob/master/examples/layouts/flexbox-interactive.slint) —
single integer of state, all derivations declarative.

### Pure callbacks: dependency tracking is shallow

Tempting pattern: declare `pure callback foo(string) -> string;` on
a global, bind it from Rust, use it as a binding source like
`current-value: foo(some-state)`. It works mechanically; what isn't
obvious is that **Slint's dependency tracker only sees properties
accessed in the binding *expression*, not data the Rust closure
captures**. So if the closure scans some other property (e.g., a
`[T]` model), the binding doesn't re-evaluate when that model
changes — only when `some-state` changes.

Workarounds: pass model-shape into the binding expression so Slint
tracks it (`foo(some-state, some-model.length)`), or maintain a
"salt" int that you bump from Rust whenever the model changes.
[Issue #9551](https://github.com/slint-ui/slint/issues/9551) (open
in 1.16) confirms a related case: re-binding the callback handler
in Rust doesn't dirty existing bindings either.

In practice the right answer is usually to skip pure callbacks in
binding context entirely and use the `current-index` + struct-model
pattern above. The pure-callback path is fine for one-shot lookups
called from event handlers (`=>` arrows); just don't trust it as a
binding source for state that depends on multiple properties.

### Keeping MCP out of production without a crate feature

The MCP server must never ship in production builds, but it does *not*
need a crate feature to gate it. `slint/mcp` is a dependency feature
passed on the command line (`cargo run --features slint/mcp`), so a
plain `cargo build` — and `--all-features`, since it's not one of our
crate's own features — never compiles it in. `SLINT_MCP_PORT` then
activates it at runtime; without it the built-in server is inert.

## Operational rituals

The user-driven workflow that worked for me:

1. **I rebuild + restart the GUI.** User does not. They only click in
   the live UI when I ask, since they aren't running my Bash for me.
2. **After a code change, kill + relaunch via Bash:** `just stop-all`
   ends the dev daemon, tray and window (an installed one keeps
   running); then `just daemon-start` and `just gui-start`, each in the
   background with its output to a file in the scratchpad. `gui-start`
   builds with the MCP server on port 9315 and live preview unless
   `GUI_MCP` or `GUI_LIVE` is `false`.
3. **I screenshot via MCP, then read the PNG with the Read tool.**
   Visual feedback loop that doesn't require the user.
4. **For interactive verification (real pads, a real game), the user
   drives.** I tell them what to click in plain English; they tell me
   the result. I check journald (`journalctl SYSLOG_IDENTIFIER=ctrlassist-gui
   --since "2 minutes ago" --no-pager`) for confirmation when callbacks
   should have fired.

## When MCP is the wrong tool

- **Multi-step UI flows.** Click navigation through nested custom
  components has been flaky. Drive interactive flows by asking the
  user; verify via screenshots + journal logs.
- **Live reload.** `slint-viewer --auto-reload crates/ctrlassist-gui` is
  the fast iteration path for pure layout/style work — the crate
  directory, so the viewer reads `slint-project.json` (the main file and
  the edition; a `.slint` path alone compiles without the edition). It
  doesn't run Rust callbacks, so anything tied to backend state has to
  be done in the full app. `--check` on the same directory prints the
  compiler's diagnostics, the info level included, which `slint-build`
  doesn't; `--screenshot out.png --size WxH --load-data props.json`
  renders a state headlessly (`SLINT_BACKEND=software`), with the root
  component's `in` properties set from the JSON.
- **Property inspection.** `get_element_properties` returns geometry
  + opacity + types only. It does NOT expose user-defined Slint
  property values. Add an `info!` log on the Rust side to introspect
  state.

## Slint text-rendering for large documents

Don't render document-scale plain text in Slint. The About → Notices
pane spent two iterations trying to do this in-app (originally a
`TextEdit { read-only: true }`, then a virtualized
`ListView { for line: Text }` with pinned row height) and neither
matched a browser for the ~1400-row license bundle. The reasoning,
in case it comes up again:

- **Rendering primitives.** `TextEdit` is editable under the hood
  ([`internal/core/textlayout/sharedparley.rs:1205`](https://github.com/slint-ui/slint/blob/master/internal/core/textlayout/sharedparley.rs#L1205))
  — it re-shapes the entire document on every frame with no cache,
  catastrophic for ~1400-row reads. `ListView` is the only virtualized
  primitive Slint exposes (compiler-special-cased in
  [`internal/compiler/object_tree.rs`](https://github.com/slint-ui/slint/blob/master/internal/compiler/object_tree.rs);
  algorithm at [`repeater.rs::update_visible_instances`](https://github.com/slint-ui/slint/blob/master/internal/core/model/repeater.rs))
  and its constraint is "exactly one `for` as children" — works fine
  for one-row-per-line, with shaping cached per row.
- **The structural ceiling.** Even with virtualized `ListView`, the
  remaining gap to a browser is renderer-architecture: femtovg
  redraws the full viewport every scroll frame and doesn't participate
  in Slint's partial-renderer / dirty-region pipeline. Maintainer
  Tronical confirmed in [slint #6754](https://github.com/slint-ui/slint/discussions/6754):
  *"Partial rendering with FemtoVG is a little tricky in the sense
  that the scissoring is implemented in the fragment shader, so it
  will still run the whole pipeline even for invisible pixels."*
  Skia *does* participate in partial rendering (CHANGELOG L557) and
  does help modestly, but `Text` widgets still don't render link
  colors / clickable anchors / cross-row selection. Browsers
  composite the `<pre>` block as a single layer that scrolls via
  transform; Slint has no equivalent.
- **No upstream demos / fixes.** Surveyed
  [`examples/`](https://github.com/slint-ui/slint/tree/master/examples)
  and [`demos/`](https://github.com/slint-ui/slint/tree/master/demos):
  every "About" page is a small static blurb (gallery, energy-monitor,
  weather-demo). The Slint team has no in-tree precedent for
  document-scale text rendering. The closest related upstream tickets
  ([#6944](https://github.com/slint-ui/slint/issues/6944) "TextEdit
  large-text lag" closed not-planned;
  [#7986](https://github.com/slint-ui/slint/discussions/7986)
  "ListView alternative" — maintainer ogoffart noted ListView's
  Rust-level optimizations *cannot be replicated in custom Slint
  code*) confirm this is a known gap without a roadmap.

The pivot: hand the HTML off to the user's browser via `xdg-open`.
[`crates/ctrlassist-gui-app/src/about.rs::open_notices_html`](../crates/ctrlassist-gui-app/src/about.rs)
prefers an installed copy at `/usr/share/doc/<pkg>/...` (FHS,
populated by nfpm) or `$prefix/share/doc/<pkg>/...` (current_exe()-
relative, covers Nix / AppImage / non-FHS), with a per-process
tempfile fallback in dev. Browser handles link colors, anchors,
search, copy-paste, and scroll smoothness for free.

If you ever consider rendering large text in Slint again: don't.
Open it in the user's browser. The effort-to-result curve is
unfavorable and the result is still inferior on every measurable
axis (smoothness, link affordance, copy/paste, search).
