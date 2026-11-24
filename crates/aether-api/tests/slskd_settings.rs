//! M7 first slice: slskd settings in `/api/system-settings` and the admin connection check
//! `POST /api/system-settings/test-soulseek` (upstream `routes/systemSettings.ts` at kima-hub
//! `b927e30`, slskd mode only). slskd is a local stand-in on an ephemeral loopback port that
//! records every request; nothing reaches a real host (`AETHER_NET_DENY_REAL=1`,
//! `NetConfig::for_tests`).

mod common;

use std::sync::{Arc, Mutex};

use aether_api::AppState;
use aether_core::identify::IdentifySwitch;
use aether_db::PgPool;
use aether_net::{Bases, Net, NetConfig};
use axum::Router;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum_test::TestServer;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    api_key: Option<String>,
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

async fn stub(status: u16, body: Value) -> Stub {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = seen.clone();
    let app = Router::new().fallback(move |method: Method, uri: Uri, headers: HeaderMap| {
        let log = log.clone();
        let body = body.clone();
        async move {
            log.lock().unwrap().push(Seen {
                method: method.to_string(),
                target: uri.to_string(),
                api_key: headers
                    .get("x-api-key")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string),
            });
            let r: Response = (
                StatusCode::from_u16(status).unwrap(),
                [(header::CONTENT_TYPE, "application/json")],
                body.to_string(),
            )
                .into_response();
            r
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Stub { seen, port }
}

fn application(connected: bool, logged_in: bool) -> Value {
    json!({ "server": { "isConnected": connected, "isLoggedIn": logged_in } })
}

fn net(pool: &PgPool) -> Arc<Net> {
    Arc::new(Net::new(
        NetConfig::for_tests(Bases::all("http://127.0.0.1:9")),
        pool.clone(),
        IdentifySwitch::new(false, true),
    ))
}

async fn setup(pool: PgPool, with_net: bool) -> (TestServer, AppState, String, String) {
    let mut state = common::state(pool.clone());
    if with_net {
        state = state.with_net(net(&pool));
    }
    let (_, admin) = common::user_with_token(&state, "ana").await;
    let (_, user) = common::user_with_token(&state, "ben").await;
    let server = TestServer::new(aether_api::router(state.clone())).unwrap();
    (server, state, admin, user)
}

async fn get(server: &TestServer, token: &str) -> Value {
    server
        .get("/api/system-settings")
        .authorization_bearer(token)
        .await
        .json()
}

async fn save(server: &TestServer, token: &str, body: Value) -> axum_test::TestResponse {
    server
        .post("/api/system-settings")
        .authorization_bearer(token)
        .json(&body)
        .await
}

async fn test_soulseek(server: &TestServer, token: &str, body: Value) -> axum_test::TestResponse {
    server
        .post("/api/system-settings/test-soulseek")
        .authorization_bearer(token)
        .json(&body)
        .await
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn defaults_are_disabled_and_empty(pool: PgPool) {
    let (server, _, admin, _) = setup(pool, false).await;
    let body = get(&server, &admin).await;
    assert_eq!(body["soulseekEnabled"], false);
    assert_eq!(body["slskdUrl"], "");
    assert_eq!(body["slskdApiKey"], "");
    assert_eq!(body["hasSlskdApiKey"], false);
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn saves_normalized_url_and_encrypted_key_never_echoed(pool: PgPool) {
    let (server, _, admin, _) = setup(pool.clone(), false).await;
    save(
        &server,
        &admin,
        json!({ "soulseekEnabled": true, "slskdUrl": " http://192.168.1.20:5030/ ",
                "slskdApiKey": "  sekret-key-42  " }),
    )
    .await
    .assert_status_ok();
    let body = get(&server, &admin).await;
    assert_eq!(body["soulseekEnabled"], true);
    assert_eq!(body["slskdUrl"], "http://192.168.1.20:5030");
    assert_eq!(body["slskdApiKey"], "");
    assert_eq!(body["hasSlskdApiKey"], true);
    assert!(!body.to_string().contains("sekret"));
    let blob: Vec<u8> =
        sqlx::query_scalar("SELECT secret_enc FROM settings_kv WHERE key = 'slskdApiKey'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!blob.windows(6).any(|w| w == b"sekret"));
    // "" and null clear the URL; an empty key is not a fresh one, so clearing the URL also drops
    // the key bound to it (review I-1). A blank key with an unchanged URL keeps it (see
    // `equivalent_url_with_blank_key_keeps_the_key`).
    save(
        &server,
        &admin,
        json!({ "slskdApiKey": "", "slskdUrl": "" }),
    )
    .await
    .assert_status_ok();
    let body = get(&server, &admin).await;
    assert_eq!(body["slskdUrl"], "");
    assert_eq!(body["hasSlskdApiKey"], false);
    save(
        &server,
        &admin,
        json!({ "slskdApiKey": null, "slskdUrl": null, "soulseekEnabled": false }),
    )
    .await
    .assert_status_ok();
    let body = get(&server, &admin).await;
    assert_eq!(body["hasSlskdApiKey"], false);
    assert_eq!(body["soulseekEnabled"], false);
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn invalid_values_are_400_and_store_nothing(pool: PgPool) {
    let (server, _, admin, _) = setup(pool, false).await;
    for (body, details) in [
        (
            json!({ "slskdUrl": "ftp://slskd.lan" }),
            "slskdUrl must start with http:// or https://",
        ),
        (
            json!({ "slskdUrl": "http://admin:hunter2@slskd.lan:5030" }),
            "slskdUrl must not contain credentials",
        ),
        (
            json!({ "slskdUrl": "http://slskd.lan:5030/?apikey=hunter2" }),
            "slskdUrl must not have a path, query or fragment",
        ),
        (
            json!({ "slskdUrl": 5030 }),
            "slskdUrl must be a string or null",
        ),
        (
            json!({ "soulseekEnabled": "yes" }),
            "soulseekEnabled must be a boolean",
        ),
        (
            json!({ "slskdApiKey": 42 }),
            "slskdApiKey must be a string or null",
        ),
        (
            json!({ "slskdApiKey": "k".repeat(201) }),
            "slskdApiKey is too long",
        ),
        // Validated together: the good field in a rejected body is not stored either.
        (
            json!({ "soulseekEnabled": true, "slskdUrl": "nope" }),
            "slskdUrl is not a valid URL",
        ),
    ] {
        let res = save(&server, &admin, body.clone()).await;
        res.assert_status_bad_request();
        res.assert_json(&json!({ "error": "Invalid settings", "details": details }));
        assert!(!res.text().contains("hunter2"), "{body}");
    }
    let body = get(&server, &admin).await;
    assert_eq!(body["soulseekEnabled"], false);
    assert_eq!(body["slskdUrl"], "");
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn saving_and_reading_settings_contacts_nothing(pool: PgPool) {
    let slskd = stub(200, application(true, true)).await;
    let (server, _, admin, _) = setup(pool, true).await;
    save(
        &server,
        &admin,
        json!({ "soulseekEnabled": true, "slskdUrl": slskd.url(), "slskdApiKey": "k" }),
    )
    .await
    .assert_status_ok();
    get(&server, &admin).await;
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(slskd.seen().is_empty(), "{:?}", slskd.seen());
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn check_uses_stored_url_and_key(pool: PgPool) {
    let slskd = stub(200, application(true, true)).await;
    let (server, _, admin, _) = setup(pool, true).await;
    save(
        &server,
        &admin,
        json!({ "slskdUrl": slskd.url(), "slskdApiKey": "stored-key" }),
    )
    .await
    .assert_status_ok();
    // The check does not need the integration switched on (upstream tests before saving).
    let res = test_soulseek(&server, &admin, json!({})).await;
    res.assert_status_ok();
    res.assert_json(
        &json!({ "success": true, "message": "Connected to slskd", "isConnected": true }),
    );
    let seen = slskd.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].method, "GET");
    assert_eq!(seen[0].target, "/api/v0/application");
    assert_eq!(seen[0].api_key.as_deref(), Some("stored-key"));
    // Upstream's form posts mode "slskd" with the URL being edited.
    test_soulseek(
        &server,
        &admin,
        json!({ "mode": "slskd", "slskdUrl": slskd.url(), "slskdApiKey": "" }),
    )
    .await
    .assert_status_ok();
    assert_eq!(slskd.seen()[1].api_key.as_deref(), Some("stored-key"));
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn stored_key_never_goes_to_another_address(pool: PgPool) {
    let stored = stub(200, application(true, true)).await;
    let candidate = stub(200, application(true, true)).await;
    let (server, _, admin, _) = setup(pool, true).await;
    save(
        &server,
        &admin,
        json!({ "slskdUrl": stored.url(), "slskdApiKey": "stored-key" }),
    )
    .await
    .assert_status_ok();
    test_soulseek(&server, &admin, json!({ "slskdUrl": candidate.url() }))
        .await
        .assert_status_ok();
    assert_eq!(candidate.seen()[0].api_key, None);
    test_soulseek(
        &server,
        &admin,
        json!({ "slskdUrl": candidate.url(), "slskdApiKey": "typed-key" }),
    )
    .await
    .assert_status_ok();
    assert_eq!(candidate.seen()[1].api_key.as_deref(), Some("typed-key"));
    assert!(stored.seen().is_empty());
    // A candidate is only tested, never saved.
    assert_eq!(get(&server, &admin).await["slskdUrl"], stored.url());
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn failures_are_502_without_secrets(pool: PgPool) {
    let not_logged_in = stub(200, application(true, false)).await;
    let rejected = stub(401, json!({ "error": "bad key secret-echo" })).await;
    let broken = stub(500, json!({ "error": "boom secret-echo" })).await;
    let closed = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        format!("http://127.0.0.1:{port}")
    };
    let (server, _, admin, _) = setup(pool, true).await;
    for (url, message) in [
        (
            not_logged_in.url(),
            "slskd is not connected to the Soulseek network",
        ),
        (rejected.url(), "slskd rejected the API key"),
        (broken.url(), "slskd answered unexpectedly"),
        (closed, "slskd did not answer"),
    ] {
        let res = test_soulseek(
            &server,
            &admin,
            json!({ "slskdUrl": url, "slskdApiKey": "typed-secret" }),
        )
        .await;
        assert_eq!(res.status_code(), StatusCode::BAD_GATEWAY, "{url}");
        res.assert_json(&json!({ "error": message }));
        let text = res.text();
        assert!(
            !text.contains("typed-secret")
                && !text.contains("secret-echo")
                && !text.contains("127.0.0.1")
        );
    }
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn bad_requests_are_400(pool: PgPool) {
    let (server, _, admin, _) = setup(pool, true).await;
    for (body, error) in [
        (json!({}), "slskd URL is required"),
        (json!({ "slskdUrl": "" }), "slskd URL is required"),
        (
            json!({ "mode": "p2p", "username": "u", "password": "p" }),
            "Only slskd can be tested for now",
        ),
        (
            json!({ "slskdUrl": "http://u:hunter2@h:1" }),
            "slskdUrl must not contain credentials",
        ),
        (json!({ "slskdUrl": 1 }), "Invalid request"),
        (json!({ "mode": 1 }), "Invalid request"),
        (
            json!({ "slskdUrl": "http://127.0.0.1:9", "slskdApiKey": ["x"] }),
            "Invalid request",
        ),
    ] {
        let res = test_soulseek(&server, &admin, body.clone()).await;
        res.assert_status_bad_request();
        res.assert_json(&json!({ "error": error }));
        assert!(!res.text().contains("hunter2"), "{body}");
    }
    let res = server
        .post("/api/system-settings/test-soulseek")
        .authorization_bearer(&admin)
        .bytes("{not json".into())
        .content_type("application/json")
        .await;
    res.assert_status_bad_request();
    res.assert_json(&json!({ "error": "Invalid request" }));
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn guard_or_disabled_client_sends_nothing(pool: PgPool) {
    // The network guard (`for_tests`: deny_real) refuses a non-loopback host without a panic.
    let (server, _, admin, _) = setup(pool.clone(), true).await;
    let res = test_soulseek(
        &server,
        &admin,
        json!({ "slskdUrl": "http://192.0.2.1:5030" }),
    )
    .await;
    assert_eq!(res.status_code(), StatusCode::CONFLICT);
    res.assert_json(&json!({ "error": "Outbound connections are turned off" }));
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn disabled_client_sends_nothing(pool: PgPool) {
    // A process without an outbound client answers like the guard.
    let slskd = stub(200, application(true, true)).await;
    let (server, _, admin, _) = setup(pool, false).await;
    let res = test_soulseek(&server, &admin, json!({ "slskdUrl": slskd.url() })).await;
    assert_eq!(res.status_code(), StatusCode::CONFLICT);
    assert!(slskd.seen().is_empty());
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn admin_only(pool: PgPool) {
    let slskd = stub(200, application(true, true)).await;
    let (server, _, _, user) = setup(pool, true).await;
    test_soulseek(&server, &user, json!({ "slskdUrl": slskd.url() }))
        .await
        .assert_status_forbidden();
    server
        .post("/api/system-settings/test-soulseek")
        .json(&json!({}))
        .await
        .assert_status_unauthorized();
    save(&server, &user, json!({ "slskdUrl": slskd.url() }))
        .await
        .assert_status_forbidden();
    assert!(slskd.seen().is_empty());
}

// --- Review I-1: the stored key is bound to the stored URL (D-291). ---

const KEY_A: &str = "fixture-key-a";
const KEY_B: &str = "fixture-key-b";

/// The raw `settings_kv` rows of the slskd pair, for "nothing changed" checks.
async fn pair_rows(pool: &PgPool) -> Vec<(String, Option<Value>, Option<Vec<u8>>)> {
    sqlx::query_as(
        "SELECT key, value, secret_enc FROM settings_kv
         WHERE key IN ('slskdUrl', 'slskdApiKey') ORDER BY key",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn url_change_without_key_drops_the_stored_key(pool: PgPool) {
    let a = stub(200, application(true, true)).await;
    let b = stub(200, application(true, true)).await;
    let (server, _, admin, _) = setup(pool, true).await;
    save(
        &server,
        &admin,
        json!({ "slskdUrl": a.url(), "slskdApiKey": KEY_A }),
    )
    .await
    .assert_status_ok();
    save(&server, &admin, json!({ "slskdUrl": b.url() }))
        .await
        .assert_status_ok();
    let body = get(&server, &admin).await;
    assert_eq!(body["slskdUrl"], b.url());
    assert_eq!(body["hasSlskdApiKey"], false);
    test_soulseek(&server, &admin, json!({}))
        .await
        .assert_status_ok();
    let seen = b.seen();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].api_key.is_none(), "the old key went to the new URL");
    assert!(a.seen().is_empty());
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn url_change_with_a_fresh_key_keeps_only_the_new_key(pool: PgPool) {
    let a = stub(200, application(true, true)).await;
    let b = stub(200, application(true, true)).await;
    let (server, _, admin, _) = setup(pool, true).await;
    save(
        &server,
        &admin,
        json!({ "slskdUrl": a.url(), "slskdApiKey": KEY_A }),
    )
    .await
    .assert_status_ok();
    save(
        &server,
        &admin,
        json!({ "slskdUrl": b.url(), "slskdApiKey": KEY_B }),
    )
    .await
    .assert_status_ok();
    test_soulseek(&server, &admin, json!({}))
        .await
        .assert_status_ok();
    assert_eq!(b.seen()[0].api_key.as_deref(), Some(KEY_B));
    assert!(a.seen().is_empty());
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn equivalent_url_with_blank_key_keeps_the_key(pool: PgPool) {
    let a = stub(200, application(true, true)).await;
    let (server, _, admin, _) = setup(pool, true).await;
    save(
        &server,
        &admin,
        json!({ "slskdUrl": a.url(), "slskdApiKey": KEY_A }),
    )
    .await
    .assert_status_ok();
    // Same origin after normalization: scheme case and the trailing slash do not matter.
    let same = format!("HTTP://127.0.0.1:{}/", a.port);
    save(
        &server,
        &admin,
        json!({ "slskdUrl": same, "slskdApiKey": "" }),
    )
    .await
    .assert_status_ok();
    let body = get(&server, &admin).await;
    assert_eq!(body["slskdUrl"], a.url());
    assert_eq!(body["hasSlskdApiKey"], true);
    test_soulseek(&server, &admin, json!({}))
        .await
        .assert_status_ok();
    assert_eq!(a.seen()[0].api_key.as_deref(), Some(KEY_A));
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn cleared_url_then_new_url_has_no_key(pool: PgPool) {
    let a = stub(200, application(true, true)).await;
    let b = stub(200, application(true, true)).await;
    let (server, _, admin, _) = setup(pool, true).await;
    save(
        &server,
        &admin,
        json!({ "slskdUrl": a.url(), "slskdApiKey": KEY_A }),
    )
    .await
    .assert_status_ok();
    save(&server, &admin, json!({ "slskdUrl": null }))
        .await
        .assert_status_ok();
    assert_eq!(get(&server, &admin).await["hasSlskdApiKey"], false);
    save(&server, &admin, json!({ "slskdUrl": b.url() }))
        .await
        .assert_status_ok();
    assert_eq!(get(&server, &admin).await["hasSlskdApiKey"], false);
    test_soulseek(&server, &admin, json!({}))
        .await
        .assert_status_ok();
    assert!(
        b.seen()[0].api_key.is_none(),
        "the old key went to the new URL"
    );
    assert!(a.seen().is_empty());
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn key_only_save_keeps_the_url(pool: PgPool) {
    let a = stub(200, application(true, true)).await;
    let (server, _, admin, _) = setup(pool, true).await;
    save(
        &server,
        &admin,
        json!({ "slskdUrl": a.url(), "slskdApiKey": KEY_A }),
    )
    .await
    .assert_status_ok();
    save(&server, &admin, json!({ "slskdApiKey": KEY_B }))
        .await
        .assert_status_ok();
    assert_eq!(get(&server, &admin).await["slskdUrl"], a.url());
    test_soulseek(&server, &admin, json!({}))
        .await
        .assert_status_ok();
    assert_eq!(a.seen()[0].api_key.as_deref(), Some(KEY_B));
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn failed_pair_write_rolls_back_the_url(pool: PgPool) {
    let a = stub(200, application(true, true)).await;
    let b = stub(200, application(true, true)).await;
    let (server, _, admin, _) = setup(pool.clone(), true).await;
    save(
        &server,
        &admin,
        json!({ "slskdUrl": a.url(), "slskdApiKey": KEY_A }),
    )
    .await
    .assert_status_ok();
    let before = pair_rows(&pool).await;
    // Any write of the key row fails, after the URL row was written in the same save.
    sqlx::raw_sql(
        "CREATE FUNCTION fail_slskd_key() RETURNS trigger LANGUAGE plpgsql AS $$
         BEGIN
           IF (TG_OP = 'DELETE' AND OLD.key = 'slskdApiKey')
              OR (TG_OP <> 'DELETE' AND NEW.key = 'slskdApiKey') THEN
             RAISE EXCEPTION 'injected key write failure';
           END IF;
           IF TG_OP = 'DELETE' THEN RETURN OLD; END IF;
           RETURN NEW;
         END $$;
         CREATE TRIGGER fail_slskd_key BEFORE INSERT OR UPDATE OR DELETE ON settings_kv
         FOR EACH ROW EXECUTE FUNCTION fail_slskd_key()",
    )
    .execute(&pool)
    .await
    .unwrap();
    // Case labels, not bodies, in failure messages: no fixture key in the logs.
    for (case, body) in [
        (
            "url+key",
            json!({ "slskdUrl": b.url(), "slskdApiKey": KEY_B }),
        ),
        ("url only", json!({ "slskdUrl": b.url() })),
    ] {
        let res = save(&server, &admin, body).await;
        assert_eq!(
            res.status_code(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "{case}"
        );
        assert!(!res.text().contains(KEY_B), "{case}");
        assert_eq!(pair_rows(&pool).await, before, "{case}");
    }
    sqlx::raw_sql("DROP TRIGGER fail_slskd_key ON settings_kv")
        .execute(&pool)
        .await
        .unwrap();
    test_soulseek(&server, &admin, json!({}))
        .await
        .assert_status_ok();
    assert_eq!(a.seen()[0].api_key.as_deref(), Some(KEY_A));
    assert!(b.seen().is_empty());
}

// --- Review M-1: a key must be a usable, visible US-ASCII header value. ---

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn invalid_key_is_400_and_changes_nothing(pool: PgPool) {
    let a = stub(200, application(true, true)).await;
    let b = stub(200, application(true, true)).await;
    let (server, _, admin, _) = setup(pool.clone(), true).await;
    save(
        &server,
        &admin,
        json!({ "slskdUrl": a.url(), "slskdApiKey": KEY_A }),
    )
    .await
    .assert_status_ok();
    let before = pair_rows(&pool).await;
    for bad in [
        "klucz-ą",
        "key\u{7f}del",
        "key\r\nX-Evil: 1",
        "key\ttab",
        "key space",
    ] {
        let res = save(
            &server,
            &admin,
            json!({ "soulseekEnabled": true, "slskdUrl": b.url(), "slskdApiKey": bad }),
        )
        .await;
        res.assert_status_bad_request();
        res.assert_json(
            &json!({ "error": "Invalid settings", "details": "slskdApiKey is invalid" }),
        );
        assert_eq!(pair_rows(&pool).await, before, "{bad:?}");
        let res = test_soulseek(
            &server,
            &admin,
            json!({ "slskdUrl": b.url(), "slskdApiKey": bad }),
        )
        .await;
        res.assert_status_bad_request();
        assert!(!res.text().contains("klucz"));
    }
    let body = get(&server, &admin).await;
    assert_eq!(body["soulseekEnabled"], false);
    assert_eq!(body["slskdUrl"], a.url());
    assert!(a.seen().is_empty() && b.seen().is_empty());
}
