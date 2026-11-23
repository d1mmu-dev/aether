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
        let head = format!(":root[data-theme=\"{}\"] {{", theme.id());
        let start = CSS.find(&head).expect("theme block") + head.len();
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
