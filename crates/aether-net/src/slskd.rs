//! The slskd connection check (M7, first slice). Upstream contract: kima-hub
//! `backend/src/lib/soulseek/slskd-client.ts` — `GET /api/v0/application` with the API key in the
//! `X-API-Key` header; the instance is usable when `server.isConnected` and `server.isLoggedIn`.
//!
//! slskd is an admin-configured service, usually on the private network, so unlike image or feed
//! downloads it is allowed on private addresses (the intentional SSRF boundary is: admin only,
//! one fixed path, GET, no redirects, no proxy, key only in a header to the configured origin).
//! The network guard (`AETHER_NET_DENY_REAL=1`) still applies: a refused host is not contacted.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{ACCEPT, HeaderValue};
use reqwest::{RequestBuilder, StatusCode, Url};
use serde::{Deserialize, Deserializer};
use tokio::sync::OwnedSemaphorePermit;
use tokio::time::Instant;
use uuid::Uuid;

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

/// A client for one operation against the configured origin: no redirects, no proxy (an
/// environment proxy would see the key), no cookies. Built once per check or search, never per
/// request.
fn client(timeout: Duration) -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(timeout)
        .connect_timeout(timeout)
        .build()
        .ok()
}

/// The key as a sensitive header value (never in the URL).
fn key_header(key: Option<&SlskdApiKey>) -> Result<Option<HeaderValue>, ()> {
    key.map(|key| {
        HeaderValue::from_str(key.expose())
            .map(|mut v| {
                v.set_sensitive(true);
                v
            })
            .map_err(|_| ())
    })
    .transpose()
}

fn with_key(rb: RequestBuilder, key: &Option<HeaderValue>) -> RequestBuilder {
    match key {
        Some(value) => rb.header("X-API-Key", value.clone()),
        None => rb,
    }
}

/// Why a body could not be read.
enum BodyError {
    TooLarge,
    Timeout,
    Broken,
}

/// Reads at most `cap` bytes, refusing a longer body by its declared length or as it streams.
async fn read_capped(mut res: reqwest::Response, cap: usize) -> Result<Vec<u8>, BodyError> {
    if res.content_length().is_some_and(|n| n > cap as u64) {
        return Err(BodyError::TooLarge);
    }
    let mut body = Vec::new();
    loop {
        match res.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > cap {
                    return Err(BodyError::TooLarge);
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok(body),
            Err(e) if e.is_timeout() => return Err(BodyError::Timeout),
            Err(_) => return Err(BodyError::Broken),
        }
    }
}

/// Asks `url`'s `/api/v0/application` once.
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
    let Some(http) = client(timeout) else {
        return SlskdCheck::NoAnswer;
    };
    let Ok(key) = key_header(key) else {
        return SlskdCheck::Unexpected;
    };
    let rb = with_key(http.get(target).header(ACCEPT, "application/json"), &key);
    let Ok(res) = rb.send().await else {
        return SlskdCheck::NoAnswer;
    };
    let status = res.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return SlskdCheck::Rejected;
    }
    if !status.is_success() {
        return SlskdCheck::Unexpected;
    }
    let body = match read_capped(res, MAX_BODY).await {
        Ok(body) => body,
        Err(BodyError::Timeout) => return SlskdCheck::NoAnswer,
        Err(_) => return SlskdCheck::Unexpected,
    };
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

// ---- Search (M7 Task 2, D-292). Upstream: `slskd-client.ts` `search()`. ----

/// slskd searches in flight per process; a further search is refused (`Busy`), not queued.
pub const MAX_SEARCHES: usize = 2;
/// The longest query sent (characters, after trimming).
pub const MAX_QUERY_CHARS: usize = 200;
/// The longest peer username kept, in UTF-8 bytes (review I-1: it is repeated into every result
/// row). A blank or longer name skips that peer's whole response; it is never truncated.
pub const MAX_USERNAME_BYTES: usize = 255;
/// The longest shared-file path kept, in UTF-8 bytes. A blank or longer path skips that file
/// only, without using up the file cap; it is never truncated.
pub const MAX_FILENAME_BYTES: usize = 1024;

/// Untrusted peer text within bounds: not blank (white space only) and at most `max` bytes.
fn bounded(text: &str, max: usize) -> bool {
    text.len() <= max && !text.trim().is_empty()
}

