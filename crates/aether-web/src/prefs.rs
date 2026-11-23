//! Interface preferences: language and theme.
//!
//! This first slice keeps them per DEVICE (`localStorage`, non-sensitive enum identifiers only). Per-profile
//! persistence through the profile settings API is the plan's task 4 and is not done yet. Precedence today:
//! stored device choice -> browser language (`navigator.language`) -> English / Midnight Slate. The server has
//! no storage, so SSR always renders the deterministic default and the browser applies the stored choice on
//! start (a remaining first-paint limitation, see the plan).
//!
//! The values are reactive and bound to `<html lang>` / `<html data-theme>` and to translated text, so a
//! change applies immediately without a reload and without touching the player.

use std::rc::Rc;

use vertigo::{Computed, JsJson, Value, get_driver, js};

use crate::auth::Storage;
use crate::i18n::{Locale, Msg};
use crate::theme::Theme;

pub const LOCALE_KEY: &str = "aether-web.locale";
pub const THEME_KEY: &str = "aether-web.theme";

#[derive(Clone)]
pub struct Prefs {
    pub locale: Value<Locale>,
    pub theme: Value<Theme>,
    storage: Rc<dyn Storage>,
}

impl Prefs {
    /// `browser_tag` is the browser's preferred language, if any.
    pub fn new(storage: Rc<dyn Storage>, browser_tag: Option<String>) -> Self {
        let locale = storage
            .get(LOCALE_KEY)
            .and_then(|id| Locale::parse(&id))
            .or_else(|| browser_tag.as_deref().and_then(Locale::from_tag))
            .unwrap_or_default();
        let theme = storage
            .get(THEME_KEY)
            .and_then(|id| Theme::parse(&id))
            .unwrap_or_default();
        Prefs {
            locale: Value::new(locale),
            theme: Value::new(theme),
            storage,
        }
    }

    /// The browser's `navigator.language` (None while rendering on the server).
    pub fn browser_tag() -> Option<String> {
        if !get_driver().is_browser() {
            return None;
        }
        match js! { window.navigator.language } {
            JsJson::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn set_locale(&self, locale: Locale) {
        self.storage.set(LOCALE_KEY, locale.id());
        self.locale.set(locale);
    }

    pub fn set_theme(&self, theme: Theme) {
        self.storage.set(THEME_KEY, theme.id());
        self.theme.set(theme);
    }

    /// A message that follows the language.
    pub fn text(&self, msg: Msg) -> Computed<String> {
        self.locale
            .to_computed()
            .map(move |l| msg.text(l).to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::MemoryStorage;
    use vertigo::transaction;

    fn locale(p: &Prefs) -> Locale {
        transaction(|c| p.locale.get(c))
    }
    fn theme(p: &Prefs) -> Theme {
        transaction(|c| p.theme.get(c))
    }

    #[test]
    fn defaults_without_storage_or_browser_language() {
        let p = Prefs::new(Rc::new(MemoryStorage::default()), None);
        assert_eq!((locale(&p), theme(&p)), (Locale::En, Theme::MidnightSlate));
    }

    #[test]
    fn browser_language_is_used_until_a_choice_is_stored() {
        let storage = Rc::new(MemoryStorage::default());
        let p = Prefs::new(storage.clone(), Some("pl-PL".into()));
        assert_eq!(locale(&p), Locale::Pl);
        p.set_locale(Locale::En);
        let again = Prefs::new(storage.clone(), Some("pl-PL".into()));
        assert_eq!(
            locale(&again),
            Locale::En,
            "stored choice wins over the browser"
        );
        assert_eq!(storage.get(LOCALE_KEY).as_deref(), Some("en"));
        let unsupported = Prefs::new(Rc::new(MemoryStorage::default()), Some("de-DE".into()));
        assert_eq!(locale(&unsupported), Locale::En);
    }

    #[test]
    fn theme_round_trips_and_tampered_values_fall_back() {
        let storage = Rc::new(MemoryStorage::default());
        Prefs::new(storage.clone(), None).set_theme(Theme::Daylight);
        assert_eq!(theme(&Prefs::new(storage.clone(), None)), Theme::Daylight);
        storage.set(THEME_KEY, "url(evil)");
        storage.set(LOCALE_KEY, "xx");
        let p = Prefs::new(storage, Some("pl".into()));
        assert_eq!((locale(&p), theme(&p)), (Locale::Pl, Theme::MidnightSlate));
    }

    #[test]
    fn text_follows_the_language() {
        let p = Prefs::new(Rc::new(MemoryStorage::default()), None);
        let title = p.text(Msg::SettingsTitle);
        assert_eq!(transaction(|c| title.get(c)), "Settings");
        p.set_locale(Locale::Pl);
        assert_eq!(transaction(|c| title.get(c)), "Ustawienia");
    }
}
