//! The interface language: a typed message catalogue for English and Polish.
//!
//! Decision (localization plan, task 1): the first slice needs plain labels, one plural rule and no runtime
//! catalogue loading, so messages are a Rust enum with both translations side by side. That keeps key
//! parity a compile-time property (every [`Msg`] arm must name both texts), adds no dependency to the WASM
//! bundle and needs no general-purpose framework. Fluent remains the candidate once messages need
//! translator-editable files or many named arguments; see the plan's ADR note.
//!
//! Only user-interface text is translated. Library metadata, user content and server error bodies are shown
//! as received.

/// A shipped interface language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Locale {
    #[default]
    En,
    Pl,
}

impl Locale {
    pub const ALL: [Locale; 2] = [Locale::Pl, Locale::En];

    /// The stored and `<html lang>` identifier.
    pub fn id(self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Pl => "pl",
        }
    }

    /// A stored identifier; anything else is rejected rather than guessed.
    pub fn parse(id: &str) -> Option<Locale> {
        match id {
            "en" => Some(Locale::En),
            "pl" => Some(Locale::Pl),
            _ => None,
        }
    }

    /// A browser language tag (`pl-PL`, `en_GB`, `PL`): matched by its primary subtag, unsupported -> None.
    pub fn from_tag(tag: &str) -> Option<Locale> {
        let primary = tag.split(['-', '_']).next()?.trim().to_ascii_lowercase();
        Locale::parse(&primary)
    }

    /// The language's own name, shown in the selector in every interface language.
    pub fn native_name(self) -> &'static str {
        match self {
            Locale::En => "English",
            Locale::Pl => "Polski",
        }
    }
}

/// Message keys, grouped by domain. Never use translated text as an identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Msg {
    // navigation
    NavCollection,
    NavRadio,
    NavSearch,
    NavPlaylists,
    NavPodcasts,
    NavVibe,
    // settings
    SettingsEyebrow,
    SettingsTitle,
    SettingsSections,
    SectionAccount,
    SectionAppearance,
    Language,
    LanguageHint,
    Theme,
    ThemeHint,
    DeviceOnly,
}

impl Msg {
    #[cfg(test)]
    pub const ALL: [Msg; 16] = [
        Msg::NavCollection,
        Msg::NavRadio,
        Msg::NavSearch,
        Msg::NavPlaylists,
        Msg::NavPodcasts,
        Msg::NavVibe,
        Msg::SettingsEyebrow,
        Msg::SettingsTitle,
        Msg::SettingsSections,
        Msg::SectionAccount,
        Msg::SectionAppearance,
        Msg::Language,
        Msg::LanguageHint,
        Msg::Theme,
        Msg::ThemeHint,
        Msg::DeviceOnly,
    ];

    /// `(English, Polish)`.
    fn texts(self) -> (&'static str, &'static str) {
        match self {
            Msg::NavCollection => ("Collection", "Kolekcja"),
            Msg::NavRadio => ("Radio", "Radio"),
            Msg::NavSearch => ("Search", "Szukaj"),
            Msg::NavPlaylists => ("Playlists", "Playlisty"),
            Msg::NavPodcasts => ("Podcasts", "Podcasty"),
            Msg::NavVibe => ("Vibe", "Klimat"),
            Msg::SettingsEyebrow => ("System configuration", "Konfiguracja systemu"),
            Msg::SettingsTitle => ("Settings", "Ustawienia"),
            Msg::SettingsSections => ("Settings sections", "Sekcje ustawień"),
            Msg::SectionAccount => ("Account", "Konto"),
            Msg::SectionAppearance => ("Appearance & language", "Wygląd i język"),
            Msg::Language => ("Language", "Język"),
            Msg::LanguageHint => ("Interface language", "Język interfejsu"),
            Msg::Theme => ("Theme", "Motyw"),
            Msg::ThemeHint => ("Colours of the whole app", "Kolory całej aplikacji"),
            Msg::DeviceOnly => ("Saved on this device", "Zapisano na tym urządzeniu"),
        }
    }