/// The bounds of one search. `Default` is the production setting; tests shorten the times.
#[derive(Debug, Clone)]
pub struct SearchLimits {
    /// `responseLimit` sent to slskd (upstream `maxResponses`, 100); also the most users kept.
    pub response_limit: u32,
    /// The most files kept from one search, over all users.
    pub max_files: usize,
    /// The largest create/status answer read.
    pub max_body: usize,
    /// The largest `/responses` answer read (it repeats every response so far).
    pub max_responses_body: usize,
    /// The whole search, create to last poll (upstream route: 10 s).
    pub deadline: Duration,
    /// One request.
    pub request_timeout: Duration,
    /// The pause between polls (upstream 1 s).
    pub poll_interval: Duration,
    /// The one `DELETE` attempt afterwards.
    pub cleanup_timeout: Duration,
}

impl Default for SearchLimits {
    fn default() -> Self {
        SearchLimits {
            response_limit: 100,
            max_files: 1000,
            max_body: MAX_BODY,
            max_responses_body: 1024 * 1024,
            deadline: Duration::from_secs(10),
            request_timeout: Duration::from_secs(5),
            poll_interval: Duration::from_secs(1),
            cleanup_timeout: Duration::from_secs(5),
        }
    }
}

/// One shared file. Names come from remote peers: untrusted text, never a local path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchFile {
    pub filename: String,
    pub size: u64,
    /// Seconds.
    pub length: Option<u64>,
    pub sample_rate: Option<u64>,
    pub bit_depth: Option<u64>,
    /// kbit/s from size and length (upstream `slskd-client.ts`), 0 when either is unknown.
    pub bitrate: u64,
}

/// One user's answer, first one per user; locked files left out. [`search`] passes only a
/// non-blank `username` of at most [`MAX_USERNAME_BYTES`] and filenames of at most
/// [`MAX_FILENAME_BYTES`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResponse {
    pub username: String,
    pub has_free_upload_slot: bool,
    pub upload_speed: u64,
    pub queue_length: u64,
    pub files: Vec<SearchFile>,
}

/// How a search ended without error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchEnd {
    /// slskd reported `isComplete`.
    Complete,
    /// The deadline passed first.
    TimedOut,
    /// The file or response cap was reached.
    Capped,
}

/// Why a search failed. Carries no URL, key, query or slskd text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchError {
    /// 401/403: the key was refused.
    Rejected,
    /// The search could not be started: no answer within its time.
    NoAnswer,
    /// An error status, a redirect, an oversized or undecodable answer, or a bad search id.
    Unexpected,
    /// Not sent: the network guard refuses the host.
    Refused,
    /// Not sent: the process has no outbound client.
    Disabled,
    /// Not sent: [`MAX_SEARCHES`] are already running.
    Busy,
}

fn lenient<'de, D: Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let n = Option::<serde_json::Number>::deserialize(d)?;
    Ok(n.and_then(|n| {
        n.as_u64().or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite() && *f >= 0.0)
                .map(|f| f as u64)
        })
    }))
}

