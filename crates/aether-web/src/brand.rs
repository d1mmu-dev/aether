//! The product's user-visible identity (Aether, owner decision D-284).
//!
//! Since the technical rename (stage 2 of `docs/superpowers/plans/2026-11-23-aether-rebrand.md`) the storage
//! keys are `aether-web.*` as well; the old keys are not read (no legacy compatibility, users sign in again).

/// The name shown in the title, header, login and onboarding.
pub const PRODUCT_NAME: &str = "Aether";

/// The document title (the body is lowercase, the tab title follows it).
pub const DOCUMENT_TITLE: &str = "aether";

/// The footer line under the login and onboarding cards.
pub const FOOTER: &str = "© 2026 Aether. Your music, your way.";

#[cfg(test)]
mod tests {
    use super::*;

    /// Sources that render the brand; none of them may still show the old product name.
    const BRANDED_SOURCES: &[(&str, &str)] = &[
        ("app.rs", include_str!("app.rs")),
        ("shell.rs", include_str!("shell.rs")),
        ("pages/login.rs", include_str!("pages/login.rs")),
        ("pages/onboarding.rs", include_str!("pages/onboarding.rs")),
    ];

    /// `<meta name="theme-color">` follows the selected theme instead of a fixed Midnight Slate value.
    #[test]
    fn browser_chrome_colour_follows_the_theme() {
        let app = include_str!("app.rs");
        assert!(app.contains(r#"<meta name="theme-color" content={theme_color} />"#));
        // Compared without whitespace so rustfmt's line breaks do not matter.
        let compact: String = app.chars().filter(|c| !c.is_whitespace()).collect();
        assert!(compact.contains("ctx.prefs.theme.to_computed().map(|t|t.swatch().0"));
    }

    #[test]
    fn branded_views_show_no_legacy_product_name() {
        let legacy = [
            "\"Kima\"",
            "\"Kima Hub\"",
            "\"kima\"",
            "continue to Kima",
            "your Kima server",
            "2026 Kima.",
        ];
        for (file, src) in BRANDED_SOURCES {
            for text in legacy {
                assert!(!src.contains(text), "{file} still renders {text}");
            }
        }
    }

    #[test]
    fn branded_views_use_the_product_constants() {
        for (file, src) in BRANDED_SOURCES {
            assert!(
                src.contains("PRODUCT_NAME")
                    || src.contains("DOCUMENT_TITLE")
                    || src.contains("FOOTER"),
                "{file} does not use the brand constants"
            );
        }
        assert_eq!(PRODUCT_NAME, "Aether");
        assert_eq!(DOCUMENT_TITLE, PRODUCT_NAME.to_lowercase());
        assert!(FOOTER.contains(PRODUCT_NAME));
    }

    /// Owner-approved Midnight Slate tokens (lowercase hex: Tailwind's minifier lowercases them).
    #[test]
    fn stylesheet_carries_the_midnight_slate_tokens() {
        let css = crate::style::stylesheet();
        for token in [
            "--bg-primary:#0d1420",
            "--bg-secondary:#151f2e",
            "--border-subtle:#223047",
            "--color-brand:#8bafd1",
            "--text-primary:#dfe7ef",
            "--text-secondary:#9aaaba",
            "--color-black:#0d1420",
            "--color-white:#dfe7ef",
        ] {
            assert!(css.contains(token), "stylesheet lacks {token}");
        }
        assert!(
            !css.contains("--color-brand:#fca200"),
            "amber brand token is still set"
        );
    }

    /// Statuses keep their meaning: the rebrand must not recolour success/warning/danger.
    #[test]
    fn status_colours_keep_their_meaning() {
        let css = crate::style::stylesheet();
        for token in [
            "--color-success:#22c55e",
            "--color-warning:#eab308",
            "--color-danger:#ef4444",
        ] {
            assert!(css.contains(token), "stylesheet lacks {token}");
        }
    }

    #[test]
    fn the_logo_mask_is_the_aether_mark() {
        let css = crate::style::stylesheet();
        assert!(css.contains("data:image/svg+xml;base64,"));
        assert!(
            !css.contains("data:image/webp;base64,"),
            "the legacy Kima mark is still embedded"
        );
        assert!(!crate::style::BRAND_GRADIENT.contains("#fca506"));
    }

    /// Stage 2 moved every stored key to `aether-web.*`; nothing reads the pre-rebrand keys.
    #[test]
    fn storage_keys_carry_the_aether_prefix() {
        assert_eq!(crate::auth::STORAGE_KEY, "aether-web.session");
        assert_eq!(crate::position::STORAGE_KEY, "aether-web.position");
        for src in [include_str!("auth.rs"), include_str!("position.rs")] {
            for legacy in ["kima", "Kima"] {
                assert!(
                    !src.contains(legacy),
                    "a storage source still names {legacy}"
                );
            }
        }
    }
}