    pub fn text(self, locale: Locale) -> &'static str {
        let (en, pl) = self.texts();
        match locale {
            Locale::En => en,
            Locale::Pl => pl,
        }
    }
}

/// Polish plural category for a whole count: `One` (1), `Few` (2-4, 22-24, ... but not 12-14), `Many`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlPlural {
    One,
    Few,
    Many,
}

pub fn pl_plural(n: u64) -> PlPlural {
    let (units, tens) = (n % 10, n % 100);
    if n == 1 {
        PlPlural::One
    } else if (2..=4).contains(&units) && !(12..=14).contains(&tens) {
        PlPlural::Few
    } else {
        PlPlural::Many
    }
}

/// A complete "N tracks" phrase (whole sentences per locale, no concatenated noun fragments).
// First consumer: the library count labels of the next translation slice.
#[allow(dead_code)]
pub fn track_count(locale: Locale, n: u64) -> String {
    match locale {
        Locale::En if n == 1 => "1 track".to_owned(),
        Locale::En => format!("{n} tracks"),
        Locale::Pl => match pl_plural(n) {
            PlPlural::One => "1 utwór".to_owned(),
            PlPlural::Few => format!("{n} utwory"),
            PlPlural::Many => format!("{n} utworów"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_ids_round_trip_and_unknown_is_rejected() {
        for l in Locale::ALL {
            assert_eq!(Locale::parse(l.id()), Some(l));
        }
        assert_eq!(Locale::parse("de"), None);
        assert_eq!(Locale::parse("PL"), None);
        assert_eq!(Locale::parse(""), None);
    }

    #[test]
    fn browser_tags_match_by_primary_subtag() {
        assert_eq!(Locale::from_tag("pl-PL"), Some(Locale::Pl));
        assert_eq!(Locale::from_tag("PL"), Some(Locale::Pl));
        assert_eq!(Locale::from_tag("en_GB"), Some(Locale::En));
        assert_eq!(Locale::from_tag("de-DE"), None);
        assert_eq!(Locale::from_tag(""), None);
    }

    #[test]
    fn every_message_is_translated_and_has_no_raw_key() {
        for m in Msg::ALL {
            for l in Locale::ALL {
                let t = m.text(l);
                assert!(!t.trim().is_empty(), "{m:?} empty in {l:?}");
                // A compound key (`SettingsTitle`) must never leak; one-word keys may equal their English text.
                let key = format!("{m:?}");
                if key.chars().filter(char::is_ascii_uppercase).count() > 1 {
                    assert!(!t.contains(&key), "{m:?} shows its key in {l:?}");
                }
                assert!(!t.contains('<'), "{m:?} must stay plain text");
            }
        }
        assert_eq!(Msg::SettingsTitle.text(Locale::Pl), "Ustawienia");
    }

    #[test]
    fn polish_plural_categories() {
        use PlPlural::*;
        for (n, want) in [
            (0, Many),
            (1, One),
            (2, Few),
            (4, Few),
            (5, Many),
            (12, Many),
            (14, Many),
            (21, Many),
            (22, Few),
            (25, Many),
            (101, Many),
            (112, Many),
            (122, Few),
        ] {
            assert_eq!(pl_plural(n), want, "n = {n}");
        }
    }

    #[test]
    fn track_count_sentences() {
        assert_eq!(track_count(Locale::Pl, 1), "1 utwór");
        assert_eq!(track_count(Locale::Pl, 2), "2 utwory");
        assert_eq!(track_count(Locale::Pl, 5), "5 utworów");
        assert_eq!(track_count(Locale::Pl, 12), "12 utworów");
        assert_eq!(track_count(Locale::Pl, 22), "22 utwory");
        assert_eq!(track_count(Locale::Pl, 25), "25 utworów");
        assert_eq!(track_count(Locale::Pl, 0), "0 utworów");
        assert_eq!(track_count(Locale::En, 1), "1 track");
        assert_eq!(track_count(Locale::En, 0), "0 tracks");
        assert_eq!(track_count(Locale::En, 2), "2 tracks");
    }
}
