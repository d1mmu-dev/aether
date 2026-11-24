//! In-memory slskd search sessions (M7 Task 2, D-292). Upstream `routes/soulseek.ts`
//! `searchSessions`: a session per search, owned by its caller (here the profile), kept for
//! five minutes, at most fifty (the oldest evicted). Nothing is stored in the database.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aether_core::profile::ProfileId;
use aether_net::slskd::{SearchLimits, SearchResponse};
use serde::Serialize;
use uuid::Uuid;

/// How long a session is kept (upstream `SEARCH_SESSION_TTL`).
pub const SESSION_TTL: Duration = Duration::from_secs(5 * 60);
/// The most sessions kept (upstream `MAX_SEARCH_SESSIONS`).
pub const MAX_SESSIONS: usize = 50;

/// One result row, upstream `formatSearchResult`; field order is the wire order. Peer names and
/// paths are untrusted text, only serialized.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub username: String,
    pub path: String,
    pub filename: String,
    pub size: u64,
    pub bitrate: u64,
    pub format: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed_artist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed_album: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed_title: Option<String>,
}

/// ECMAScript `\s` (and `trim`): Rust's white space without U+0085, plus U+FEFF.
fn js_space(c: char) -> bool {
    (c.is_whitespace() && c != '\u{85}') || c == '\u{feff}'
}

/// `fileName.replace(/\.[^.]+$/, "").replace(/^\d+[\s.\-_]*/, "").replace(/^\s*-\s*/, "").trim()`
fn title_from_file_name(name: &str) -> String {
    let stem = match name.rfind('.') {
        Some(i) if i + 1 < name.len() => &name[..i],
        _ => name,
    };
    let rest = stem.trim_start_matches(|c: char| c.is_ascii_digit());
    let rest = if rest.len() < stem.len() {
        rest.trim_start_matches(|c: char| js_space(c) || matches!(c, '.' | '-' | '_'))
    } else {
        stem
    };
    let after = rest.trim_start_matches(js_space);
    let rest = match after.strip_prefix('-') {
        Some(r) => r.trim_start_matches(js_space),
        None => rest,
    };
    rest.trim_matches(js_space).to_string()
}

/// Upstream `formatSearchResult` for one user's response.
pub fn format_response(resp: SearchResponse) -> Vec<SearchResult> {
    resp.files
        .into_iter()
        .map(|file| {
            let full = file.filename;
            let parts: Vec<&str> = full.split(['/', '\\']).collect();
            let leaf = parts.last().copied().unwrap_or("");
            let filename = if leaf.is_empty() { full.as_str() } else { leaf }.to_string();
            let n = parts.len();
            let parsed_artist = (n > 2).then(|| parts[n - 3].to_string());
            let parsed_album = (n > 1).then(|| parts[n - 2].to_string());
            let title = title_from_file_name(&filename);
            SearchResult {
                username: resp.username.clone(),
                format: if filename.to_lowercase().ends_with(".flac") {
                    "flac"
                } else {
                    "mp3"
                },
                parsed_title: (!title.is_empty()).then_some(title),
                filename,
                size: file.size,
                bitrate: file.bitrate,
                parsed_artist,
                parsed_album,
                path: full,
            }
        })
        .collect()
}

struct Session {
    profile: ProfileId,
    created: Instant,
    results: Vec<SearchResult>,
    complete: bool,
}

/// The process's search sessions and search bounds.
#[derive(Clone)]
pub struct SlskdSearches {
    sessions: Arc<Mutex<HashMap<Uuid, Session>>>,
    pub limits: Arc<SearchLimits>,
}

impl Default for SlskdSearches {
    fn default() -> Self {
        SlskdSearches::new(SearchLimits::default())
    }
}

/// A session's answer for its owner.
pub struct View {
    pub results: Vec<SearchResult>,
    pub complete: bool,
}

impl SlskdSearches {
    pub fn new(limits: SearchLimits) -> Self {
        SlskdSearches {
            sessions: Arc::default(),
            limits: Arc::new(limits),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, Session>> {
        self.sessions.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A new, empty session for `profile`: expired ones are dropped, then the oldest while full.
    pub fn open(&self, profile: ProfileId) -> Uuid {
        let id = Uuid::new_v4();
        let mut sessions = self.lock();
        sessions.retain(|_, s| s.created.elapsed() <= SESSION_TTL);
        while sessions.len() >= MAX_SESSIONS {
            let Some(oldest) = sessions
                .iter()
                .min_by_key(|(_, s)| s.created)
                .map(|(k, _)| *k)
            else {
                break;
            };
            sessions.remove(&oldest);
        }
        sessions.insert(
            id,
            Session {
                profile,
                created: Instant::now(),
                results: Vec::new(),
                complete: false,
            },
        );
        id
    }

    /// Adds results to a session still kept (an evicted one is ignored).
    pub fn append(&self, id: Uuid, results: Vec<SearchResult>) {
        if let Some(s) = self.lock().get_mut(&id) {
            s.results.extend(results);
        }
    }

    pub fn finish(&self, id: Uuid) {
        if let Some(s) = self.lock().get_mut(&id) {
            s.complete = true;
        }
    }

    /// The session when it exists, has not expired and belongs to `profile`.
    pub fn view(&self, id: Uuid, profile: ProfileId) -> Option<View> {
        let sessions = self.lock();
        let s = sessions.get(&id)?;
        (s.profile == profile && s.created.elapsed() <= SESSION_TTL).then(|| View {
            results: s.results.clone(),
            complete: s.complete,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_follow_the_upstream_regexes() {
        for (name, want) in [
            ("01 - Song.flac", "Song"),
            ("01. Song.mp3", "Song"),
            ("Song.mp3", "Song"),
            ("07_-_Track.ogg", "Track"),
            ("x.", "x."),
            ("12", ""),
            ("a.b.c", "a.b"),
            ("01\u{85}Song.mp3", "\u{85}Song"),
        ] {
            assert_eq!(title_from_file_name(name), want, "{name:?}");
        }
    }
}
