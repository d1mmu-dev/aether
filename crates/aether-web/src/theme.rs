//! Bundled colour themes (localization/themes plan, task 3).
//!
//! A theme is a validated identifier put on `<html data-theme>`; `assets/themes.css` maps it to the semantic
//! tokens. Midnight Slate is the default (owner decision D-284). Nothing here loads user CSS or remote files.

/// The CSS appended after the compiled stylesheet.
pub const CSS: &str = include_str!("../assets/themes.css");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Theme {
    #[default]
    MidnightSlate,
    Graphite,
    Daylight,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::MidnightSlate, Theme::Graphite, Theme::Daylight];

    /// The stored and `data-theme` identifier.
    pub fn id(self) -> &'static str {
        match self {
            Theme::MidnightSlate => "midnight-slate",
            Theme::Graphite => "graphite",
            Theme::Daylight => "daylight",
        }
    }

    pub fn parse(id: &str) -> Option<Theme> {
        Theme::ALL.into_iter().find(|t| t.id() == id)
    }

    /// Theme names are product names and stay the same in every interface language.
    pub fn name(self) -> &'static str {
        match self {
            Theme::MidnightSlate => "Midnight Slate",
            Theme::Graphite => "Graphite",
            Theme::Daylight => "Daylight",
        }
    }

    /// `(ground, accent, text)` for the selector's swatch.
    pub fn swatch(self) -> (&'static str, &'static str, &'static str) {
        match self {
            Theme::MidnightSlate => ("#0d1420", "#8bafd1", "#dfe7ef"),
            Theme::Graphite => ("#121212", "#a9b8c6", "#e6e6e6"),
            Theme::Daylight => ("#f5f7fa", "#335b85", "#1c2530"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIRED: &[&str] = &[
        "--color-black",
        "--color-white",
        "--color-brand",
        "--color-brand-hover",
        "--color-brand-light",
        "--color-brand-dark",
        "--color-on-brand",
        "--color-ambient",
        "--color-danger-light",
        "--bg-primary",
        "--bg-secondary",
        "--bg-tertiary",
        "--bg-hover",
        "--border-subtle",
        "--border-interactive",
        "--border-focus",
        "--text-primary",
        "--text-secondary",
        "--text-muted",
    ];

    /// The `name: #rrggbb` declarations of one theme block.
    fn tokens(theme: Theme) -> Vec<(String, [f64; 3])> {
        let head = format!(":root[data-theme=\"{}\"]", theme.id());
        let at = CSS.find(&head).expect("theme block");
        let start = at + CSS[at..].find('{').unwrap() + 1;
        let body = &CSS[start..start + CSS[start..].find('}').unwrap()];
        body.split(';')
            .filter_map(|decl| {
                let (name, value) = decl.split_once(':')?;
                let hex = value.trim().strip_prefix('#')?;
                let c =
                    |i: usize| f64::from(u8::from_str_radix(&hex[i..i + 2], 16).unwrap()) / 255.0;
                Some((name.trim().to_owned(), [c(0), c(2), c(4)]))
            })
            .collect()
    }

    fn get(theme: Theme, name: &str) -> [f64; 3] {
        tokens(theme)
            .into_iter()
            .find(|(n, _)| n == name)
            .unwrap_or_else(|| panic!("{} lacks {name}", theme.id()))
            .1
    }

    fn luminance(c: [f64; 3]) -> f64 {
        let lin = |v: f64| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c[0]) + 0.7152 * lin(c[1]) + 0.0722 * lin(c[2])
    }

    fn contrast(a: [f64; 3], b: [f64; 3]) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// `top` at `alpha` over `under` (sRGB source-over; the `#rrggbbaa` fallback the compiled CSS also ships).
    fn over(top: [f64; 3], alpha: f64, under: [f64; 3]) -> [f64; 3] {
        [0, 1, 2].map(|i| top[i] * alpha + under[i] * (1.0 - alpha))
    }

    fn hex(s: &str) -> [f64; 3] {
        let c = |i: usize| f64::from(u8::from_str_radix(&s[i..i + 2], 16).unwrap()) / 255.0;
        [c(1), c(3), c(5)]
    }

    /// A token as seen inside the Vibe scene: the `.vibe-sky` scene rule's value if it declares one, else the
    /// theme's.
    fn in_scene(theme: Theme, name: &str) -> [f64; 3] {
        let scene = CSS.find(".vibe-sky").map(|at| {
            let start = at + CSS[at..].find('{').unwrap() + 1;
            &CSS[start..start + CSS[start..].find('}').unwrap()]
        });
        scene
            .and_then(|body| {
                body.split(';').find_map(|decl| {
                    let (n, v) = decl.split_once(':')?;
                    (n.trim() == name).then(|| hex(v.trim()))
                })
            })
            .unwrap_or_else(|| get(theme, name))
    }

    /// The Vibe map is a night sky in every theme (`.vibe-sky`/`.vibe-glass` in aether-base.css are fixed dark
    /// navy), so the text over it must read on those surfaces whatever the app theme is.
    #[test]
    fn vibe_scene_text_meets_wcag_aa_on_sky_and_glass_in_every_theme() {
        const BASE: &str = include_str!("../assets/aether-base.css");
        // Literal surfaces from aether-base.css; this fails if they drift so the colours below stay real.
        for decl in [
            "#16233a 0%, #0d1420 25%, #090e17 48%, #05080e 68%",
            "background: rgba(9, 14, 23, 0.66);",
            "background: rgba(13, 20, 32, 0.94);",
        ] {
            assert!(BASE.contains(decl), "aether-base.css changed: {decl}");
        }
        let sky_light = hex("#16233a");
        let surfaces = [
            ("sky", sky_light),
            ("sky edge", hex("#05080e")),
            ("glass", over(hex("#090e17"), 0.66, sky_light)),
            ("solid glass", over(hex("#0d1420"), 0.94, sky_light)),
        ];
        for t in Theme::ALL {
            let white = in_scene(t, "--color-white");
            for (surface, bg) in surfaces {
                let texts = [
                    ("text-white", white),
                    ("text-white/60", over(white, 0.60, bg)),
                    ("--text-primary", in_scene(t, "--text-primary")),
                    ("--text-secondary", in_scene(t, "--text-secondary")),
                    ("--text-muted", in_scene(t, "--text-muted")),
                ];
                for (text, fg) in texts {
                    let ratio = contrast(fg, bg);
                    assert!(
                        ratio >= 4.5,
                        "{}: {text} on Vibe {surface} is {ratio:.2}",
                        t.id()
                    );
                }
                // Caret, spinner and active chips use the brand as a non-text indicator.
                let brand = contrast(in_scene(t, "--color-brand"), bg);
                assert!(
                    brand >= 3.0,
                    "{}: brand on Vibe {surface} is {brand:.2}",
                    t.id()
                );
            }
        }
    }

    #[test]
    fn ids_round_trip_and_unknown_is_rejected() {
        for t in Theme::ALL {
            assert_eq!(Theme::parse(t.id()), Some(t));
        }
        for bad in ["", "Graphite", "tokyo-night", "midnight-slate;", "../x"] {
            assert_eq!(Theme::parse(bad), None, "{bad}");
        }
        assert_eq!(Theme::default(), Theme::MidnightSlate);
    }

    #[test]
    fn every_theme_declares_every_required_token() {
        for t in Theme::ALL {
            for name in REQUIRED {
                get(t, name);
            }
            let (ground, accent, text) = t.swatch();
            let hex = |c: [f64; 3]| {
                format!(
                    "#{:02x}{:02x}{:02x}",
                    (c[0] * 255.0).round() as u8,
                    (c[1] * 255.0).round() as u8,
                    (c[2] * 255.0).round() as u8
                )
            };
            assert_eq!(hex(get(t, "--bg-primary")), ground);
            assert_eq!(hex(get(t, "--color-brand")), accent);
            assert_eq!(hex(get(t, "--text-primary")), text);
        }
    }

    #[test]
    fn theme_css_cannot_load_anything() {
        for banned in ["@import", "url(", "expression(", "javascript:", "<"] {
            assert!(!CSS.contains(banned), "themes.css contains {banned}");
        }
    }

    #[test]
    fn text_and_controls_meet_wcag_aa_on_actual_surfaces() {
        for t in Theme::ALL {
            let ground = get(t, "--bg-primary");
            let white = get(t, "--color-white");
            // Surfaces the app draws: the ground, a `white/10` hover/field overlay and the elevated tertiary.
            let surfaces = [
                ("ground", ground),
                ("white/10 over ground", over(white, 0.10, ground)),
                ("bg-tertiary", get(t, "--bg-tertiary")),
            ];
            for (surface, bg) in surfaces {
                for (token, min) in [
                    ("--text-primary", 4.5),
                    ("--text-secondary", 4.5),
                    ("--text-muted", 4.5),
                    ("--color-brand", 4.5),
                    ("--color-danger-light", 4.5),
                ] {
                    let ratio = contrast(get(t, token), bg);
                    assert!(
                        ratio >= min,
                        "{}: {token} on {surface} is {ratio:.2}",
                        t.id()
                    );
                }
            }
            let on_brand = contrast(get(t, "--color-on-brand"), get(t, "--color-brand"));
            assert!(on_brand >= 4.5, "{}: on-brand {on_brand:.2}", t.id());
            // The focus ring (`:focus-visible` outline = brand) is a non-text indicator: 3:1.
            assert!(contrast(get(t, "--color-brand"), ground) >= 3.0);
        }
    }
}
