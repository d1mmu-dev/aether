//! M7 Task 2 (D-292): `POST /api/soulseek/search` and `GET /api/soulseek/search/{searchId}`
//! (upstream `routes/soulseek.ts` at kima-hub `b927e30`) over a local stand-in slskd on an
//! ephemeral loopback port that records every request. Nothing reaches a real host
//! (`AETHER_NET_DENY_REAL=1`, `NetConfig::for_tests`).

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use aether_api::AppState;
use aether_core::identify::IdentifySwitch;
use aether_core::profile::ProfileKind;
use aether_db::PgPool;
use aether_db::profiles::{self, NewProfile};
use aether_net::slskd::SearchLimits;
use aether_net::{Bases, Net, NetConfig};
use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum_test::TestServer;
use serde_json::{Value, json};

const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";
const KEY: &str = "sk-synthetic-admin";
const NOT_CONFIGURED: &str =
    "Soulseek credentials not configured. Add username/password in System Settings.";

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    api_key: Option<String>,
    body: String,
}

#[derive(Clone)]
struct Stub {
    seen: Arc<Mutex<Vec<Seen>>>,
    port: u16,
}

impl Stub {
    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}

fn json_answer(body: Value) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// A stand-in slskd: creates search `ID`, answers `responses`, reports `complete`.
async fn stub(responses: Value, complete: bool) -> Stub {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = seen.clone();
    let app = Router::new().fallback(
        move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| {
            let log = log.clone();
            let responses = responses.clone();
            async move {
                log.lock().unwrap().push(Seen {
                    method: method.to_string(),
                    target: uri.to_string(),
                    api_key: headers
                        .get("x-api-key")
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string),
                    body: String::from_utf8_lossy(&body).into_owned(),
                });
                match (method.as_str(), uri.path()) {
                    ("POST", "/api/v0/searches") => json_answer(json!({ "id": ID })),
                    ("GET", p) if p.ends_with("/responses") => json_answer(responses),
                    ("GET", _) => json_answer(json!({ "id": ID, "isComplete": complete })),
                    ("DELETE", _) => StatusCode::NO_CONTENT.into_response(),
                    _ => StatusCode::NOT_FOUND.into_response(),
                }
            }
        },
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Stub { seen, port }
}

fn net(pool: &PgPool) -> Arc<Net> {
    Arc::new(Net::new(
        NetConfig::for_tests(Bases::all("http://127.0.0.1:9")),
        pool.clone(),
        IdentifySwitch::new(false, true),
    ))
}

fn fast(deadline: Duration) -> SearchLimits {
    SearchLimits {
        deadline,
        request_timeout: Duration::from_millis(500),
        poll_interval: Duration::from_millis(20),
        cleanup_timeout: Duration::from_millis(500),
        ..SearchLimits::default()
    }
}

/// Server, admin token, member token.
async fn setup(pool: PgPool, with_net: bool, limits: SearchLimits) -> (TestServer, String, String) {
    let mut state: AppState = common::state(pool.clone()).with_slskd_search_limits(limits);
    if with_net {
        state = state.with_net(net(&pool));
    }
    let (_, admin) = common::user_with_token(&state, "ana").await;
    let (_, user) = common::user_with_token(&state, "ben").await;
    let server = TestServer::new(aether_api::router(state)).unwrap();
    (server, admin, user)
}

async fn configure(server: &TestServer, admin: &str, body: Value) {
    let res = server
        .post("/api/system-settings")
        .authorization_bearer(admin)
        .json(&body)
        .await;
    assert_eq!(res.status_code(), StatusCode::OK, "{}", res.text());
}

async fn start(server: &TestServer, token: &str, body: Value) -> axum_test::TestResponse {
    server
        .post("/api/soulseek/search")
        .authorization_bearer(token)
        .json(&body)
        .await
}

