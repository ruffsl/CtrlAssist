# Breeze color mapping

## Context

`ctrlassist-gui`'s color tokens (`crates/ctrlassist-gui/ui/theme.slint`) were re-valued to match the KDE **Breeze** color schemes — **Breeze Light** and **Breeze Dark** — so the app reads as a native Breeze application in either scheme. The mapping was made in TisVlay, whose theme this is, and holds here unchanged. The values are not eyeballed: each token derives from a specific Breeze color-scheme role, listed below. The accent stays DE-driven (`crates/ctrlassist-gui-app/src/accent.rs` pushes `org.freedesktop.appearance.accent-color`); its default is already Breeze's blue.

Provenance is pinned by `crates/ctrlassist-gui/tests/theme_breeze.rs`, which text-parses `theme.slint` and asserts each fixed-hex token equals the value in this table (the tokens are `out property` brushes, so they can't be read back at runtime).

## Breeze roles used

From `colors/BreezeDark.colors` / `colors/BreezeLight.colors` (RGB → hex), dark / light:

| Role | Dark | Light |
|---|---|---|
| `View:BackgroundNormal` | `#141618` | `#ffffff` |
| `Window:BackgroundNormal` | `#202326` | `#eff0f1` |
| `Window:BackgroundAlternate` | `#292c30` | `#e3e5e7` |
| `Button:BackgroundNormal` | `#292c30` | `#fcfcfc` |
| `*:ForegroundNormal` | `#fcfcfc` | `#232629` |
| `*:ForegroundInactive` | `#a1a9b1` | `#707d8a` |
| `*:ForegroundPositive` | `#27ae60` | `#27ae60` |
| `*:ForegroundNegative` | `#da4453` | `#da4453` |
| `*:ForegroundNeutral` | `#f67400` | `#f67400` |
| `*:ForegroundLink` | `#1d99f3` | `#2980b9` |
| `Selection:BackgroundNormal` / `DecorationFocus` | `#3daee9` | `#3daee9` |

## Token → role (dark / light)

| Token | Breeze role | Dark | Light |
|---|---|---|---|
| `surface` (card/dialog) | Window bg | `#202326` | `#eff0f1` |
| `surface2` (input/control) | Button bg | `#292c30` | `#fcfcfc` |
| `surface-alt` (app bg / list panels) | View bg | `#141618` | `#ffffff` |
| `sidebar` | Window bg | `#202326` | `#eff0f1` |
| `sidebar-text` | FgNormal | `#fcfcfc` | `#232629` |
| `sidebar-text-dim` | FgInactive | `#a1a9b1` | `#707d8a` |
| `sidebar-hover` | Window BgAlt | `#292c30` | `#e3e5e7` |
| `text` | FgNormal | `#fcfcfc` | `#232629` |
| `text-dim` | FgInactive | `#a1a9b1` | `#707d8a` |
| `chip` | Window BgAlt | `#292c30` | `#e3e5e7` |
| `chip-text` | FgNormal | `#fcfcfc` | `#232629` |
| `row-hover` | Window BgAlt | `#292c30` | `#e3e5e7` |
| `success` | ForegroundPositive | `#27ae60` | `#27ae60` |
| `danger` | ForegroundNegative | `#da4453` | `#da4453` |
| `warn` | ForegroundNeutral | `#f67400` | `#f67400` |
| `info` | ForegroundLink | `#1d99f3` | `#2980b9` |
| `accent` (default) | Selection / DecorationFocus | `#3daee9` | `#3daee9` |

## Derived tokens (no direct Breeze role)

- **`border` / `border-strong` / `chip-border`** — Breeze defines no standalone border color; these are the frame shade KDE draws around cards and inputs (the Window/View background stepped toward the foreground). `border` `#31363b` / `#bcc0c4`; `border-strong` `#45494f` / `#9fa4aa`.
- **`text-faint`** — Breeze has a single inactive-foreground level; this is `ForegroundInactive` shaded toward the background: `#6e757e` / `#9096a0`.
- **`*-soft` / `*-text`** (status) — Breeze gives only the base hue. `-soft` = the base at low alpha (a tint wash for badges/banners); `-text` = the base brightened in dark / darkened in light so it stays legible on the wash. Single source per status, so re-basing the hue cascades.
- **`accent-pressed` / `accent-soft`** — derived from `accent` (brighter/darker, low-alpha) as before, so they track the DE accent.
- **`focus-ring`** — the keyboard-focus ring the focusable widgets draw (`FocusRing` in `widgets.slint`; its width and offset are `Design` tokens), equal to `accent`: Breeze's DecorationFocus is its selection color.
- **`row-selected`** — Breeze's Selection role *is* the accent, so a selected row is a soft `accent` wash, tracking the DE accent and the focus ring. **`row-selected-text`** keeps the normal `text` color (the label is not tinted to the accent), so selected rows stay legible against the wash.

## Intentionally not Breeze-derived

- **`scrim` / `drop-shadow`** — functional overlays (modal dim, elevation tint), same alpha in both schemes.
- **`maker-sony` / `maker-microsoft` / `maker-nintendo` / `maker-8bitdo` / `maker-valve`** — a pad maker's own color behind its mark on a pad's tile, mode-invariant.

## Verification

- `cargo test -p ctrlassist-gui --test theme_breeze` — the drift guard.
- Visual: run the GUI in both schemes (`org.freedesktop.appearance color-scheme`). Sidebar, list section headers, and cards render on `Window`; the app background and list panels on `View`; inputs on `Button`. Selection is an accent wash, and status pills stay legible in both schemes.
