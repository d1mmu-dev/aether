//! Interface preferences: language and theme.
//!
//! Precedence: the signed-in profile's saved choice (`/api/settings` `uiLocale`/`uiTheme`, D-287) -> the
//! remembered device choice (`localStorage`, used before sign-in and for a profile that has not chosen)
//! -> the browser language (`navigator.language`) -> English / Midnight Slate. Only enum identifiers are
//! stored; tampered or unknown values fall back.
//!
//! The profile layer follows the session: every session change (sign-in, profile switch, sign-out, a new
//! token) starts a new generation that drops the previous profile's choice at once and shows the
//! device/browser fallback until the new profile's settings arrive. Answers and failed saves of an older
//! generation are ignored (on top of the API client's own session fence), so a late reply can never put
//! one profile's look on another. A choice made while signed in is saved to the profile only (the device
//! choice is not changed, so another profile or the login screen never inherits it); a failed save rolls
//! the choice back. SSR has no session or storage and always renders the deterministic default.
//!
//! The values are reactive and bound to `<html lang>` / `<html data-theme>` and to translated text, so a
//! change applies immediately without a reload and without touching the player.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use vertigo::{Computed, JsJson, Value, get_driver, js};

use crate::auth::Storage;
use crate::i18n::{Locale, Msg};
use crate::theme::Theme;

pub const LOCALE_KEY: &str = "aether-web.locale";
pub const THEME_KEY: &str = "aether-web.theme";

/// The active profile's saved choices (`None`: not chosen, use the fallback).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ProfileChoice {
    locale: Option<Locale>,
    theme: Option<Theme>,
}

/// Identifies a pending profile save: its session generation and the choice it replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveTicket<T> {
    generation: u64,
    previous: Option<T>,
    chosen: T,
}

#[derive(Clone)]
pub struct Prefs {
    pub locale: Value<Locale>,
    pub theme: Value<Theme>,
    storage: Rc<dyn Storage>,
    browser_tag: Option<String>,
    generation: Rc<Cell<u64>>,
    /// `Some` while a profile is signed in (its choices may still be loading).
    profile: Rc<RefCell<Option<ProfileChoice>>>,
}

