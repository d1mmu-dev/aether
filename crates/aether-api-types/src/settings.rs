//! `GET /api/settings`: the signed-in user's own settings. The M9 Settings page only shows the
//! account slice (`auth::MeResponse`, `auth::ChangePassword`); this shape is the rest of that endpoint.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSettings {
    pub user_id: Uuid,
    /// Owner profile on M-PROF servers (D-242); `userId` remains the account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<Uuid>,
    /// `original`, `high`, `medium` or `low`.
    pub playback_quality: String,
    pub wifi_only: bool,
    pub offline_enabled: bool,
    pub max_cache_size_mb: i32,
    pub share_live_listening: bool,
    /// Always `false` in aether.
    pub has_listenbrainz_token: bool,
    /// Always `false` in aether.
    pub live_sharing_available: bool,
    /// aether-only (D-207): `hide` (default) or `show` AI-labelled podcasts and episodes.
    #[serde(default = "hide")]
    pub podcast_ai_content: String,
    /// aether-only (D-287): the profile's interface language id, `None` when not chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_locale: Option<String>,
    /// aether-only (D-287): the profile's interface theme id, `None` when not chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui_theme: Option<String>,
}

/// Interface language ids a profile may store (`uiLocale`); the web catalogue tests check its `Locale` ids
/// against this list, the API validates with it and migration 0066 constrains the column to it.
pub const UI_LOCALES: [&str; 2] = ["en", "pl"];
/// Interface theme ids a profile may store (`uiTheme`), in selector order; see [`UI_LOCALES`].
pub const UI_THEMES: [&str; 4] = ["midnight-slate", "abyss", "graphite", "daylight"];

/// `POST /api/settings` body for the interface preferences; absent fields are left alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppearancePatch {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_locale: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_theme: Option<String>,
}

fn hide() -> String {
    "hide".into()
}
