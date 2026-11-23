//! The product's user-visible identity (Aether, owner decision D-284).
//!
//! Only what a person reads changes here. Storage keys (`kima-web.*`), crate names, environment variables and
//! wire values keep their legacy spelling until the technical rename stage
//! (`docs/superpowers/plans/2026-11-23-aether-rebrand.md`), so existing sessions and settings survive.

/// The name shown in the title, header, login and onboarding.
pub const PRODUCT_NAME: &str = "Aether";

/// The document title (the body is lowercase, the tab title follows it).
pub const DOCUMENT_TITLE: &str = "aether";

/// The browser chrome colour (`<meta name="theme-color">`): the Midnight Slate page background.
pub const THEME_COLOR: &str = "#0D1420";

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

    /// Stage 1 must not rename stored keys: a renamed key logs every user out.
    #[test]
    fn legacy_storage_keys_are_kept() {
        assert_eq!(crate::auth::STORAGE_KEY, "kima-web.session");
        assert_eq!(crate::position::STORAGE_KEY, "kima-web.position");
    }
}