impl Prefs {
    /// `browser_tag` is the browser's preferred language, if any. Starts signed out (device fallback);
    /// the session layer calls [`Prefs::begin_session`] once it knows whether a profile is signed in.
    pub fn new(storage: Rc<dyn Storage>, browser_tag: Option<String>) -> Self {
        let p = Prefs {
            locale: Value::new(Locale::default()),
            theme: Value::new(Theme::default()),
            storage,
            browser_tag,
            generation: Rc::new(Cell::new(0)),
            profile: Rc::new(RefCell::new(None)),
        };
        p.locale.set(p.fallback_locale());
        p.theme.set(p.fallback_theme());
        p
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

    fn fallback_locale(&self) -> Locale {
        self.storage
            .get(LOCALE_KEY)
            .and_then(|id| Locale::parse(&id))
            .or_else(|| self.browser_tag.as_deref().and_then(Locale::from_tag))
            .unwrap_or_default()
    }

    fn fallback_theme(&self) -> Theme {
        self.storage
            .get(THEME_KEY)
            .and_then(|id| Theme::parse(&id))
            .unwrap_or_default()
    }

    fn apply(&self) {
        let choice = (*self.profile.borrow()).unwrap_or_default();
        let locale = choice.locale.unwrap_or_else(|| self.fallback_locale());
        let theme = choice.theme.unwrap_or_else(|| self.fallback_theme());
        if vertigo::transaction(|c| self.locale.get(c)) != locale {
            self.locale.set(locale);
        }
        if vertigo::transaction(|c| self.theme.get(c)) != theme {
            self.theme.set(theme);
        }
    }

    /// A new session state (signed in or not). Drops the previous profile's choices immediately and
    /// returns the generation that the new profile's settings must carry.
    pub fn begin_session(&self, signed_in: bool) -> u64 {
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        *self.profile.borrow_mut() = signed_in.then(ProfileChoice::default);
        self.apply();
        generation
    }

    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// The signed-in profile's saved ids (from `/api/settings`). Ignored when `generation` is stale or
    /// nobody is signed in; unknown ids count as "not chosen". Returns whether it was applied.
    pub fn apply_profile(
        &self,
        generation: u64,
        locale: Option<&str>,
        theme: Option<&str>,
    ) -> bool {
        if generation != self.generation.get() || self.profile.borrow().is_none() {
            return false;
        }
        *self.profile.borrow_mut() = Some(ProfileChoice {
            locale: locale.and_then(Locale::parse),
            theme: theme.and_then(Theme::parse),
        });
        self.apply();
        true
    }

    /// Applies `locale` at once. Signed out it is the device choice (stored, nothing to send); signed in
    /// it is the profile's and the caller must save it, then call [`Prefs::rollback_locale`] on failure.
    pub fn set_locale(&self, locale: Locale) -> Option<SaveTicket<Locale>> {
        let ticket = {
            let mut profile = self.profile.borrow_mut();
            match profile.as_mut() {
                Some(choice) => {
                    let previous = choice.locale.replace(locale);
                    Some(SaveTicket {
                        generation: self.generation.get(),
                        previous,
                        chosen: locale,
                    })
                }
                None => {
                    self.storage.set(LOCALE_KEY, locale.id());
                    None
                }
            }
        };
        self.apply();
        ticket
    }

    /// See [`Prefs::set_locale`].
    pub fn set_theme(&self, theme: Theme) -> Option<SaveTicket<Theme>> {
        let ticket = {
            let mut profile = self.profile.borrow_mut();
            match profile.as_mut() {
                Some(choice) => {
                    let previous = choice.theme.replace(theme);
                    Some(SaveTicket {
                        generation: self.generation.get(),
                        previous,
                        chosen: theme,
                    })
                }
                None => {
                    self.storage.set(THEME_KEY, theme.id());
                    None
                }
            }
        };
        self.apply();
        ticket
    }

    /// A failed save: restores the replaced choice if the session and the choice are still the ones the
    /// save was for (a newer choice or another profile is left alone). Returns whether it rolled back.
    pub fn rollback_locale(&self, ticket: SaveTicket<Locale>) -> bool {
        self.rollback(ticket, |c| &mut c.locale)
    }

    /// See [`Prefs::rollback_locale`].
    pub fn rollback_theme(&self, ticket: SaveTicket<Theme>) -> bool {
        self.rollback(ticket, |c| &mut c.theme)
    }

    fn rollback<T: Copy + PartialEq>(
        &self,
        ticket: SaveTicket<T>,
        field: impl Fn(&mut ProfileChoice) -> &mut Option<T>,
    ) -> bool {
        if ticket.generation != self.generation.get() {
            return false;
        }
        let rolled = match self.profile.borrow_mut().as_mut() {
            Some(choice) => {
                let slot = field(choice);
                let current = *slot == Some(ticket.chosen);
                if current {
                    *slot = ticket.previous;
                }
                current
            }
            None => false,
        };
        if rolled {
            self.apply();
        }
        rolled
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

    fn device(locale: &str, theme: &str) -> Rc<MemoryStorage> {
        let storage = Rc::new(MemoryStorage::default());
        storage.set(LOCALE_KEY, locale);
        storage.set(THEME_KEY, theme);
        storage
    }

    #[test]
    fn server_accepts_exactly_these_locale_ids() {
        let ids: Vec<&str> = Locale::ALL.iter().map(|l| l.id()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(sorted, aether_api_types::settings::UI_LOCALES);
    }

    #[test]
    fn profile_choice_wins_over_device_browser_and_defaults() {
        let p = Prefs::new(device("en", "graphite"), Some("pl-PL".into()));
        assert_eq!((locale(&p), theme(&p)), (Locale::En, Theme::Graphite));
        let generation = p.begin_session(true);
        // Until the profile's settings arrive, the device choice shows.
        assert_eq!((locale(&p), theme(&p)), (Locale::En, Theme::Graphite));
        assert!(p.apply_profile(generation, Some("pl"), Some("daylight")));
        assert_eq!((locale(&p), theme(&p)), (Locale::Pl, Theme::Daylight));
        // A profile that has not chosen (or a tampered id) falls back per field.
        let generation = p.begin_session(true);
        assert!(p.apply_profile(generation, None, Some("url(evil)")));
        assert_eq!((locale(&p), theme(&p)), (Locale::En, Theme::Graphite));
        // Without a device choice: browser language, then the defaults.
        let bare = Prefs::new(Rc::new(MemoryStorage::default()), Some("pl".into()));
        let generation = bare.begin_session(true);
        assert!(bare.apply_profile(generation, None, None));
        assert_eq!(
            (locale(&bare), theme(&bare)),
            (Locale::Pl, Theme::MidnightSlate)
        );
    }

    #[test]
    fn switching_profile_never_shows_the_previous_profiles_choice() {
        let p = Prefs::new(Rc::new(MemoryStorage::default()), None);
        let a = p.begin_session(true);
        p.apply_profile(a, Some("pl"), Some("daylight"));
        let b = p.begin_session(true);
        // Immediately on the switch: B's look is the fallback, not A's.
        assert_eq!((locale(&p), theme(&p)), (Locale::En, Theme::MidnightSlate));
        // A's settings answer arriving late after the switch is ignored.
        assert!(!p.apply_profile(a, Some("pl"), Some("daylight")));
        assert_eq!((locale(&p), theme(&p)), (Locale::En, Theme::MidnightSlate));
        assert!(p.apply_profile(b, None, Some("graphite")));
        assert_eq!((locale(&p), theme(&p)), (Locale::En, Theme::Graphite));
    }

    #[test]
    fn sign_out_returns_to_the_device_choice_and_ignores_late_answers() {
        let storage = device("pl", "graphite");
        let p = Prefs::new(storage.clone(), None);
        let g = p.begin_session(true);
        p.apply_profile(g, Some("en"), Some("daylight"));
        p.begin_session(false);
        assert_eq!((locale(&p), theme(&p)), (Locale::Pl, Theme::Graphite));
        assert!(!p.apply_profile(g, Some("en"), Some("daylight")));
        // Signed out there is no profile to apply to, even with the current generation.
        assert!(!p.apply_profile(p.generation(), Some("en"), Some("daylight")));
        assert_eq!((locale(&p), theme(&p)), (Locale::Pl, Theme::Graphite));
        assert_eq!(
            storage.get(THEME_KEY).as_deref(),
            Some("graphite"),
            "the device choice survives"
        );
    }

    #[test]
    fn signed_in_choice_is_the_profiles_and_does_not_change_the_device() {
        let storage = device("en", "midnight-slate");
        let p = Prefs::new(storage.clone(), None);
        let g = p.begin_session(true);
        p.apply_profile(g, None, None);
        let ticket = p
            .set_theme(Theme::Daylight)
            .expect("signed in: the caller saves it");
        assert_eq!(theme(&p), Theme::Daylight, "applies at once");
        assert_eq!(storage.get(THEME_KEY).as_deref(), Some("midnight-slate"));
        assert!(p.set_locale(Locale::Pl).is_some());
        let _ = ticket;
        // Signed out, a choice is the device's and needs no save.
        p.begin_session(false);
        assert!(p.set_locale(Locale::Pl).is_none());
        assert_eq!(storage.get(LOCALE_KEY).as_deref(), Some("pl"));
    }

    #[test]
    fn failed_save_rolls_back_only_its_own_choice_in_its_own_session() {
        let p = Prefs::new(Rc::new(MemoryStorage::default()), None);
        let g = p.begin_session(true);
        p.apply_profile(g, Some("en"), Some("graphite"));
        let ticket = p.set_theme(Theme::Daylight).unwrap();
        assert!(p.rollback_theme(ticket));
        assert_eq!(theme(&p), Theme::Graphite);
        // A newer choice made while the first save was pending is kept.
        let first = p.set_theme(Theme::Daylight).unwrap();
        let _second = p.set_theme(Theme::MidnightSlate).unwrap();
        assert!(!p.rollback_theme(first));
        assert_eq!(theme(&p), Theme::MidnightSlate);
        // A failure arriving after a profile switch does not touch the new profile.
        let pending = p.set_locale(Locale::Pl).unwrap();
        let b = p.begin_session(true);
        p.apply_profile(b, Some("pl"), None);
        assert!(!p.rollback_locale(pending));
        assert_eq!(locale(&p), Locale::Pl);
        // Rolling back to "not chosen" shows the fallback again.
        let fresh = p.begin_session(true);
        p.apply_profile(fresh, None, None);
        let t = p.set_locale(Locale::Pl).unwrap();
        assert!(p.rollback_locale(t));
        assert_eq!(locale(&p), Locale::En);
    }
}