/// Polls until `complete` (bounded).
async fn results(server: &TestServer, token: &str, id: &str) -> Value {
    for _ in 0..200 {
        let res = server
            .get(&format!("/api/soulseek/search/{id}"))
            .authorization_bearer(token)
            .await;
        assert_eq!(res.status_code(), StatusCode::OK, "{}", res.text());
        let body: Value = res.json();
        if body["complete"] == json!(true) {
            return body;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("search did not complete");
}

fn sample() -> Value {
    json!([
        {
            "username": "peer-a", "hasFreeUploadSlot": true, "uploadSpeed": 1000, "queueLength": 2,
            "files": [
                { "filename": "Music\\Artist\\Album\\01 - Song.flac", "size": 30000000, "length": 240 },
                { "filename": "Music\\Artist\\Album\\02 - Locked.flac", "size": 1, "isLocked": true }
            ]
        },
        { "username": "peer-b", "files": [ { "filename": "b/x.mp3", "size": 0 } ] }
    ])
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn search_needs_a_signed_in_caller(pool: PgPool) {
    let (server, _, _) = setup(pool, true, fast(Duration::from_secs(1))).await;
    let res = server
        .post("/api/soulseek/search")
        .json(&json!({ "query": "q" }))
        .await;
    assert_eq!(res.status_code(), StatusCode::UNAUTHORIZED);
    let res = server.get(&format!("/api/soulseek/search/{ID}")).await;
    assert_eq!(res.status_code(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn search_needs_the_switch_and_a_url(pool: PgPool) {
    let slskd = stub(sample(), true).await;
    let (server, admin, user) = setup(pool, true, fast(Duration::from_secs(1))).await;
    // Off by default, even with a URL.
    configure(&server, &admin, json!({ "slskdUrl": slskd.url() })).await;
    let res = start(&server, &user, json!({ "query": "q" })).await;
    assert_eq!(res.status_code(), StatusCode::FORBIDDEN);
    assert_eq!(res.json::<Value>()["error"], NOT_CONFIGURED);
    // On, but no URL.
    configure(
        &server,
        &admin,
        json!({ "soulseekEnabled": true, "slskdUrl": "" }),
    )
    .await;
    let res = start(&server, &user, json!({ "query": "q" })).await;
    assert_eq!(res.status_code(), StatusCode::FORBIDDEN);
    assert!(
        slskd.seen().is_empty(),
        "nothing may be sent: {:?}",
        slskd.seen()
    );
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn search_refuses_bad_bodies(pool: PgPool) {
    let slskd = stub(sample(), true).await;
    let (server, admin, user) = setup(pool, true, fast(Duration::from_secs(1))).await;
    configure(
        &server,
        &admin,
        json!({ "soulseekEnabled": true, "slskdUrl": slskd.url() }),
    )
    .await;
    for body in [
        json!({}),
        json!({ "artist": "a" }),
        json!({ "title": "t" }),
        json!({ "query": "" }),
        json!({ "query": 5 }),
        json!(["q"]),
    ] {
        let res = start(&server, &user, body.clone()).await;
        assert_eq!(res.status_code(), StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(
            res.json::<Value>()["error"],
            "Either 'query' or both 'artist' and 'title' are required"
        );
    }
    let res = start(&server, &user, json!({ "query": "x".repeat(201) })).await;
    assert_eq!(res.status_code(), StatusCode::BAD_REQUEST);
    assert_eq!(res.json::<Value>()["error"], "query is too long");
    assert!(slskd.seen().is_empty());
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn a_member_searches_and_reads_upstream_shaped_results(pool: PgPool) {
    let slskd = stub(sample(), true).await;
    let (server, admin, user) = setup(pool, true, fast(Duration::from_secs(2))).await;
    configure(
        &server,
        &admin,
        json!({ "soulseekEnabled": true, "slskdUrl": slskd.url(), "slskdApiKey": KEY }),
    )
    .await;

    let res = start(
        &server,
        &user,
        json!({ "artist": "Some Artist", "title": "Song" }),
    )
    .await;
    assert_eq!(res.status_code(), StatusCode::OK, "{}", res.text());
    let started: Value = res.json();
    assert_eq!(started["message"], "Search started");
    let id = started["searchId"].as_str().unwrap().to_string();
    assert!(uuid::Uuid::parse_str(&id).is_ok());
    assert!(!res.text().contains(KEY));

    let body = results(&server, &user, &id).await;
    assert_eq!(
        body,
        json!({
            "results": [
                {
                    "username": "peer-a",
                    "path": "Music\\Artist\\Album\\01 - Song.flac",
                    "filename": "01 - Song.flac",
                    "size": 30000000,
                    "bitrate": 1000,
                    "format": "flac",
                    "parsedArtist": "Artist",
                    "parsedAlbum": "Album",
                    "parsedTitle": "Song"
                },
                {
                    "username": "peer-b",
                    "path": "b/x.mp3",
                    "filename": "x.mp3",
                    "size": 0,
                    "bitrate": 0,
                    "format": "mp3",
                    "parsedAlbum": "b",
                    "parsedTitle": "x"
                }
            ],
            "count": 2,
            "complete": true
        })
    );

    // The upstream wire, the admin's key sent only to slskd, and the remote search deleted.
    let mut seen = Vec::new();
    for _ in 0..100 {
        seen = slskd.seen();
        if seen.iter().any(|s| s.method == "DELETE") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let post: Value = serde_json::from_str(&seen[0].body).unwrap();
    assert_eq!(
        post,
        json!({ "searchText": "Some Artist Song", "responseLimit": 100 })
    );
    assert!(seen.iter().all(|s| s.api_key.as_deref() == Some(KEY)));
    let deletes: Vec<_> = seen.iter().filter(|s| s.method == "DELETE").collect();
    assert_eq!(deletes.len(), 1);
    assert_eq!(deletes[0].target, format!("/api/v0/searches/{ID}"));
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn another_caller_cannot_read_a_search(pool: PgPool) {
    let slskd = stub(sample(), true).await;
    let (server, admin, user) = setup(pool, true, fast(Duration::from_secs(2))).await;
    configure(
        &server,
        &admin,
        json!({ "soulseekEnabled": true, "slskdUrl": slskd.url() }),
    )
    .await;
    let id = start(&server, &user, json!({ "query": "q" }))
        .await
        .json::<Value>()["searchId"]
        .as_str()
        .unwrap()
        .to_string();
    let missing = json!({ "error": "Search not found or expired", "results": [], "count": 0 });
    for (token, path) in [
        (&admin, format!("/api/soulseek/search/{id}")),
        (&user, format!("/api/soulseek/search/{ID}")),
        (&user, "/api/soulseek/search/not-a-uuid".to_string()),
    ] {
        let res = server.get(&path).authorization_bearer(token).await;
        assert_eq!(res.status_code(), StatusCode::NOT_FOUND, "{path}");
        assert_eq!(res.json::<Value>(), missing);
    }
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn a_sibling_profile_of_the_same_account_cannot_read_a_search(pool: PgPool) {
    let slskd = stub(sample(), true).await;
    let state = common::state(pool.clone())
        .with_slskd_search_limits(fast(Duration::from_secs(2)))
        .with_net(net(&pool));
    let (ana, admin) = common::user_with_token(&state, "ana").await;
    let teen = profiles::create(
        &state.pool,
        ana.account(),
        NewProfile {
            name: "Teen".into(),
            kind: ProfileKind::Standard,
            avatar: None,
        },
    )
    .await
    .unwrap()
    .unwrap();
    let teen_token = state.jwt.issue_access(
        ana.account(),
        teen.id,
        &ana.username,
        &ana.role,
        ana.token_version,
    );
    let server = TestServer::new(aether_api::router(state)).unwrap();
    configure(
        &server,
        &admin,
        json!({ "soulseekEnabled": true, "slskdUrl": slskd.url() }),
    )
    .await;
    let id = start(&server, &teen_token, json!({ "query": "q" }))
        .await
        .json::<Value>()["searchId"]
        .as_str()
        .unwrap()
        .to_string();
    let res = server
        .get(&format!("/api/soulseek/search/{id}"))
        .authorization_bearer(&admin)
        .await;
    assert_eq!(res.status_code(), StatusCode::NOT_FOUND);
    let own = results(&server, &teen_token, &id).await;
    assert_eq!(own["count"], 2);
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn searches_beyond_the_limit_are_refused(pool: PgPool) {
    let slskd = stub(json!([]), false).await;
    let (server, admin, user) = setup(pool, true, fast(Duration::from_secs(3))).await;
    configure(
        &server,
        &admin,
        json!({ "soulseekEnabled": true, "slskdUrl": slskd.url() }),
    )
    .await;
    for _ in 0..aether_net::slskd::MAX_SEARCHES {
        let res = start(&server, &user, json!({ "query": "q" })).await;
        assert_eq!(res.status_code(), StatusCode::OK);
    }
    let res = start(&server, &user, json!({ "query": "q" })).await;
    assert_eq!(res.status_code(), StatusCode::TOO_MANY_REQUESTS);
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn search_without_an_outbound_client_is_a_conflict(pool: PgPool) {
    let slskd = stub(sample(), true).await;
    let (server, admin, user) = setup(pool, false, fast(Duration::from_secs(1))).await;
    configure(
        &server,
        &admin,
        json!({ "soulseekEnabled": true, "slskdUrl": slskd.url() }),
    )
    .await;
    let res = start(&server, &user, json!({ "query": "q" })).await;
    assert_eq!(res.status_code(), StatusCode::CONFLICT);
    assert_eq!(
        res.json::<Value>()["error"],
        "Outbound connections are turned off"
    );
    assert!(slskd.seen().is_empty());
}
