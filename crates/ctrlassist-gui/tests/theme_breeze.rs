// Drift guard for the Breeze color mapping in `ui/theme.slint`.
//
// The theme tokens are `out property` brushes compiled into the UI, so
// they can't be read back at runtime — this test text-parses the .slint
// source instead and asserts each fixed-hex token still equals the value
// its Breeze color-scheme role resolves to. If someone re-values a token
// or a future Breeze bump changes a role, this fails and points at the
// exact token. The provenance table lives in docs/gui/theme-breeze.md.
//
// Only the *fixed-hex* tokens are pinned here. Accent-derived and status
// `-soft`/`-text` tokens are formulas (checked structurally at the bottom),
// and the functional overlays (scrim, drop-shadow) and brand swatches are
// intentionally not Breeze-derived, so they're excluded.

use std::collections::HashMap;

/// Parse `theme.slint` into a `token -> value-expression` map. Handles the
/// `out property <TYPE> name: VALUE;` / `in-out property <TYPE> name: VALUE;`
/// shape, stripping trailing `// …` comments and the closing `;`.
fn tokens() -> HashMap<String, String> {
    let src = include_str!("../ui/theme.slint");
    let mut map = HashMap::new();
    for raw in src.lines() {
        // Drop any trailing line comment before parsing the value.
        let line = raw.split("//").next().unwrap_or("").trim();
        if !line.contains("property") || !line.contains(':') {
            continue;
        }
        // The token name sits between the type's closing `>` and the `:`.
        let Some(after_type) = line.split_once("> ").map(|(_, r)| r) else {
            continue;
        };
        let Some((name, value)) = after_type.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_end_matches(';').trim();
        map.insert(name.trim().to_string(), value.to_string());
    }
    map
}

#[test]
fn breeze_dark_light_hexes() {
    let t = tokens();

    // (token, dark hex, light hex) — dark-mode ternary tokens.
    // Breeze roles (dark / light):
    //   View          #141618 / #ffffff      Window        #202326 / #eff0f1
    //   Button        #292c30 / #fcfcfc      Window BgAlt   #292c30 / #e3e5e7
    //   FgNormal      #fcfcfc / #232629      FgInactive     #a1a9b1 / #707d8a
    //   Link          #1d99f3 / #2980b9
    // Derived (no Breeze role): border/border-strong (frame shade),
    //   text-faint (inactive→bg).
    let ternary: &[(&str, &str, &str)] = &[
        ("surface", "202326", "eff0f1"),          // Window (card text/UI bg)
        ("surface2", "292c30", "fcfcfc"),         // Button
        ("surface-alt", "141618", "ffffff"),      // View (app bg / list panels)
        ("sidebar", "202326", "eff0f1"),          // Window (= surface)
        ("sidebar-text", "fcfcfc", "232629"),     // FgNormal
        ("sidebar-text-dim", "a1a9b1", "707d8a"), // FgInactive
        ("sidebar-hover", "292c30", "e3e5e7"),    // Window BgAlt
        ("border", "31363b", "bcc0c4"),           // derived frame
        ("border-strong", "45494f", "9fa4aa"),    // derived frame
        ("text", "fcfcfc", "232629"),             // FgNormal
        ("text-dim", "a1a9b1", "707d8a"),         // FgInactive
        ("text-faint", "6e757e", "9096a0"),       // derived
        ("info", "1d99f3", "2980b9"),             // Link
        ("chip", "292c30", "e3e5e7"),             // Window BgAlt
        ("chip-text", "fcfcfc", "232629"),        // FgNormal
        ("chip-border", "31363b", "bcc0c4"),      // = border
        ("row-hover", "292c30", "e3e5e7"),        // Window BgAlt
    ];
    for (name, dark, light) in ternary {
        let v = t
            .get(*name)
            .unwrap_or_else(|| panic!("token `{name}` not found in theme.slint"));
        let d = v
            .find(&format!("#{dark}"))
            .unwrap_or_else(|| panic!("`{name}` dark expected #{dark}, got: {v}"));
        let l = v
            .find(&format!("#{light}"))
            .unwrap_or_else(|| panic!("`{name}` light expected #{light}, got: {v}"));
        assert!(d < l, "`{name}`: dark value must precede light (got: {v})");
    }

    // Mode-invariant single-value tokens.
    //   ForegroundPositive/Negative/Neutral, plus the Breeze-blue accent
    //   default (Selection / DecorationFocus).
    let single: &[(&str, &str)] = &[
        ("success", "27ae60"), // ForegroundPositive
        ("danger", "da4453"),  // ForegroundNegative
        ("warn", "f67400"),    // ForegroundNeutral
        ("accent", "3daee9"),  // Selection / DecorationFocus (DE default)
    ];
    for (name, hex) in single {
        let v = t
            .get(*name)
            .unwrap_or_else(|| panic!("token `{name}` not found in theme.slint"));
        assert!(
            v.contains(&format!("#{hex}")),
            "`{name}` expected #{hex}, got: {v}"
        );
    }
}

/// The status `-soft` / `-text` variants must stay *derived* from their
/// base (a low-alpha wash + a brightened/darkened text), so re-basing the
/// hue in one place cascades. Guard the derivation structure.
#[test]
fn status_variants_derive_from_base() {
    let t = tokens();
    for base in ["success", "danger", "warn", "info"] {
        let soft = &t[&format!("{base}-soft")];
        assert!(
            soft.contains(&format!("{base}.with-alpha")),
            "`{base}-soft` should be `{base}.with-alpha(…)`, got: {soft}"
        );
        let text = &t[&format!("{base}-text")];
        // Direction matters: brighten in the dark branch (before `:`) and
        // darken in the light branch (after). A swap would darken on the
        // dark soft-wash (illegible), so assert the order, not just that
        // both appear.
        let br = text
            .find(&format!("{base}.brighter"))
            .unwrap_or_else(|| panic!("`{base}-text` should brighten `{base}`, got: {text}"));
        let dk = text
            .find(&format!("{base}.darker"))
            .unwrap_or_else(|| panic!("`{base}-text` should darken `{base}`, got: {text}"));
        assert!(
            br < dk,
            "`{base}-text` must brighten in dark / darken in light `{base}`, got: {text}"
        );
    }
    // Selection tracks the accent (Breeze's Selection role IS the accent).
    let sel = &t["row-selected"];
    assert!(
        sel.contains("accent.with-alpha"),
        "`row-selected` should be an accent wash, got: {sel}"
    );
    assert_eq!(
        t.get("row-selected-text").map(String::as_str),
        Some("text"),
        "`row-selected-text` should be the normal text color (unchanged on selection)"
    );
}