#[derive(Deserialize)]
struct Created {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    #[serde(default)]
    is_complete: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireResponse {
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    has_free_upload_slot: Option<bool>,
    #[serde(default, deserialize_with = "lenient")]
    upload_speed: Option<u64>,
    #[serde(default, deserialize_with = "lenient")]
    queue_length: Option<u64>,
    #[serde(default)]
    files: Option<Vec<WireFile>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireFile {
    #[serde(default)]
    filename: Option<String>,
    #[serde(default, deserialize_with = "lenient")]
    size: Option<u64>,
    #[serde(default, deserialize_with = "lenient")]
    length: Option<u64>,
    #[serde(default, deserialize_with = "lenient")]
    sample_rate: Option<u64>,
    #[serde(default, deserialize_with = "lenient")]
    bit_depth: Option<u64>,
    #[serde(default)]
    is_locked: Option<bool>,
}

impl WireFile {
    /// The file when it is shared (not locked) and its name is within bounds.
    fn into_file(self) -> Option<SearchFile> {
        if self.is_locked == Some(true) {
            return None;
        }
        let filename = self.filename.filter(|f| bounded(f, MAX_FILENAME_BYTES))?;
        let size = self.size.unwrap_or(0);
        let length = self.length.filter(|&l| l > 0);
        let bitrate = match length {
            Some(l) if size > 0 => ((size as f64 * 8.0) / (l as f64 * 1000.0)).round() as u64,
            _ => 0,
        };
        Some(SearchFile {
            filename,
            size,
            length,
            sample_rate: self.sample_rate.filter(|&r| r > 0),
            bit_depth: self.bit_depth.filter(|&b| b > 0),
            bitrate,
        })
    }
}

/// One search's fixed endpoints and captured credentials.
struct Endpoint {
    http: reqwest::Client,
    base: Url,
    key: Option<HeaderValue>,
}

/// What one request produced.
enum Answer {
    Body(Vec<u8>),
    /// 401/403.
    Rejected,
    /// A non-success status (redirects included, never followed).
    Status,
    /// No answer within its time.
    Silent,
    /// Over the size cap, or broken mid-body.
    Bad,
}

impl Endpoint {
    fn url(&self, path: &str) -> Url {
        let mut url = self.base.clone();
        url.set_path(path);
        url
    }

    async fn send(&self, rb: RequestBuilder, cap: usize, until: Instant) -> Answer {
        // An expired budget sends nothing (a ready timeout can still poll the request once).
        if Instant::now() >= until {
            return Answer::Silent;
        }
        let rb = with_key(rb.header(ACCEPT, "application/json"), &self.key);
        let work = async {
            let Ok(res) = rb.send().await else {
                return Answer::Silent;
            };
            let status = res.status();
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                return Answer::Rejected;
            }
            if !status.is_success() {
                return Answer::Status;
            }
            match read_capped(res, cap).await {
                Ok(body) => Answer::Body(body),
                Err(BodyError::Timeout) => Answer::Silent,
                Err(_) => Answer::Bad,
            }
        };
        tokio::time::timeout_at(until, work)
            .await
            .unwrap_or(Answer::Silent)
    }
}

/// Deletes the remote search once: awaited on every normal end, spawned from `Drop` when the
/// search future is cancelled (bounded by `cleanup_timeout`, same client and key).
struct Cleanup {
    endpoint: Option<Arc<Endpoint>>,
    url: Url,
    timeout: Duration,
}

impl Cleanup {
    async fn request(endpoint: Arc<Endpoint>, url: Url, timeout: Duration) {
        let until = Instant::now() + timeout;
        let rb = endpoint.http.delete(url);
        let _ = endpoint.send(rb, MAX_BODY, until).await;
    }

    async fn run(mut self) {
        if let Some(endpoint) = self.endpoint.take() {
            Cleanup::request(endpoint, self.url.clone(), self.timeout).await;
        }
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let Some(endpoint) = self.endpoint.take() else {
            return;
        };
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(Cleanup::request(endpoint, self.url.clone(), self.timeout));
        }
    }
}

/// Searches slskd for `query`: create, poll responses and status until complete, the deadline or
/// a cap, then delete the remote search. `on_response` sees each user's first response as it
/// arrives. Nothing is logged.
pub async fn search(
    url: &SlskdUrl,
    key: Option<&SlskdApiKey>,
    query: &str,
    limits: &SearchLimits,
    deny_real: bool,
    mut on_response: impl FnMut(SearchResponse),
) -> Result<SearchEnd, SearchError> {
    let base = url.0.clone();
    if crate::guard::refuses(&base, deny_real) {
        return Err(SearchError::Refused);
    }
    let deadline = Instant::now() + limits.deadline;
    let budget = |now: Instant| deadline.min(now + limits.request_timeout);
    let Some(http) = client(limits.request_timeout) else {
        return Err(SearchError::NoAnswer);
    };
    let key = key_header(key).map_err(|_| SearchError::Unexpected)?;
    let endpoint = Arc::new(Endpoint { http, base, key });

    let body = serde_json::json!({ "searchText": query, "responseLimit": limits.response_limit });
    let rb = endpoint
        .http
        .post(endpoint.url("/api/v0/searches"))
        .json(&body);
    let id = match endpoint
        .send(rb, limits.max_body, budget(Instant::now()))
        .await
    {
        Answer::Body(b) => serde_json::from_slice::<Created>(&b)
            .ok()
            .and_then(|c| Uuid::parse_str(&c.id).ok())
            .ok_or(SearchError::Unexpected)?,
        Answer::Rejected => return Err(SearchError::Rejected),
        Answer::Silent => return Err(SearchError::NoAnswer),
        Answer::Status | Answer::Bad => return Err(SearchError::Unexpected),
    };
    let search_path = format!("/api/v0/searches/{}", id.hyphenated());
    let cleanup = Cleanup {
        endpoint: Some(endpoint.clone()),
        url: endpoint.url(&search_path),
        timeout: limits.cleanup_timeout,
    };
    let end = poll(&endpoint, &search_path, limits, deadline, &mut on_response).await;
    cleanup.run().await;
    end
}

