//! The slskd connection check (M7 first slice) against a local stand-in server on an ephemeral
//! loopback port. No real slskd is contacted: every request here goes to 127.0.0.1 and is
//! recorded, so the test sees exactly what the adapter sent. Upstream contract:
//! `backend/src/lib/soulseek/slskd-client.ts` (`slskdRequest`, `login`) at kima-hub `b927e30`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use aether_net::slskd::{SlskdApiKey, SlskdCheck, SlskdUrl, check_connection};
use axum::Router;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    api_key: Option<String>,
    authorization: Option<String>,
}

#[derive(Clone)]
struct Stub {
    seen: Arc<Mutex<Vec<Seen>>>,
    port: u16,
}

impl Stub {
    fn url(&self) -> SlskdUrl {
        SlskdUrl::parse(&format!("http://127.0.0.1:{}", self.port)).unwrap()
    }
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

/// Serves `answer` for every request and records what arrived.
async fn stub<F>(answer: F) -> Stub
where
    F: Fn() -> Response + Clone + Send + Sync + 'static,
{
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = seen.clone();
    let app = Router::new().fallback(move |method: Method, uri: Uri, headers: HeaderMap| {
        let log = log.clone();
        let answer = answer.clone();
        async move {
            let get = |name: &str| {
                headers
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string)
            };
            log.lock().unwrap().push(Seen {
                method: method.to_string(),
                target: uri.to_string(),
                api_key: get("x-api-key"),
                authorization: get("authorization"),
            });
            answer()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Stub { seen, port }
}

fn json_answer(status: u16, body: Value) -> impl Fn() -> Response + Clone {
    move || {
        (
            StatusCode::from_u16(status).unwrap(),
            [(header::CONTENT_TYPE, "application/json")],
            body.to_string(),
        )
            .into_response()
    }
}

fn application(connected: bool, logged_in: bool) -> Value {
    json!({
        "version": { "full": "0.0.0-stub" },
        "server": { "address": "server.slsknet.org:2242", "state": "Connected, LoggedIn",
                    "isConnected": connected, "isLoggedIn": logged_in }
    })
}

const T: Duration = Duration::from_secs(5);

fn key(s: &str) -> SlskdApiKey {
    SlskdApiKey::new(s).unwrap()
}

#[test]
fn url_accepts_http_and_https_origins() {
    for ok in [
        "http://127.0.0.1:5030",
        "http://slskd.lan:5030/",
        "https://slskd.example",
        "  http://10.0.0.5:5030  ",
    ] {
        let url = SlskdUrl::parse(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
        assert!(!url.as_str().ends_with('/'), "{ok} -> {}", url.as_str());
    }
    assert_eq!(
        SlskdUrl::parse("http://slskd.lan:5030/").unwrap().as_str(),
        "http://slskd.lan:5030"
    );
}

#[test]
fn url_rejects_everything_but_a_plain_http_origin() {
    for bad in [
        "",
        "   ",
        "slskd.lan:5030",
        "ftp://slskd.lan",
        "file:///etc/passwd",
        "http://user:pass@slskd.lan:5030",
        "http://user@slskd.lan:5030",
        "http://slskd.lan:5030/?apikey=x",
        "http://slskd.lan:5030/#x",
        "http://slskd.lan:5030/slskd",
        "http://",
    ] {
        let err = SlskdUrl::parse(bad).expect_err(bad);
        // The message never quotes the input: it may carry credentials.
        let msg = err.to_string();
        assert!(!msg.contains("pass") && !msg.contains("apikey"), "{msg}");
    }
    let long = format!("http://{}.lan", "a".repeat(600));
    assert!(SlskdUrl::parse(&long).is_err());
}

#[test]
fn api_key_is_trimmed_bounded_and_never_printed() {
    assert!(SlskdApiKey::new("").is_none());
    assert!(SlskdApiKey::new("   ").is_none());
    assert!(SlskdApiKey::new(&"k".repeat(201)).is_none());
    let k = key("  secret-key-123  ");
    assert_eq!(k.expose(), "secret-key-123");
    assert!(!format!("{k:?}").contains("secret"));
}

#[test]
fn api_key_is_visible_us_ascii_only() {
    // Review M-1: the same boundary for the stored key and the probe's typed key.
    for bad in [
        "klucz-\u{105}",
        "key\u{7f}del",
        "key\r\nX-Evil: 1",
        "key\ttab",
        "key space",
        "key\0nul",
    ] {
        assert!(SlskdApiKey::new(bad).is_none(), "{bad:?}");
    }
    assert_eq!(
        key("  Ab0-_.~!#$%&*+/=?^|  ").expose(),
        "Ab0-_.~!#$%&*+/=?^|"
    );
}

#[test]
fn equivalent_urls_normalize_to_one_origin() {
    for same in [
        "http://SLSKD.Lan:80/",
        "HTTP://slskd.lan",
        "http://slskd.lan:80",
    ] {
        assert_eq!(
            SlskdUrl::parse(same).unwrap().as_str(),
            "http://slskd.lan",
            "{same}"
        );
    }
    assert_eq!(
        SlskdUrl::parse("https://slskd.lan:443/").unwrap().as_str(),
        "https://slskd.lan"
    );
}

#[tokio::test]
async fn connected_and_logged_in_is_connected() {
    let s = stub(json_answer(200, application(true, true))).await;
    let got = check_connection(&s.url(), Some(&key("k-123")), false, T).await;
    assert_eq!(got, SlskdCheck::Connected);
    let seen = s.seen();
    assert_eq!(seen.len(), 1, "{seen:?}");
    // Exactly upstream's probe: GET /api/v0/application with the key in X-API-Key.
    assert_eq!(seen[0].method, "GET");
    assert_eq!(seen[0].target, "/api/v0/application");
    assert_eq!(seen[0].api_key.as_deref(), Some("k-123"));
    assert_eq!(seen[0].authorization, None);
}

#[tokio::test]
async fn no_key_sends_no_header() {
    let s = stub(json_answer(200, application(true, true))).await;
    assert_eq!(
        check_connection(&s.url(), None, false, T).await,
        SlskdCheck::Connected
    );
    assert_eq!(s.seen()[0].api_key, None);
}

#[tokio::test]
async fn reachable_but_not_on_the_network_is_not_logged_in() {
    for (connected, logged_in) in [(true, false), (false, false), (false, true)] {
        let s = stub(json_answer(200, application(connected, logged_in))).await;
        assert_eq!(
            check_connection(&s.url(), None, false, T).await,
            SlskdCheck::NotLoggedIn,
            "{connected}/{logged_in}"
        );
    }
    // Upstream: `!app.server` is the same failure.
    let s = stub(json_answer(200, json!({ "version": {} }))).await;
    assert_eq!(
        check_connection(&s.url(), None, false, T).await,
        SlskdCheck::NotLoggedIn
    );
}

#[tokio::test]
async fn unauthorized_and_forbidden_are_rejected() {
    for status in [401, 403] {
        let s = stub(json_answer(status, json!({ "error": "nope" }))).await;
        assert_eq!(
            check_connection(&s.url(), Some(&key("bad")), false, T).await,
            SlskdCheck::Rejected,
            "{status}"
        );
    }
}

#[tokio::test]
async fn other_failures_are_unexpected() {
    for (status, body) in [
        (500, json!({ "error": "boom" })),
        (404, json!(null)),
        (200, json!("not an object")),
    ] {
        let s = stub(json_answer(status, body.clone())).await;
        assert_eq!(
            check_connection(&s.url(), None, false, T).await,
            SlskdCheck::Unexpected,
            "{status} {body}"
        );
    }
    let s = stub(|| (StatusCode::OK, "<html>not json</html>").into_response()).await;
    assert_eq!(
        check_connection(&s.url(), None, false, T).await,
        SlskdCheck::Unexpected
    );
}

#[tokio::test]
async fn oversized_answer_is_unexpected() {
    let big = "x".repeat(200 * 1024);
    let s = stub(move || (StatusCode::OK, big.clone()).into_response()).await;
    assert_eq!(
        check_connection(&s.url(), None, false, T).await,
        SlskdCheck::Unexpected
    );
}

#[tokio::test]
async fn redirect_is_not_followed_and_key_not_forwarded() {
    let other = stub(json_answer(200, application(true, true))).await;
    let to = format!("http://127.0.0.1:{}/api/v0/application", other.port);
    let s = stub(move || (StatusCode::FOUND, [(header::LOCATION, to.clone())], "").into_response())
        .await;
    assert_eq!(
        check_connection(&s.url(), Some(&key("k-123")), false, T).await,
        SlskdCheck::Unexpected
    );
    assert_eq!(s.seen().len(), 1);
    assert!(other.seen().is_empty(), "the redirect target was contacted");
}

#[tokio::test]
async fn closed_port_is_no_answer() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let url = SlskdUrl::parse(&format!("http://127.0.0.1:{port}")).unwrap();
    assert_eq!(
        check_connection(&url, Some(&key("k")), false, T).await,
        SlskdCheck::NoAnswer
    );
}

#[tokio::test]
async fn slow_answer_times_out_as_no_answer() {
    // Accepts the connection and never answers.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let held = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(30)).await;
        drop(socket);
    });
    let url = SlskdUrl::parse(&format!("http://127.0.0.1:{port}")).unwrap();
    let started = std::time::Instant::now();
    assert_eq!(
        check_connection(&url, None, false, Duration::from_millis(150)).await,
        SlskdCheck::NoAnswer
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    held.abort();
}

#[tokio::test]
async fn network_guard_refuses_a_real_host_without_sending_or_panicking() {
    // 192.0.2.0/24 is TEST-NET-1: never routed, and not contacted anyway.
    let url = SlskdUrl::parse("http://192.0.2.1:5030").unwrap();
    assert_eq!(
        check_connection(&url, Some(&key("k")), true, T).await,
        SlskdCheck::Refused
    );
    // The guard lets loopback through.
    let s = stub(json_answer(200, application(true, true))).await;
    assert_eq!(
        check_connection(&s.url(), None, true, T).await,
        SlskdCheck::Connected
    );
}
