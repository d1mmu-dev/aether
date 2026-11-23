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

/// Declares the message keys with both texts side by side, plus the test-only list of every key (so a new
/// key can never be missed by the catalogue tests).
macro_rules! catalogue {
    ($($(#[$doc:meta])* $key:ident => ($en:expr, $pl:expr $(,)?),)+) => {
        /// Message keys, grouped by domain. Never use translated text as an identifier.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Msg {
            $($(#[$doc])* $key,)+
        }

        impl Msg {
            #[cfg(test)]
            pub const ALL: &'static [Msg] = &[$(Msg::$key,)+];

            /// `(English, Polish)`.
            fn texts(self) -> (&'static str, &'static str) {
                match self {
                    $(Msg::$key => ($en, $pl),)+
                }
            }
        }
    };
}

catalogue! {
    // navigation
    NavCollection => ("Collection", "Kolekcja"),
    NavRadio => ("Radio", "Radio"),
    NavSearch => ("Search", "Szukaj"),
    NavPlaylists => ("Playlists", "Playlisty"),
    NavPodcasts => ("Podcasts", "Podcasty"),
    NavVibe => ("Vibe", "Klimat"),
    // header
    Home => ("Home", "Strona główna"),
    SearchPlaceholder => ("What do you want to play?", "Czego chcesz posłuchać?"),
    SearchPlaceholderShort => ("Search...", "Szukaj…"),
    SyncLibrary => ("Sync library", "Synchronizuj bibliotekę"),
    OpenMenu => ("Open menu", "Otwórz menu"),
    CloseMenu => ("Close menu", "Zamknij menu"),
    MobileMenu => ("Mobile menu", "Menu mobilne"),
    CreatePlaylist => ("Create playlist", "Utwórz playlistę"),
    ToggleQueuePanel => ("Toggle queue panel", "Pokaż lub ukryj kolejkę"),
    OpenPanel => ("Open panel", "Otwórz panel"),
    // profile menu
    Profiles => ("Profiles", "Profile"),
    ChooseProfile => ("Choose profile", "Wybierz profil"),
    SwitchProfile => ("Switch profile", "Zmień profil"),
    WhoIsListening => ("Who's listening?", "Kto słucha?"),
    ManageProfiles => ("Manage profiles", "Zarządzaj profilami"),
    SignOut => ("Sign out", "Wyloguj się"),
    // library scan toasts
    ScanStarting => ("Starting scan…", "Rozpoczynam skanowanie…"),
    ScanFinished => ("Scan finished", "Skanowanie zakończone"),
    ScanStillRunning => (
        "Scan is still running; refresh the page later",
        "Skanowanie wciąż trwa; odśwież stronę później",
    ),
    TheMusicDirectory => ("the music directory", "katalog z muzyką"),
    UnknownError => ("unknown error", "nieznany błąd"),
    // sign-in and first run
    Loading => ("Loading…", "Wczytywanie…"),
    WelcomeBack => ("Welcome back", "Witaj ponownie"),
    Username => ("Username", "Nazwa użytkownika"),
    Password => ("Password", "Hasło"),
    UsernamePlaceholder => ("Enter your username", "Wpisz nazwę użytkownika"),
    PasswordPlaceholder => ("Enter your password", "Wpisz hasło"),
    SignIn => ("Sign In", "Zaloguj się"),
    CreateAnAccount => ("Create an account", "Utwórz konto"),
    LoginFieldsRequired => ("Enter a username and a password", "Wpisz nazwę użytkownika i hasło"),
    /// The server's sign-in rejection (401), recognized by status; the body stays as received.
    InvalidCredentials => ("Invalid credentials", "Nieprawidłowa nazwa użytkownika lub hasło"),
    SetupTagline => (
        "Welcome to your personal music streaming platform",
        "Witaj na Twojej osobistej platformie do słuchania muzyki",
    ),
    CreateAdminAccount => ("Create Your Admin Account", "Utwórz konto administratora"),
    ChooseUsername => ("Choose a username", "Wybierz nazwę użytkownika"),
    AtLeastSixCharacters => ("At least 6 characters", "Co najmniej 6 znaków"),
    ConfirmPassword => ("Confirm Password", "Potwierdź hasło"),
    ConfirmPasswordPlaceholder => ("Confirm your password", "Wpisz hasło ponownie"),
    Continue => ("Continue", "Dalej"),
    AccountExistsRedirecting => (
        "An account already exists. Redirecting to sign in…",
        "Konto już istnieje. Przekierowuję do logowania…",
    ),
    UsernameLength => (
        "Username must be 3 to 50 characters",
        "Nazwa użytkownika musi mieć od 3 do 50 znaków",
    ),
    PasswordTooShort => ("Password must be at least 6 characters", "Hasło musi mieć co najmniej 6 znaków"),
    PasswordsDoNotMatch => ("Passwords do not match", "Hasła nie są zgodne"),
    // player
    NotPlaying => ("Not Playing", "Nic nie gra"),
    SelectSomethingToPlay => ("Select something to play", "Wybierz coś do odtworzenia"),
    PlaybackControls => ("Playback controls", "Sterowanie odtwarzaniem"),
    Play => ("Play", "Odtwórz"),
    Pause => ("Pause", "Wstrzymaj"),
    Shuffle => ("Shuffle", "Odtwarzanie losowe"),
    ShuffleOffForStation => (
        "Shuffle is off while a radio station plays",
        "Odtwarzanie losowe jest wyłączone, gdy gra stacja radiowa",
    ),
    Rewind30 => ("Rewind 30 seconds", "Cofnij o 30 sekund"),
    Forward30 => ("Forward 30 seconds", "Przewiń o 30 sekund do przodu"),
    PreviousTrack => ("Previous track", "Poprzedni utwór"),
    NextTrack => ("Next track", "Następny utwór"),
    RepeatOff => ("Repeat off", "Powtarzanie wyłączone"),
    RepeatAll => ("Repeat all", "Powtarzaj wszystko"),
    RepeatOne => ("Repeat one", "Powtarzaj jeden utwór"),
    RepeatOffHint => ("Repeat: off", "Powtarzanie: wyłączone"),
    RepeatAllHint => ("Repeat: all (loop queue)", "Powtarzanie: wszystko (cała kolejka w pętli)"),
    RepeatOneHint => ("Repeat: one (loop this track)", "Powtarzanie: jeden (ten utwór w pętli)"),
    Seek => ("Seek", "Pozycja odtwarzania"),
    SeekHint => ("Click or drag to seek", "Kliknij lub przeciągnij, aby przewinąć"),
    Mute => ("Mute", "Wycisz"),
    Volume => ("Volume", "Głośność"),
    SleepTimer => ("Sleep timer", "Wyłącznik czasowy"),
    SleepTimerOptions => ("Sleep timer options", "Opcje wyłącznika czasowego"),
    CancelTimer => ("Cancel Timer", "Anuluj wyłącznik"),
    AddToQueue => ("Add to queue", "Dodaj do kolejki"),
    NothingToShuffle => ("The library has no tracks to shuffle", "W bibliotece nie ma utworów do odtwarzania losowego"),
    // settings
    SettingsEyebrow => ("System configuration", "Konfiguracja systemu"),
    SettingsTitle => ("Settings", "Ustawienia"),
    SettingsSections => ("Settings sections", "Sekcje ustawień"),
    SectionAccount => ("Account", "Konto"),
    SectionAppearance => ("Appearance & language", "Wygląd i język"),
    Language => ("Language", "Język"),
    LanguageHint => ("Interface language", "Język interfejsu"),
    Theme => ("Theme", "Motyw"),
    ThemeHint => ("Colours of the whole app", "Kolory całej aplikacji"),
    SavedToProfile => (
        "Saved to this profile. Before sign-in, this device's choice is used.",
        "Zapisywane w tym profilu. Przed zalogowaniem obowiązuje wybór tego urządzenia.",
    ),
    AppearanceSaveFailed => (
        "Couldn't save the change to your profile; it was undone.",
        "Nie udało się zapisać zmiany w profilu; została cofnięta.",
    ),
    // settings: account
    SignedInAs => ("Signed in as", "Zalogowano jako"),
    RoleAdministrator => ("Administrator", "Administrator"),
    RoleMember => ("Member", "Użytkownik"),
    ChangePasswordHint => ("Change your account password", "Zmień hasło do konta"),
    Change => ("Change", "Zmień"),
    Cancel => ("Cancel", "Anuluj"),
    CurrentPassword => ("Current password", "Obecne hasło"),
    NewPasswordLabel => ("New password (6+ characters)", "Nowe hasło (co najmniej 6 znaków)"),
    NewPasswordPlaceholder => ("New password (min 6 characters)", "Nowe hasło (min. 6 znaków)"),
    ConfirmNewPassword => ("Confirm new password", "Potwierdź nowe hasło"),
    ChangePasswordButton => ("Change Password", "Zmień hasło"),
    PasswordChangeWarning => (
        "Changing your password signs out every app that uses these tokens. You will need to create new ones.",
        "Zmiana hasła wyloguje wszystkie aplikacje, które używają tych tokenów. Trzeba będzie utworzyć nowe.",
    ),
    EnterCurrentPassword => ("Enter your current password", "Wpisz obecne hasło"),
    NewPasswordTooShort => (
        "New password must be at least 6 characters",
        "Nowe hasło musi mieć co najmniej 6 znaków",
    ),
    /// The change-password success (200); the server's body is not shown.
    PasswordChanged => ("Password changed successfully", "Hasło zostało zmienione"),
    ShowAiPodcasts => ("Show AI-generated podcasts", "Pokazuj podcasty generowane przez AI"),
    ShowAiPodcastsHint => (
        "Shows and episodes labelled AI are hidden unless this is on; unlabelled ones always show",
        "Audycje i odcinki oznaczone jako AI są ukryte, dopóki ta opcja jest wyłączona; nieoznaczone są zawsze widoczne",
    ),
    // settings: profiles
    ProfilesIntro => (
        "Up to 5 profiles, including main. Multiple kids profiles are allowed. Kids has no content filters yet. No PIN is required: profiles are not a security boundary.",
        "Do 5 profili, wliczając główny. Można mieć kilka profili dziecięcych. Profil dziecięcy nie ma jeszcze filtrów treści. PIN nie jest wymagany: profile nie są zabezpieczeniem.",
    ),
    ProfileLimitReached => (
        "Profile limit reached. Delete a profile before adding another.",
        "Osiągnięto limit profili. Usuń jeden z profili, zanim dodasz kolejny.",
    ),
    OnlyMainManagesProfiles => (
        "Only the main profile can manage profiles.",
        "Tylko profil główny może zarządzać profilami.",
    ),
    EditProfile => ("Edit profile", "Edytuj profil"),
    AddProfile => ("Add profile", "Dodaj profil"),
    KindMain => ("Main", "Główny"),
    KindKids => ("Kids", "Dziecięcy"),
    KindStandard => ("Standard", "Standardowy"),
    KindUnknown => ("Unknown", "Nieznany"),
    MainKindFixed => ("Main profile kind cannot be changed.", "Rodzaju profilu głównego nie można zmienić."),
    ProfileKindGroup => ("Profile kind", "Rodzaj profilu"),
    AvatarPreset => ("Avatar preset", "Awatar"),
    AvatarInitial => ("Initial", "Inicjał"),
    AvatarCat => ("Cat", "Kot"),
    AvatarDog => ("Dog", "Pies"),
    AvatarFox => ("Fox", "Lis"),
    AvatarOwl => ("Owl", "Sowa"),
    AvatarBear => ("Bear", "Niedźwiedź"),
    AvatarPanda => ("Panda", "Panda"),
    AvatarRabbit => ("Rabbit", "Królik"),
    AvatarPenguin => ("Penguin", "Pingwin"),
    AvatarRobot => ("Robot", "Robot"),
    AvatarRocket => ("Rocket", "Rakieta"),
    AvatarStar => ("Star", "Gwiazda"),
    AvatarMusic => ("Music", "Muzyka"),
    ProfileName => ("Profile name", "Nazwa profilu"),
    ProfileNamePlaceholder => ("Profile name (1–50 characters)", "Nazwa profilu (1–50 znaków)"),
    SaveProfile => ("Save profile", "Zapisz profil"),
    ProfileSaved => ("Profile saved", "Profil zapisany"),
    ProfileDeleted => ("Profile and personal data deleted", "Usunięto profil i jego dane osobiste"),
    DeleteProfilePermanently => ("Delete profile permanently", "Usuń profil na zawsze"),
    DeleteProfileWarning => (
        "This cannot be undone. Deleting this profile permanently deletes its playlists, listening history, stars, ratings, settings, playback state, bookmarks, podcast follows/progress/download records and app tokens. Its sessions and app tokens stop working. Other profiles and the shared library are not deleted.",
        "Tej operacji nie można cofnąć. Usunięcie profilu na zawsze usuwa jego playlisty, historię słuchania, gwiazdki, oceny, ustawienia, stan odtwarzania, zakładki, obserwowane podcasty wraz z postępem i pobraniami oraz tokeny aplikacji. Jego sesje i tokeny aplikacji przestają działać. Inne profile i wspólna biblioteka nie są usuwane.",
    ),
    ConfirmProfileName => ("Confirm profile name", "Potwierdź nazwę profilu"),
    DeletePermanently => ("Delete permanently", "Usuń na zawsze"),
    ProfileNameLength => ("Profile name must be 1 to 50 characters", "Nazwa profilu musi mieć od 1 do 50 znaków"),
    InvalidProfileKind => ("Invalid profile kind", "Nieprawidłowy rodzaj profilu"),
    UnknownAvatar => ("Unknown avatar", "Nieznany awatar"),
    // settings: app tokens
    SectionApps => ("Apps", "Aplikacje"),
    AppsIntro => (
        "Only this profile's Subsonic app tokens are listed. Each profile manages its own tokens, including main. The account shares a limit of 50 tokens across all profiles; the server enforces it.",
        "Widoczne są tylko tokeny aplikacji Subsonic tego profilu. Każdy profil, także główny, zarządza własnymi tokenami. Wszystkie profile konta mają wspólny limit 50 tokenów; pilnuje go serwer.",
    ),
    NoAppTokens => ("No app tokens for this profile.", "Ten profil nie ma tokenów aplikacji."),
    Revoke => ("Revoke", "Unieważnij"),
    AppTokenRevoked => ("App token revoked", "Token aplikacji unieważniony"),
    AppTokenCopied => ("App token copied", "Skopiowano token aplikacji"),
    CopyBlocked => (
        "Copy was blocked. Select and copy the token manually.",
        "Kopiowanie zostało zablokowane. Zaznacz i skopiuj token ręcznie.",
    ),
    SaveTokenNow => (
        "Save this app token now. It is shown only once; dismissing or leaving Settings hides it permanently. Use your account username and this token in your Subsonic app. It signs into this profile.",
        "Zapisz teraz ten token aplikacji. Jest pokazywany tylko raz; po ukryciu go lub wyjściu z Ustawień zniknie na zawsze. W aplikacji Subsonic użyj nazwy użytkownika konta i tego tokenu. Token loguje do tego profilu.",
    ),
    NewAppToken => ("New app token (shown once)", "Nowy token aplikacji (pokazywany raz)"),
    CopyToken => ("Copy token", "Kopiuj token"),
    HideToken => ("I saved it — hide token", "Zapisano — ukryj token"),
    RefreshAppTokens => ("Refresh app tokens", "Odśwież tokeny aplikacji"),
    AppDeviceName => ("App device name", "Nazwa urządzenia aplikacji"),
    DeviceNamePlaceholder => ("Device name (e.g. Phone)", "Nazwa urządzenia (np. Telefon)"),
    CreateAppToken => ("Create app token", "Utwórz token aplikacji"),
    DeviceNameRequired => ("Device name is required", "Podaj nazwę urządzenia"),
    DeviceNameTooLong => (
        "Device name must be at most 100 characters",
        "Nazwa urządzenia może mieć najwyżej 100 znaków",
    ),
    // footer (the product name and year stay outside the catalogue)
    FooterTagline => ("Your music, your way.", "Twoja muzyka, na Twój sposób."),
    // desktop sidebar
    MainNavigation => ("Main navigation", "Nawigacja główna"),
    NavigationIndex => ("Navigation Index", "Spis nawigacji"),
    PlaylistStack => ("Playlist Stack", "Stos playlist"),
    // collection: tabs, refine panel, toolbar
    TabArtists => ("Artists", "Artyści"),
    TabAlbums => ("Albums", "Albumy"),
    TabTracks => ("Tracks", "Utwory"),
    Refine => ("Refine", "Dostosuj"),
    RefineButton => ("Refine: sort and per-page options", "Dostosuj: sortowanie i liczba na stronie"),
    RefineOptions => ("Refine options", "Opcje widoku"),
    CloseRefinePanel => ("Close refine panel", "Zamknij opcje widoku"),
    RefinementsActive => ("Refinements active", "Zmieniono opcje widoku"),
    SortBy => ("Sort by", "Sortuj według"),
    ItemsPerPage => ("Items per page", "Pozycji na stronie"),
    SortNameAsc => ("Name (A–Z)", "Nazwa (A–Z)"),
    SortNameDesc => ("Name (Z–A)", "Nazwa (Z–A)"),
    SortYearNewest => ("Year (Newest)", "Rok (od najnowszych)"),
    SortMostTracks => ("Most Tracks", "Najwięcej utworów"),
    ShuffleLibrary => ("Shuffle entire library", "Odtwarzaj losowo całą bibliotekę"),
    /// Tooltip form (lower-cased by [`Msg::hint`]).
    ShuffleLibraryHint => ("Shuffle library", "Losowo z biblioteki"),
    // collection: empty states and toasts
    NoArtistsYet => ("No artists yet", "Nie ma jeszcze artystów"),
    NoArtistsHelp => (
        "Put music in the server's music directory and press the sync button to scan it.",
        "Umieść muzykę w katalogu muzyki serwera i naciśnij przycisk synchronizacji, aby ją zeskanować.",
    ),
    NoAlbums => ("No albums", "Brak albumów"),
    NoTracks => ("No tracks", "Brak utworów"),
    NothingHereYet => ("Nothing here yet.", "Na razie nic tu nie ma."),
    AlbumHasNoTracks => ("This album has no tracks", "Ten album nie ma utworów"),
    PlaylistHasNoTracks => ("This playlist has no tracks", "Ta playlista nie ma utworów"),
    // pagination
    Pagination => ("Pagination", "Stronicowanie"),
    FirstPage => ("First page", "Pierwsza strona"),
    PreviousPage => ("Previous page", "Poprzednia strona"),
    NextPage => ("Next page", "Następna strona"),
    LastPage => ("Last page", "Ostatnia strona"),
    // search
    TypeToSearch => ("Type to search", "Zacznij pisać, aby wyszukać"),
    TypeToSearchHelp => (
        "Use the search bar above to find music, artists, and albums",
        "Użyj pola wyszukiwania powyżej, aby znaleźć muzykę, artystów i albumy",
    ),
    NoMatches => ("No matches found", "Nic nie znaleziono"),
    NoMatchesHelp => (
        "Try a different search term or check your spelling",
        "Spróbuj wyszukać coś innego lub sprawdź pisownię",
    ),
    // track rows
    ColumnTitle => ("Title", "Tytuł"),
    ColumnAlbum => ("Album", "Album"),
    ColumnDuration => ("Duration", "Czas"),
    ColumnActions => ("Actions", "Akcje"),
    AddToPlaylist => ("Add to playlist", "Dodaj do playlisty"),
    NoPlaylistsPicker => (
        "No playlists yet: create one on the Playlists page.",
        "Nie masz jeszcze playlist: utwórz ją na stronie Playlisty.",
    ),
    // playlists
    YourLibrary => ("Your library", "Twoja biblioteka"),
    Create => ("Create", "Utwórz"),
    Close => ("Close", "Zamknij"),
    PlaylistNamePlaceholder => ("Playlist name...", "Nazwa playlisty…"),
    NoPlaylistsYet => ("No playlists yet", "Nie masz jeszcze playlist"),
    NoPlaylistsHelp => (
        "Create one to start organising your music",
        "Utwórz pierwszą, aby uporządkować swoją muzykę",
    ),
    AllPlaylists => ("All Playlists", "Wszystkie playlisty"),
    EnterPlaylistName => ("Enter a playlist name", "Wpisz nazwę playlisty"),
    PlaylistNameTooLong => (
        "Playlist name must be at most 200 characters",
        "Nazwa playlisty może mieć najwyżej 200 znaków",
    ),
    PlaylistEyebrow => ("Playlist", "Playlista"),
    PlaylistName => ("Playlist name", "Nazwa playlisty"),
    PlayAll => ("Play all", "Odtwórz wszystko"),
    ShufflePlay => ("Shuffle play", "Odtwarzaj losowo"),
    MakePrivate => ("Make private", "Ustaw jako prywatną"),
    MakePublic => ("Make public", "Ustaw jako publiczną"),
    DeletePlaylist => ("Delete playlist", "Usuń playlistę"),
    DeletePlaylistQuestion => ("Delete Playlist?", "Usunąć playlistę?"),
    Delete => ("Delete", "Usuń"),
    EmptyPlaylist => ("Empty Playlist", "Pusta playlista"),
    NoTracksYet => ("No tracks yet", "Nie ma jeszcze utworów"),
    AddTracksToStart => ("Add some tracks to get started", "Dodaj utwory, aby zacząć"),
    RemoveFromPlaylist => ("Remove from playlist", "Usuń z playlisty"),
}

impl Msg {
    pub fn text(self, locale: Locale) -> &'static str {
        let (en, pl) = self.texts();
        match locale {
            Locale::En => en,
            Locale::Pl => pl,
        }
    }

    /// The tooltip form (`title=`): the legacy UI writes tooltips in lower case.
    pub fn hint(self, locale: Locale) -> String {
        self.text(locale).to_lowercase()
    }
}

/// A form/inline error: a catalogue message (follows the language) or a server body shown as received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiError {
    Msg(Msg),
    Server(String),
}

impl UiError {
    pub fn text(&self, locale: Locale) -> String {
        match self {
            UiError::Msg(m) => m.text(locale).to_owned(),
            UiError::Server(body) => body.clone(),
        }
    }

    /// The sign-in call's failure: a rejected sign-in (401) is a known user error and is localized; any
    /// other failure keeps the server's message verbatim.
    pub fn sign_in(status: Option<u32>, message: String) -> UiError {
        if status == Some(401) {
            UiError::Msg(Msg::InvalidCredentials)
        } else {
            UiError::Server(message)
        }
    }
}

// ---- messages with arguments (whole sentences per language) -----------------------------------------------

pub fn listening_as(locale: Locale, name: &str) -> String {
    match locale {
        Locale::En => format!("Listening as {name}"),
        Locale::Pl => format!("Słuchasz jako {name}"),
    }
}

pub fn sign_in_to(locale: Locale, product: &str) -> String {
    match locale {
        Locale::En => format!("Sign in to continue to {product}"),
        Locale::Pl => format!("Zaloguj się, aby przejść do {product}"),
    }
}

pub fn owner_account_intro(locale: Locale, product: &str) -> String {
    match locale {
        Locale::En => format!(
            "This is the owner account for your {product} server. You'll manage users, integrations, and settings from here."
        ),
        Locale::Pl => format!(
            "To konto właściciela Twojego serwera {product}. Stąd zarządzasz użytkownikami, integracjami i ustawieniami."
        ),
    }
}

pub fn scanning_dir(locale: Locale, dir: &str) -> String {
    match locale {
        Locale::En => format!("Scanning {dir}…"),
        Locale::Pl => format!("Skanuję {dir}…"),
    }
}

pub fn scan_failed(locale: Locale, reason: &str) -> String {
    match locale {
        Locale::En => format!("Scan failed: {reason}"),
        Locale::Pl => format!("Skanowanie nie powiodło się: {reason}"),
    }
}

pub fn added_to_queue(locale: Locale, title: &str) -> String {
    match locale {
        Locale::En => format!("Added “{title}” to the queue"),
        Locale::Pl => format!("Dodano „{title}” do kolejki"),
    }
}

/// The running sleep timer; `left` is an already formatted countdown.
pub fn sleep_timer_left(locale: Locale, left: &str) -> String {
    match locale {
        Locale::En => format!("Sleep timer: {left} remaining"),
        Locale::Pl => format!("Wyłącznik czasowy: pozostało {left}"),
    }
}

/// A sleep-timer preset: whole hours as hours, anything else in minutes.
pub fn sleep_preset(locale: Locale, mins: u32) -> String {
    let n = u64::from(mins);
    if mins >= 60 && mins.is_multiple_of(60) {
        let h = n / 60;
        match locale {
            Locale::En if h == 1 => "1 hour".to_owned(),
            Locale::En => format!("{h} hours"),
            Locale::Pl => pl_count(h, "godzina", "godziny", "godzin"),
        }
    } else {
        match locale {
            Locale::En if n == 1 => "1 minute".to_owned(),
            Locale::En => format!("{n} minutes"),
            Locale::Pl => pl_count(n, "minuta", "minuty", "minut"),
        }
    }
}

fn pl_count(n: u64, one: &str, few: &str, many: &str) -> String {
    let noun = match pl_plural(n) {
        PlPlural::One => one,
        PlPlural::Few => few,
        PlPlural::Many => many,
    };
    format!("{n} {noun}")
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

/// The profile counter, e.g. "2 / 5 profiles" (Polish avoids the plural with a label).
pub fn profile_count(locale: Locale, count: usize, max: u32) -> String {
    match locale {
        Locale::En => format!("{count} / {max} profiles"),
        Locale::Pl => format!("Profile: {count} / {max}"),
    }
}

/// The typed-name deletion prompt; `name` is the profile's own name, shown as is.
pub fn type_name_to_confirm(locale: Locale, name: &str) -> String {
    match locale {
        Locale::En => format!("Type {name} exactly to confirm"),
        Locale::Pl => format!("Aby potwierdzić, wpisz dokładnie: {name}"),
    }
}

/// An app token's dates; the timestamps are shown as received.
pub fn token_dates(locale: Locale, created: &str, last_used: Option<&str>) -> String {
    match locale {
        Locale::En => format!(
            "Created {created} · Last used {}",
            last_used.unwrap_or("never")
        ),
        Locale::Pl => format!(
            "Utworzono {created} · Ostatnie użycie: {}",
            last_used.unwrap_or("nigdy")
        ),
    }
}

/// A count as the API sends it (`i64`); a negative count never happens and reads as zero.
fn whole(n: i64) -> u64 {
    u64::try_from(n).unwrap_or(0)
}

/// `en_one` / `en_many` in English, the three Polish forms otherwise (whole phrases, no noun fragments).
fn counted(locale: Locale, n: i64, en: (&str, &str), pl: (&str, &str, &str)) -> String {
    let n = whole(n);
    match locale {
        Locale::En => format!("{n} {}", if n == 1 { en.0 } else { en.1 }),
        Locale::Pl => pl_count(n, pl.0, pl.1, pl.2),
    }
}

/// A complete "N tracks" phrase (whole sentences per locale, no concatenated noun fragments).
pub fn track_count(locale: Locale, n: i64) -> String {
    counted(
        locale,
        n,
        ("track", "tracks"),
        ("utwór", "utwory", "utworów"),
    )
}

/// "N songs" (playlist cards and header; Polish uses the same noun as tracks).
pub fn song_count(locale: Locale, n: i64) -> String {
    counted(locale, n, ("song", "songs"), ("utwór", "utwory", "utworów"))
}

pub fn artist_count(locale: Locale, n: i64) -> String {
    // Masculine-personal noun: with digits both 2-4 and 5+ take the genitive plural.
    counted(
        locale,
        n,
        ("artist", "artists"),
        ("artysta", "artystów", "artystów"),
    )
}

pub fn album_count(locale: Locale, n: i64) -> String {
    counted(
        locale,
        n,
        ("album", "albums"),
        ("album", "albumy", "albumów"),
    )
}

pub fn playlist_count(locale: Locale, n: i64) -> String {
    counted(
        locale,
        n,
        ("playlist", "playlists"),
        ("playlista", "playlisty", "playlist"),
    )
}

/// The pager's position, e.g. "Page 2 of 3".
pub fn page_of(locale: Locale, page: i64, pages: i64) -> String {
    match locale {
        Locale::En => format!("Page {page} of {pages}"),
        Locale::Pl => format!("Strona {page} z {pages}"),
    }
}

/// The items shown, e.g. `1–40 of 120` (or `0 of 0`).
pub fn page_range(locale: Locale, offset: i64, shown: usize, total: i64) -> String {
    let of = match locale {
        Locale::En => "of",
        Locale::Pl => "z",
    };
    if shown == 0 {
        return format!("0 {of} {total}");
    }
    format!("{}–{} {of} {total}", offset + 1, offset + shown as i64)
}

/// A card's play button label; `title` is the item's own name.
pub fn play_named(locale: Locale, title: &str) -> String {
    match locale {
        Locale::En => format!("Play {title}"),
        Locale::Pl => format!("Odtwórz: {title}"),
    }
}

/// The playlist deletion prompt; `name` is the playlist's own name.
pub fn delete_playlist_confirm(locale: Locale, name: &str) -> String {
    match locale {
        Locale::En => {
            format!("Are you sure you want to delete \"{name}\"? This action cannot be undone.")
        }
        Locale::Pl => {
            format!("Czy na pewno chcesz usunąć „{name}”? Tej operacji nie można cofnąć.")
        }
    }
}

/// The toast after adding a track to the playlist called `name` (`added` false: it was already there).
pub fn added_to_playlist(locale: Locale, name: &str, added: bool) -> String {
    match (locale, added) {
        (Locale::En, true) => format!("Added to {name}"),
        (Locale::En, false) => format!("Already in {name}"),
        (Locale::Pl, true) => format!("Dodano do playlisty „{name}”"),
        (Locale::Pl, false) => format!("Ten utwór jest już na playliście „{name}”"),
    }
}

/// A playlist's total length: `4 min`, `1 hr 5 min` (legacy `formatTotalDuration`).
pub fn total_duration(locale: Locale, seconds: i64) -> String {
    let (hours, mins) = (seconds / 3600, (seconds % 3600) / 60);
    match locale {
        Locale::En if hours > 0 => format!("{hours} hr {mins} min"),
        Locale::Pl if hours > 0 => format!("{hours} godz. {mins} min"),
        _ => format!("{mins} min"),
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
        for &m in Msg::ALL {
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
        assert_eq!(track_count(Locale::Pl, -3), "0 utworów");
    }

    #[test]
    fn library_counts_use_the_three_polish_forms() {
        let pl = |f: fn(Locale, i64) -> String| [1, 2, 5, 12, 22].map(|n| f(Locale::Pl, n));
        assert_eq!(
            pl(album_count),
            [
                "1 album",
                "2 albumy",
                "5 albumów",
                "12 albumów",
                "22 albumy"
            ]
        );
        assert_eq!(
            pl(playlist_count),
            [
                "1 playlista",
                "2 playlisty",
                "5 playlist",
                "12 playlist",
                "22 playlisty"
            ]
        );
        assert_eq!(
            pl(artist_count),
            [
                "1 artysta",
                "2 artystów",
                "5 artystów",
                "12 artystów",
                "22 artystów"
            ]
        );
        assert_eq!(
            pl(song_count),
            [
                "1 utwór",
                "2 utwory",
                "5 utworów",
                "12 utworów",
                "22 utwory"
            ]
        );
        assert_eq!(artist_count(Locale::En, 1), "1 artist");
        assert_eq!(album_count(Locale::En, 0), "0 albums");
        assert_eq!(playlist_count(Locale::En, 3), "3 playlists");
        assert_eq!(song_count(Locale::En, 1), "1 song");
    }

    #[test]
    fn pagination_and_duration_sentences() {
        assert_eq!(page_of(Locale::En, 2, 3), "Page 2 of 3");
        assert_eq!(page_of(Locale::Pl, 2, 3), "Strona 2 z 3");
        assert_eq!(page_range(Locale::En, 0, 40, 120), "1–40 of 120");
        assert_eq!(page_range(Locale::En, 80, 40, 120), "81–120 of 120");
        assert_eq!(page_range(Locale::En, 0, 0, 0), "0 of 0");
        assert_eq!(page_range(Locale::Pl, 40, 40, 120), "41–80 z 120");
        assert_eq!(page_range(Locale::Pl, 0, 0, 0), "0 z 0");
        assert_eq!(total_duration(Locale::En, 0), "0 min");
        assert_eq!(total_duration(Locale::En, 59), "0 min");
        assert_eq!(total_duration(Locale::En, 245), "4 min");
        assert_eq!(total_duration(Locale::En, 3900), "1 hr 5 min");
        assert_eq!(total_duration(Locale::Pl, 3900), "1 godz. 5 min");
        assert_eq!(total_duration(Locale::Pl, 245), "4 min");
    }

    #[test]
    fn playlist_sentences_keep_names_verbatim() {
        let name = "Zażółć \"<b>\"";
        for l in Locale::ALL {
            for text in [
                play_named(l, name),
                delete_playlist_confirm(l, name),
                added_to_playlist(l, name, true),
                added_to_playlist(l, name, false),
            ] {
                assert!(text.contains(name), "{text}");
                assert!(text.len() > name.len() + 3, "{text}");
            }
        }
        assert_eq!(
            delete_playlist_confirm(Locale::En, "Road"),
            "Are you sure you want to delete \"Road\"? This action cannot be undone."
        );
        assert_eq!(
            delete_playlist_confirm(Locale::Pl, "Road"),
            "Czy na pewno chcesz usunąć „Road”? Tej operacji nie można cofnąć."
        );
        assert_eq!(added_to_playlist(Locale::En, "Road", true), "Added to Road");
        assert_eq!(
            added_to_playlist(Locale::En, "Road", false),
            "Already in Road"
        );
        assert_eq!(
            added_to_playlist(Locale::Pl, "Road", true),
            "Dodano do playlisty „Road”"
        );
        assert_eq!(play_named(Locale::Pl, "Abbey Road"), "Odtwórz: Abbey Road");
        assert_eq!(play_named(Locale::En, "Abbey Road"), "Play Abbey Road");
    }

    #[test]
    fn polish_is_translated_except_words_shared_by_both_languages() {
        // Polish must actually be translated: only names that are the same word in both languages may match.
        let same: Vec<Msg> = Msg::ALL
            .iter()
            .copied()
            .filter(|m| m.text(Locale::En) == m.text(Locale::Pl))
            .collect();
        assert_eq!(
            same,
            vec![
                Msg::NavRadio,
                Msg::RoleAdministrator,
                Msg::AvatarPanda,
                Msg::AvatarRobot,
                Msg::ColumnAlbum,
            ]
        );
        assert!(Msg::ALL.len() >= 220, "{}", Msg::ALL.len());
    }

    #[test]
    fn settings_sentences_keep_names_and_dates_verbatim() {
        let name = "Zażółć <b>";
        for l in Locale::ALL {
            assert!(type_name_to_confirm(l, name).contains(name));
            let dates = token_dates(l, "2026-11-23T10:00:00Z", Some("2026-11-24"));
            assert!(dates.contains("2026-11-23T10:00:00Z") && dates.contains("2026-11-24"));
        }
        assert_eq!(
            type_name_to_confirm(Locale::Pl, "Ala"),
            "Aby potwierdzić, wpisz dokładnie: Ala"
        );
        assert_eq!(
            type_name_to_confirm(Locale::En, "Ala"),
            "Type Ala exactly to confirm"
        );
        assert_eq!(
            token_dates(Locale::Pl, "X", None),
            "Utworzono X · Ostatnie użycie: nigdy"
        );
        assert_eq!(
            token_dates(Locale::En, "X", None),
            "Created X · Last used never"
        );
        assert_eq!(profile_count(Locale::En, 2, 5), "2 / 5 profiles");
        assert_eq!(profile_count(Locale::Pl, 1, 5), "Profile: 1 / 5");
    }

    #[test]
    fn hints_are_lower_case_tooltips() {
        assert_eq!(Msg::Home.hint(Locale::En), "home");
        assert_eq!(
            Msg::RepeatAllHint.hint(Locale::En),
            "repeat: all (loop queue)"
        );
        assert_eq!(
            Msg::SeekHint.hint(Locale::Pl),
            "kliknij lub przeciągnij, aby przewinąć"
        );
    }

    #[test]
    fn messages_with_arguments_keep_the_argument_in_both_languages() {
        let arg = "Zażółć <b>";
        for l in Locale::ALL {
            for text in [
                listening_as(l, arg),
                sign_in_to(l, arg),
                owner_account_intro(l, arg),
                scanning_dir(l, arg),
                scan_failed(l, arg),
                added_to_queue(l, arg),
                sleep_timer_left(l, arg),
            ] {
                assert!(text.contains(arg), "{text}");
                assert!(text.len() > arg.len() + 3, "{text}");
            }
        }
        assert_eq!(listening_as(Locale::Pl, "Ala"), "Słuchasz jako Ala");
        assert_eq!(added_to_queue(Locale::Pl, "X"), "Dodano „X” do kolejki");
        assert_eq!(added_to_queue(Locale::En, "X"), "Added “X” to the queue");
    }

    #[test]
    fn sleep_presets_read_naturally() {
        let en: Vec<String> = [15, 30, 45, 60, 90, 120]
            .map(|m| sleep_preset(Locale::En, m))
            .into();
        assert_eq!(
            en,
            [
                "15 minutes",
                "30 minutes",
                "45 minutes",
                "1 hour",
                "90 minutes",
                "2 hours"
            ]
        );
        let pl: Vec<String> = [15, 30, 45, 60, 90, 120]
            .map(|m| sleep_preset(Locale::Pl, m))
            .into();
        assert_eq!(
            pl,
            [
                "15 minut",
                "30 minut",
                "45 minut",
                "1 godzina",
                "90 minut",
                "2 godziny"
            ]
        );
        assert_eq!(sleep_preset(Locale::Pl, 2), "2 minuty");
        assert_eq!(sleep_preset(Locale::Pl, 1), "1 minuta");
        assert_eq!(sleep_preset(Locale::Pl, 300), "5 godzin");
    }

    #[test]
    fn sign_in_errors_localize_only_the_known_rejection() {
        let bad = UiError::sign_in(Some(401), "Invalid credentials".into());
        assert_eq!(
            bad.text(Locale::Pl),
            "Nieprawidłowa nazwa użytkownika lub hasło"
        );
        assert_eq!(bad.text(Locale::En), "Invalid credentials");
        let other = UiError::sign_in(Some(429), "Too many attempts, wait 60s".into());
        assert_eq!(other.text(Locale::Pl), "Too many attempts, wait 60s");
        let offline = UiError::sign_in(None, "network down".into());
        assert_eq!(offline.text(Locale::Pl), "network down");
    }
}
