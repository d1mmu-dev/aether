//! The slskd connection check (M7, first slice). Upstream contract: kima-hub
//! `backend/src/lib/soulseek/slskd-client.ts` — `GET /api/v0/application` with the API key in the
//! `X-API-Key` header; the instance is usable when `server.isConnected` and `server.isLoggedIn`.
//!
//! slskd is an admin-configured service, usually on the private network, so unlike image or feed
//! downloads it is allowed on private addresses (the intentional SSRF boundary is: admin only,
//! one fixed path, GET, no redirects, no proxy, key only in a header to the configured origin).
//! The network guard (`AETHER_NET_DENY_REAL=1`) still applies: a refused host is not contacted.

use std::fmt;
use std::time::Duration;

use reqwest::Url;
use reqwest::header::{ACCEPT, HeaderValue};
use serde::Deserialize;

/// The longest URL accepted (characters, after trimming).
const MAX_URL_CHARS: usize = 512;
/// The longest API key accepted (characters, after trimming), as for the other secrets.
const MAX_KEY_CHARS: usize = 200;
/// The largest `/application` answer read.
const MAX_BODY: usize = 64 * 1024;
/// The timeout of the admin connection check.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// A validated slskd address: an `http`/`https` origin without credentials, path, query or
/// fragment. Stored without a trailing slash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlskdUrl(Url);

/// Why an address was refused. The message never quotes the input, which may carry credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlskdUrlError {
    Empty,
    TooLong,
    Invalid,
    Scheme,
    Credentials,
    Path,
}

impl fmt::Display for SlskdUrlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            SlskdUrlError::Empty => "slskdUrl is required",
            SlskdUrlError::TooLong => "slskdUrl is too long",
            SlskdUrlError::Invalid => "slskdUrl is not a valid URL",
            SlskdUrlError::Scheme => "slskdUrl must start with http:// or https://",
            SlskdUrlError::Credentials => "slskdUrl must not contain credentials",
            SlskdUrlError::Path => "slskdUrl must not have a path, query or fragment",
        })
    }
}

impl std::error::Error for SlskdUrlError {}

impl SlskdUrl {
    pub fn parse(input: &str) -> Result<SlskdUrl, SlskdUrlError> {
        let s = input.trim();
        if s.is_empty() {
            return Err(SlskdUrlError::Empty);
        }
        if s.chars().count() > MAX_URL_CHARS {
            return Err(SlskdUrlError::TooLong);
        }
        let url = Url::parse(s).map_err(|_| SlskdUrlError::Invalid)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(SlskdUrlError::Scheme);
        }
        if url.host_str().is_none_or(str::is_empty) {
            return Err(SlskdUrlError::Invalid);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(SlskdUrlError::Credentials);
        }
        if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
            return Err(SlskdUrlError::Path);
        }
        Ok(SlskdUrl(url))
    }

    /// `scheme://host[:port]`, no trailing slash.
    pub fn as_str(&self) -> &str {
        self.0.as_str().trim_end_matches('/')
    }

    fn application(&self) -> Url {
        let mut url = self.0.clone();
        url.set_path("/api/v0/application");
        url
    }
}

impl fmt::Display for SlskdUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The slskd API key. Never printed.
#[derive(Clone, PartialEq, Eq)]
pub struct SlskdApiKey(String);

impl SlskdApiKey {
    /// Trimmed; `None` when blank, too long, or not visible US-ASCII (`!`..=`~`: no spaces,
    /// tabs, CR/LF, DEL, other controls or non-ASCII, which `HeaderValue` alone would let
    /// through as opaque bytes). The stored key and a typed check key share this boundary.
    pub fn new(raw: &str) -> Option<SlskdApiKey> {
        let s = raw.trim();
        if s.is_empty()
            || s.len() > MAX_KEY_CHARS
            || !s.bytes().all(|b| b.is_ascii_graphic())
            || HeaderValue::from_str(s).is_err()
        {
            return None;
        }
        Some(SlskdApiKey(s.to_string()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SlskdApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SlskdApiKey(..)")
    }
}

/// What the check found. Carries no URL, header or body text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlskdCheck {
    /// Reachable, connected and logged in to the Soulseek network.
    Connected,
    /// Reachable, but slskd is not connected/logged in to Soulseek.
    NotLoggedIn,
    /// 401 or 403: the API key was refused.
    Rejected,
    /// No answer: connection refused, DNS failure, TLS failure or timeout.
    NoAnswer,
    /// Any other answer: an error status, a redirect, an oversized or non-JSON body.
    Unexpected,
    /// Not sent: the network guard refuses the host.
    Refused,
    /// Not sent: the process has no outbound client.
    Disabled,
}

#[derive(Deserialize)]
struct Application {
    server: Option<ServerState>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerState {
    #[serde(default)]
    is_connected: bool,
    #[serde(default)]
    is_logged_in: bool,
}

/// Asks `url`'s `/api/v0/application` once. A fresh client per check: no redirects, no proxy
/// (an environment proxy would see the key), no cookies.
pub async fn check_connection(
    url: &SlskdUrl,
    key: Option<&SlskdApiKey>,
    deny_real: bool,
    timeout: Duration,
) -> SlskdCheck {
    let target = url.application();
    if crate::guard::refuses(&target, deny_real) {
        return SlskdCheck::Refused;
    }
    let Ok(http) = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(timeout)
        .connect_timeout(timeout)
        .build()
    else {
        return SlskdCheck::NoAnswer;
    };
    let mut rb = http.get(target).header(ACCEPT, "application/json");
    if let Some(key) = key {
        let Ok(mut value) = HeaderValue::from_str(key.expose()) else {
            return SlskdCheck::Unexpected;
        };
        value.set_sensitive(true);
        rb = rb.header("X-API-Key", value);
    }
    let Ok(mut res) = rb.send().await else {
        return SlskdCheck::NoAnswer;
    };
    let status = res.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return SlskdCheck::Rejected;
    }
    if !status.is_success() {
        return SlskdCheck::Unexpected;
    }
    if res.content_length().is_some_and(|n| n > MAX_BODY as u64) {
        return SlskdCheck::Unexpected;
    }
    let mut body = Vec::new();
    loop {
        match res.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > MAX_BODY {
                    return SlskdCheck::Unexpected;
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(e) if e.is_timeout() => return SlskdCheck::NoAnswer,
            Err(_) => return SlskdCheck::Unexpected,
        }
    }
    match serde_json::from_slice::<Application>(&body) {
        Ok(Application {
            server:
                Some(ServerState {
                    is_connected: true,
                    is_logged_in: true,
                }),
        }) => SlskdCheck::Connected,
        Ok(_) => SlskdCheck::NotLoggedIn,
        Err(_) => SlskdCheck::Unexpected,
    }
}

impl crate::Net {
    /// [`check_connection`] under this process's network guard; a disabled client sends nothing.
    /// Not gated by the identification switch: slskd is a separate, admin-configured integration.
    pub async fn check_slskd(&self, url: &SlskdUrl, key: Option<&SlskdApiKey>) -> SlskdCheck {
        let Some(live) = &self.live else {
            return SlskdCheck::Disabled;
        };
        check_connection(url, key, live.cfg.deny_real, CHECK_TIMEOUT).await
    }
}
