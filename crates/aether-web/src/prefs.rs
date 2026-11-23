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
//! Profile changes are serialized: while the profile's settings load or a save is pending, a new choice is
//! refused ([`Change::Busy`], the selectors are disabled) and only the load of the current generation may
//! apply the server's values. So the initial answer can never overwrite a newer choice and two saves can
//! never reach the server out of order; a save or load of an older session never unlocks the new one.
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

/// Whether a profile choice can be made now (see the module notes on serialization).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Ready,
    /// The signed-in profile's settings are loading.
    Loading,
    /// A profile save is pending.
    Saving,
}

/// Identifies a pending profile save: its session generation and the choice it replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveTicket<T> {
    generation: u64,
    save: u64,
    previous: Option<T>,
    chosen: T,
}

/// The outcome of a choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change<T> {
    /// Signed out: applied and remembered on this device; nothing to save.
    Device,
    /// Signed in: applied; the caller saves it, then calls `finish_locale`/`finish_theme`.
    Save(SaveTicket<T>),
    /// Refused: the profile's settings are still loading or another save is pending.
    Busy,
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
    phase: Value<Phase>,
    /// Counts profile saves; only the pending one's ticket may finish it.
    saves: Rc<Cell<u64>>,
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
            phase: Value::new(Phase::Ready),
            saves: Rc::new(Cell::new(0)),
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
        self.set_phase(if signed_in {
            Phase::Loading
        } else {
            Phase::Ready
        });
        self.apply();
        generation
    }

    fn phase(&self) -> Phase {
        vertigo::transaction(|c| self.phase.get(c))
    }

    fn set_phase(&self, phase: Phase) {
        if self.phase() != phase {
            self.phase.set(phase);
        }
    }

    /// True while a choice would be refused (settings loading or a save pending): disables the selectors.
    pub fn busy(&self) -> Computed<bool> {
        self.phase.to_computed().map(|p| p != Phase::Ready)
    }

    #[cfg(test)]
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    /// The signed-in profile's saved ids (from `/api/settings`), ending the load. Ignored unless it is the
    /// load of the current generation (stale, signed out, a repeated answer); unknown ids count as "not
    /// chosen". Returns whether it was applied.
    pub fn apply_profile(
        &self,
        generation: u64,
        locale: Option<&str>,
        theme: Option<&str>,
    ) -> bool {
        if !self.loading(generation) {
            return false;
        }
        *self.profile.borrow_mut() = Some(ProfileChoice {
            locale: locale.and_then(Locale::parse),
            theme: theme.and_then(Theme::parse),
        });
        self.set_phase(Phase::Ready);
        self.apply();
        true
    }

    /// The profile's settings could not be read: keeps the fallback and unlocks the selectors, if this is
    /// still the current generation's load. Returns whether it applied.
    pub fn load_failed(&self, generation: u64) -> bool {
        let current = self.loading(generation);
        if current {
            self.set_phase(Phase::Ready);
        }
        current
    }

    fn loading(&self, generation: u64) -> bool {
        generation == self.generation.get()
            && self.phase() == Phase::Loading
            && self.profile.borrow().is_some()
    }

    /// Applies `locale` at once, unless [`Change::Busy`]. Signed out it is the device choice (stored,
    /// nothing to send); signed in it is the profile's and the caller must save it, then call
    /// [`Prefs::finish_locale`].
    pub fn set_locale(&self, locale: Locale) -> Change<Locale> {
        self.choose(locale, LOCALE_KEY, locale.id(), |c| &mut c.locale)
    }

    /// See [`Prefs::set_locale`].
    pub fn set_theme(&self, theme: Theme) -> Change<Theme> {
        self.choose(theme, THEME_KEY, theme.id(), |c| &mut c.theme)
    }

    fn choose<T: Copy>(
        &self,
        value: T,
        key: &str,
        id: &str,
        field: impl Fn(&mut ProfileChoice) -> &mut Option<T>,
    ) -> Change<T> {
        if self.phase() != Phase::Ready {
            return Change::Busy;
        }
        let change = match self.profile.borrow_mut().as_mut() {
            Some(choice) => Change::Save(SaveTicket {
                generation: self.generation.get(),
                save: {
                    self.saves.set(self.saves.get().wrapping_add(1));
                    self.saves.get()
                },
                previous: field(choice).replace(value),
                chosen: value,
            }),
            None => {
                self.storage.set(key, id);
                Change::Device
            }
        };
        if matches!(change, Change::Save(_)) {
            self.set_phase(Phase::Saving);
        }
        self.apply();
        change
    }

    /// The save of `ticket` ended (`saved` or failed). In its own session it unlocks the selectors and, on
    /// failure, restores the replaced choice; a save of an older session changes nothing. Returns whether
    /// it rolled back (the caller reports the failure).
    pub fn finish_locale(&self, ticket: SaveTicket<Locale>, saved: bool) -> bool {
        self.finish(ticket, saved, |c| &mut c.locale)
    }

    /// See [`Prefs::finish_locale`].
    pub fn finish_theme(&self, ticket: SaveTicket<Theme>, saved: bool) -> bool {
        self.finish(ticket, saved, |c| &mut c.theme)
    }

    fn finish<T: Copy + PartialEq>(
        &self,
        ticket: SaveTicket<T>,
        saved: bool,
        field: impl Fn(&mut ProfileChoice) -> &mut Option<T>,
    ) -> bool {
        if ticket.generation != self.generation.get()
            || ticket.save != self.saves.get()
            || self.phase() != Phase::Saving
        {
            return false;
        }
        self.set_phase(Phase::Ready);
        if saved {
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

    fn ticket<T>(change: Change<T>) -> SaveTicket<T> {
        match change {
            Change::Save(ticket) => ticket,
            _ => panic!("expected a profile save"),
        }
    }

    fn busy(p: &Prefs) -> bool {
        transaction(|c| p.busy().get(c))
    }

    /// Signed in with the profile's settings loaded (nothing chosen).
    fn loaded(storage: Rc<MemoryStorage>) -> (Prefs, u64) {
        let p = Prefs::new(storage, None);
        let g = p.begin_session(true);
        assert!(p.apply_profile(g, None, None));
        (p, g)
    }

    #[test]
    fn signed_in_choice_is_the_profiles_and_does_not_change_the_device() {
        let storage = device("en", "midnight-slate");
        let (p, _) = loaded(storage.clone());
        let t = ticket(p.set_theme(Theme::Daylight));
        assert_eq!(theme(&p), Theme::Daylight, "applies at once");
        assert_eq!(storage.get(THEME_KEY).as_deref(), Some("midnight-slate"));
        assert!(!p.finish_theme(t, true));
        assert!(matches!(p.set_locale(Locale::Pl), Change::Save(_)));
        // Signed out, a choice is the device's and needs no save (and is never busy).
        p.begin_session(false);
        assert!(!busy(&p));
        assert_eq!(p.set_locale(Locale::Pl), Change::Device);
        assert_eq!(p.set_theme(Theme::Graphite), Change::Device);
        assert_eq!(storage.get(LOCALE_KEY).as_deref(), Some("pl"));
    }

    #[test]
    fn a_choice_waits_for_the_profiles_settings_so_the_answer_cannot_overwrite_it() {
        let p = Prefs::new(device("en", "midnight-slate"), None);
        let g = p.begin_session(true);
        assert!(busy(&p), "selectors are disabled while the settings load");
        // A click while GET /api/settings is held is refused, even from a caller ignoring `busy`.
        assert_eq!(p.set_theme(Theme::Graphite), Change::Busy);
        assert_eq!(p.set_locale(Locale::Pl), Change::Busy);
        assert_eq!((locale(&p), theme(&p)), (Locale::En, Theme::MidnightSlate));
        // The held answer is released: the profile's choice applies and the selectors unlock.
        assert!(p.apply_profile(g, Some("pl"), Some("daylight")));
        assert!(!busy(&p));
        let t = ticket(p.set_theme(Theme::Graphite));
        // A repeated/late answer of the same session can no longer overwrite the newer choice.
        assert!(!p.apply_profile(g, Some("pl"), Some("daylight")));
        assert_eq!(theme(&p), Theme::Graphite);
        assert!(!p.finish_theme(t, true));
        assert!(!p.apply_profile(g, Some("en"), Some("daylight")));
        assert_eq!((locale(&p), theme(&p)), (Locale::Pl, Theme::Graphite));
    }

    #[test]
    fn a_failed_settings_read_unlocks_only_its_own_session() {
        let p = Prefs::new(Rc::new(MemoryStorage::default()), None);
        let a = p.begin_session(true);
        let b = p.begin_session(true);
        assert!(
            !p.load_failed(a),
            "an old session's failure does not unlock the new load"
        );
        assert!(busy(&p));
        assert!(p.load_failed(b));
        assert!(!busy(&p));
        assert!(!p.apply_profile(b, Some("pl"), None), "the load is over");
        assert!(matches!(p.set_locale(Locale::Pl), Change::Save(_)));
    }

    #[test]
    fn one_save_at_a_time_so_saves_cannot_reach_the_server_out_of_order() {
        let (p, _) = loaded(Rc::new(MemoryStorage::default()));
        let first = ticket(p.set_theme(Theme::Daylight));
        assert!(busy(&p));
        // Two more clicks while the POST is held: refused, nothing to send, the pending choice stays.
        assert_eq!(p.set_theme(Theme::Graphite), Change::Busy);
        assert_eq!(p.set_locale(Locale::Pl), Change::Busy);
        assert_eq!((locale(&p), theme(&p)), (Locale::En, Theme::Daylight));
        assert!(!p.finish_theme(first, true));
        assert!(!busy(&p));
        // Finishing the same save twice cannot unlock a later one.
        let second = ticket(p.set_locale(Locale::Pl));
        assert!(!p.finish_theme(first, true));
        assert!(busy(&p));
        assert!(!p.finish_locale(second, true));
        assert_eq!((locale(&p), theme(&p)), (Locale::Pl, Theme::Daylight));
    }

    #[test]
    fn failed_save_rolls_back_and_unlocks() {
        let p = Prefs::new(Rc::new(MemoryStorage::default()), None);
        let g = p.begin_session(true);
        p.apply_profile(g, Some("en"), Some("graphite"));
        let t = ticket(p.set_theme(Theme::Daylight));
        assert!(p.finish_theme(t, false));
        assert_eq!(theme(&p), Theme::Graphite);
        assert!(!busy(&p), "a failed save re-enables the selectors");
        // Rolling back to "not chosen" shows the fallback again.
        let fresh = p.begin_session(true);
        p.apply_profile(fresh, None, None);
        let t = ticket(p.set_locale(Locale::Pl));
        assert!(p.finish_locale(t, false));
        assert_eq!(locale(&p), Locale::En);
        assert!(!busy(&p));
    }

    #[test]
    fn an_old_sessions_save_never_unlocks_or_rolls_back_the_new_one() {
        let p = Prefs::new(Rc::new(MemoryStorage::default()), None);
        let a = p.begin_session(true);
        p.apply_profile(a, Some("en"), None);
        let pending = ticket(p.set_locale(Locale::Pl));
        // Profile switch while A's save is held: B loads, then B saves its own choice.
        let b = p.begin_session(true);
        assert!(
            !p.finish_locale(pending, false),
            "A's failure during B's load"
        );
        assert!(busy(&p), "B is still loading");
        assert!(p.apply_profile(b, Some("pl"), None));
        let mine = ticket(p.set_theme(Theme::Abyss));
        assert!(
            !p.finish_locale(pending, false),
            "A's failure during B's save"
        );
        assert!(busy(&p), "B's save is still pending");
        assert_eq!((locale(&p), theme(&p)), (Locale::Pl, Theme::Abyss));
        assert!(!p.finish_theme(mine, true));
        assert!(!busy(&p));
        // Signed out while a save is held: the device choice is free at once.
        let held = ticket(p.set_theme(Theme::Daylight));
        p.begin_session(false);
        assert!(!busy(&p));
        assert!(!p.finish_theme(held, false));
        assert_eq!(p.set_theme(Theme::Graphite), Change::Device);
    }
}