async fn poll(
    endpoint: &Endpoint,
    search_path: &str,
    limits: &SearchLimits,
    deadline: Instant,
    on_response: &mut impl FnMut(SearchResponse),
) -> Result<SearchEnd, SearchError> {
    let budget = |now: Instant| deadline.min(now + limits.request_timeout);
    let responses_url = endpoint.url(&format!("{search_path}/responses"));
    let status_url = endpoint.url(search_path);
    let mut seen_users = std::collections::HashSet::new();
    let mut files_left = limits.max_files;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Ok(SearchEnd::TimedOut);
        }
        tokio::time::sleep_until(deadline.min(now + limits.poll_interval)).await;
        if Instant::now() >= deadline {
            return Ok(SearchEnd::TimedOut);
        }

        let rb = endpoint.http.get(responses_url.clone());
        match endpoint
            .send(rb, limits.max_responses_body, budget(Instant::now()))
            .await
        {
            Answer::Body(b) => {
                let wire: Vec<WireResponse> =
                    serde_json::from_slice(&b).map_err(|_| SearchError::Unexpected)?;
                for resp in wire {
                    if seen_users.len() >= limits.response_limit as usize || files_left == 0 {
                        return Ok(SearchEnd::Capped);
                    }
                    // An invalid name skips the peer before it is stored or cloned.
                    let Some(username) = resp.username.filter(|u| bounded(u, MAX_USERNAME_BYTES))
                    else {
                        continue;
                    };
                    if !seen_users.insert(username.clone()) {
                        continue;
                    }
                    let files: Vec<SearchFile> = resp
                        .files
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(WireFile::into_file)
                        .take(files_left)
                        .collect();
                    files_left -= files.len();
                    on_response(SearchResponse {
                        username,
                        has_free_upload_slot: resp.has_free_upload_slot.unwrap_or(false),
                        upload_speed: resp.upload_speed.unwrap_or(0),
                        queue_length: resp.queue_length.unwrap_or(0),
                        files,
                    });
                }
                if files_left == 0 {
                    return Ok(SearchEnd::Capped);
                }
            }
            Answer::Rejected => return Err(SearchError::Rejected),
            Answer::Bad => return Err(SearchError::Unexpected),
            // Upstream keeps polling through a failed poll.
            Answer::Status | Answer::Silent => continue,
        }

        let rb = endpoint.http.get(status_url.clone());
        match endpoint
            .send(rb, limits.max_body, budget(Instant::now()))
            .await
        {
            Answer::Body(b) => {
                if serde_json::from_slice::<Status>(&b).is_ok_and(|s| s.is_complete) {
                    return Ok(SearchEnd::Complete);
                }
            }
            Answer::Rejected => return Err(SearchError::Rejected),
            Answer::Bad | Answer::Status | Answer::Silent => {}
        }
    }
}

/// A slot among the [`MAX_SEARCHES`]; the search keeps it until it ends.
pub struct SearchPermit(#[allow(dead_code)] OwnedSemaphorePermit);

impl crate::Net {
    /// [`check_connection`] under this process's network guard; a disabled client sends nothing.
    /// Not gated by the identification switch: slskd is a separate, admin-configured integration.
    pub async fn check_slskd(&self, url: &SlskdUrl, key: Option<&SlskdApiKey>) -> SlskdCheck {
        let Some(live) = &self.live else {
            return SlskdCheck::Disabled;
        };
        check_connection(url, key, live.cfg.deny_real, CHECK_TIMEOUT).await
    }

    /// Admits one search to `url` now, or says why not (no client, guard, all slots taken).
    pub fn slskd_search_permit(&self, url: &SlskdUrl) -> Result<SearchPermit, SearchError> {
        let live = self.live.as_ref().ok_or(SearchError::Disabled)?;
        if crate::guard::refuses(&url.0, live.cfg.deny_real) {
            return Err(SearchError::Refused);
        }
        live.slskd_searches
            .clone()
            .try_acquire_owned()
            .map(SearchPermit)
            .map_err(|_| SearchError::Busy)
    }

    /// [`search`] under this process's network guard. Callers hold a [`SearchPermit`].
    pub async fn search_slskd(
        &self,
        _permit: &SearchPermit,
        url: &SlskdUrl,
        key: Option<&SlskdApiKey>,
        query: &str,
        limits: &SearchLimits,
        on_response: impl FnMut(SearchResponse),
    ) -> Result<SearchEnd, SearchError> {
        let Some(live) = &self.live else {
            return Err(SearchError::Disabled);
        };
        search(url, key, query, limits, live.cfg.deny_real, on_response).await
    }
}
