//! slskd search (M7 Task 2, D-292) against a local stand-in slskd on an ephemeral loopback port.
//! No real slskd or public host is contacted; every request is recorded. Upstream contract:
//! `backend/src/lib/soulseek/slskd-client.ts` `search()` at kima-hub `b927e30`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aether_net::slskd::{
    SearchEnd, SearchError, SearchLimits, SearchResponse, SlskdApiKey, SlskdUrl, search,
};
use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

const ID: &str = "0f8fad5b-d9cb-469f-a165-70867728950e";

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    api_key: Option<String>,
    body: String,
}

/// What the stand-in answers, by request.
#[derive(Clone)]
struct Script {
    create: Arc<dyn Fn() -> Response + Send + Sync>,
    responses: Arc<dyn Fn() -> Response + Send + Sync>,
    status: Arc<dyn Fn() -> Response + Send + Sync>,
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
    fn deletes(&self) -> Vec<Seen> {
        self.seen()
            .into_iter()
            .filter(|s| s.method == "DELETE")
            .collect()
    }
}

fn json_answer(status: u16, body: Value) -> Response {
    (
        StatusCode::from_u16(status).unwrap(),
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

async fn stub(script: Script) -> Stub {
    let seen: Arc<Mutex<Vec<Seen>>> = Arc::default();
    let log = seen.clone();
    let app = Router::new().fallback(
        move |method: Method, uri: Uri, headers: HeaderMap, body: Bytes| {
            let log = log.clone();
            let script = script.clone();
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
                let path = uri.path().to_string();
                match (method.as_str(), path.as_str()) {
                    ("POST", "/api/v0/searches") => (script.create)(),
                    ("GET", p) if p.ends_with("/responses") => (script.responses)(),
                    ("GET", _) => (script.status)(),
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

fn created() -> Arc<dyn Fn() -> Response + Send + Sync> {
    Arc::new(|| json_answer(200, json!({ "id": ID, "searchText": "x" })))
}

fn answers(v: Value) -> Arc<dyn Fn() -> Response + Send + Sync> {
    Arc::new(move || json_answer(200, v.clone()))
}

fn complete(done: bool) -> Arc<dyn Fn() -> Response + Send + Sync> {
    answers(json!({ "id": ID, "isComplete": done }))
}

fn fast() -> SearchLimits {
    SearchLimits {
        deadline: Duration::from_millis(800),
        request_timeout: Duration::from_millis(500),
        poll_interval: Duration::from_millis(20),
        cleanup_timeout: Duration::from_millis(500),
        ..SearchLimits::default()
    }
}

fn sample_responses() -> Value {
    json!([
        {
            "username": "peer-a",
            "hasFreeUploadSlot": true,
            "uploadSpeed": 1000,
            "queueLength": 2,
            "token": 7,
            "files": [
                { "filename": "Music\\Artist\\Album\\01 - Song.flac", "size": 30000000,
                  "length": 240, "sampleRate": 44100, "bitDepth": 16 },
                { "filename": "Music\\Artist\\Album\\02 - Locked.flac", "size": 1, "isLocked": true }
            ]
        },
        { "username": "peer-a", "hasFreeUploadSlot": false, "files": [ { "filename": "dup.mp3", "size": 5 } ] },
        { "username": "peer-b", "hasFreeUploadSlot": false, "files": [ { "filename": "b/x.mp3", "size": 0 } ] }
    ])
}

async fn run(
    stub: &Stub,
    limits: SearchLimits,
) -> (Result<SearchEnd, SearchError>, Vec<SearchResponse>) {
    let key = SlskdApiKey::new("sk-synthetic").unwrap();
    let mut got = Vec::new();
    let end = search(&stub.url(), Some(&key), "artist song", &limits, true, |r| {
        got.push(r)
    })
    .await;
    (end, got)
}

#[tokio::test]
async fn search_uses_the_upstream_wire_and_cleans_up() {
    let stub = stub(Script {
        create: created(),
        responses: answers(sample_responses()),
        status: complete(true),
    })
    .await;
    let (end, got) = run(&stub, fast()).await;
    assert_eq!(end, Ok(SearchEnd::Complete));

    let seen = stub.seen();
    let post: Value = serde_json::from_str(&seen[0].body).unwrap();
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].target, "/api/v0/searches");
    assert_eq!(
        post,
        json!({ "searchText": "artist song", "responseLimit": 100 })
    );
    assert_eq!(seen[1].target, format!("/api/v0/searches/{ID}/responses"));
    assert_eq!(seen[2].target, format!("/api/v0/searches/{ID}"));
    assert_eq!(seen.last().unwrap().method, "DELETE");
    assert_eq!(
        seen.last().unwrap().target,
        format!("/api/v0/searches/{ID}")
    );
    assert!(
        seen.iter()
            .all(|s| s.api_key.as_deref() == Some("sk-synthetic"))
    );

    // First response per user; locked files dropped; metadata kept for the later download.
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].username, "peer-a");
    assert!(got[0].has_free_upload_slot);
    assert_eq!(got[0].upload_speed, 1000);
    assert_eq!(got[0].queue_length, 2);
    assert_eq!(got[0].files.len(), 1);
    let f = &got[0].files[0];
    assert_eq!(f.filename, "Music\\Artist\\Album\\01 - Song.flac");
    assert_eq!(f.size, 30_000_000);
    assert_eq!(f.length, Some(240));
    assert_eq!(f.sample_rate, Some(44100));
    assert_eq!(f.bit_depth, Some(16));
    assert_eq!(f.bitrate, 1000);
    assert_eq!(got[1].username, "peer-b");
    assert_eq!(got[1].files[0].bitrate, 0);
}

#[tokio::test]
async fn a_search_id_that_is_not_a_uuid_is_never_put_into_a_path() {
    let stub = stub(Script {
        create: answers(json!({ "id": "../../application" })),
        responses: answers(json!([])),
        status: complete(true),
    })
    .await;
    let (end, _) = run(&stub, fast()).await;
    assert_eq!(end, Err(SearchError::Unexpected));
    assert_eq!(
        stub.seen().len(),
        1,
        "only the create request: {:?}",
        stub.seen()
    );
}

#[tokio::test]
async fn the_deadline_ends_an_unfinished_search_and_deletes_it() {
    let stub = stub(Script {
        create: created(),
        responses: answers(json!([])),
        status: complete(false),
    })
    .await;
    let started = Instant::now();
    let (end, _) = run(&stub, fast()).await;
    assert_eq!(end, Ok(SearchEnd::TimedOut));
    assert!(started.elapsed() < Duration::from_millis(800 + 600));
    assert_eq!(stub.deletes().len(), 1);
    // Polls are spaced, not a busy loop: at most deadline / interval rounds of two requests.
    assert!(stub.seen().len() <= 2 + 2 * (800 / 20) + 1);
}

#[tokio::test]
async fn an_oversized_responses_answer_is_an_error_and_still_deleted() {
    let big = "x".repeat(4096);
    let stub = stub(Script {
        create: created(),
        responses: answers(json!([{ "username": "p", "files": [{ "filename": big, "size": 1 }] }])),
        status: complete(true),
    })
    .await;
    let (end, got) = run(
        &stub,
        SearchLimits {
            max_responses_body: 1024,
            ..fast()
        },
    )
    .await;
    assert_eq!(end, Err(SearchError::Unexpected));
    assert!(got.is_empty());
    assert_eq!(stub.deletes().len(), 1);
}

#[tokio::test]
async fn the_file_cap_bounds_one_search() {
    let files: Vec<Value> = (0..10)
        .map(|i| json!({ "filename": format!("f{i}.mp3"), "size": 1 }))
        .collect();
    let stub = stub(Script {
        create: created(),
        responses: answers(json!([
            { "username": "a", "files": files },
            { "username": "b", "files": files }
        ])),
        status: complete(true),
    })
    .await;
    let (end, got) = run(
        &stub,
        SearchLimits {
            max_files: 4,
            ..fast()
        },
    )
    .await;
    assert_eq!(end, Ok(SearchEnd::Capped));
    assert_eq!(stub.deletes().len(), 1);
    assert_eq!(got.iter().map(|r| r.files.len()).sum::<usize>(), 4);
}

fn file(name: impl Into<Value>) -> Value {
    json!({ "filename": name.into(), "size": 1 })
}

/// Review I-1: a peer name repeated into every result row. 16 KiB x 512 files fits the 1 MiB
/// body cap but would format to 8 MiB of usernames; the whole response is skipped instead.
#[tokio::test]
async fn an_oversized_username_skips_the_whole_response() {
    let big = "u".repeat(16 * 1024);
    let files: Vec<Value> = (0..512).map(|i| file(format!("f{i}.mp3"))).collect();
    let stub = stub(Script {
        create: created(),
        responses: answers(json!([
            { "username": big, "files": files },
            { "username": "ok", "files": [file("a.mp3")] }
        ])),
        status: complete(true),
    })
    .await;
    let (end, got) = run(&stub, fast()).await;
    assert_eq!(end, Ok(SearchEnd::Complete));
    assert_eq!(got.len(), 1, "only the valid peer");
    assert_eq!(got[0].username, "ok");
    assert_eq!(got[0].files.len(), 1);
    assert_eq!(stub.deletes().len(), 1);
}

#[tokio::test]
async fn blank_or_missing_usernames_are_skipped() {
    let stub = stub(Script {
        create: created(),
        responses: answers(json!([
            { "username": "", "files": [file("a.mp3")] },
            { "username": " \t", "files": [file("b.mp3")] },
            { "files": [file("c.mp3")] },
            { "username": "ok", "files": [file("d.mp3")] }
        ])),
        status: complete(true),
    })
    .await;
    let (end, got) = run(&stub, fast()).await;
    assert_eq!(end, Ok(SearchEnd::Complete));
    let users: Vec<&str> = got.iter().map(|r| r.username.as_str()).collect();
    assert_eq!(users, ["ok"]);
    assert_eq!(stub.deletes().len(), 1);
}

/// Byte bounds on UTF-8 text: 255-byte username and 1024-byte filename are kept whole, one byte
/// more is skipped (never truncated); bad files are dropped, valid siblings kept in order.
#[tokio::test]
async fn usernames_and_filenames_are_bounded_in_utf8_bytes() {
    let user_max = format!("{}a", "é".repeat(127)); // 255 bytes
    let user_over = "é".repeat(128); // 256 bytes
    let name_max = "ü".repeat(512); // 1024 bytes
    let name_over = format!("{}a", "ü".repeat(512)); // 1025 bytes
    assert_eq!((user_max.len(), user_over.len()), (255, 256));
    assert_eq!((name_max.len(), name_over.len()), (1024, 1025));
    let stub = stub(Script {
        create: created(),
        responses: answers(json!([
            { "username": user_over, "files": [file("x.mp3")] },
            { "username": user_max, "files": [
                file(name_over.clone()),
                file(name_max.clone()),
                file(""),
                file("  "),
                { "size": 1 },
                file("ok.mp3")
            ] }
        ])),
        status: complete(true),
    })
    .await;
    let (end, got) = run(&stub, fast()).await;
    assert_eq!(end, Ok(SearchEnd::Complete));
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].username, user_max);
    let names: Vec<&str> = got[0].files.iter().map(|f| f.filename.as_str()).collect();
    assert_eq!(names, [name_max.as_str(), "ok.mp3"]);
    assert_eq!(stub.deletes().len(), 1);
}

/// Skipped files do not use up the file cap.
#[tokio::test]
async fn invalid_files_do_not_consume_the_file_cap() {
    let stub = stub(Script {
        create: created(),
        responses: answers(json!([
            { "username": "a", "files": [file(""), file("x".repeat(1025)), file("1.mp3"), file("2.mp3")] }
        ])),
        status: complete(true),
    })
    .await;
    let (end, got) = run(
        &stub,
        SearchLimits {
            max_files: 2,
            ..fast()
        },
    )
    .await;
    assert_eq!(end, Ok(SearchEnd::Capped));
    let names: Vec<&str> = got[0].files.iter().map(|f| f.filename.as_str()).collect();
    assert_eq!(names, ["1.mp3", "2.mp3"]);
    assert_eq!(stub.deletes().len(), 1);
}

#[tokio::test]
async fn a_rejected_key_stops_the_search() {
    let stub = stub(Script {
        create: Arc::new(|| json_answer(401, json!({}))),
        responses: answers(json!([])),
        status: complete(true),
    })
    .await;
    let (end, _) = run(&stub, fast()).await;
    assert_eq!(end, Err(SearchError::Rejected));
    assert_eq!(stub.seen().len(), 1);
}

#[tokio::test]
async fn a_redirect_is_not_followed() {
    let stub = stub(Script {
        create: Arc::new(|| {
            (
                StatusCode::TEMPORARY_REDIRECT,
                [(header::LOCATION, "http://127.0.0.1:9/")],
            )
                .into_response()
        }),
        responses: answers(json!([])),
        status: complete(true),
    })
    .await;
    let (end, _) = run(&stub, fast()).await;
    assert_eq!(end, Err(SearchError::Unexpected));
    assert_eq!(stub.seen().len(), 1);
}

#[tokio::test]
async fn the_network_guard_refuses_a_real_host_before_sending() {
    let url = SlskdUrl::parse("http://slskd.example.invalid:5030").unwrap();
    let end = search(&url, None, "q", &fast(), true, |_| {}).await;
    assert_eq!(end, Err(SearchError::Refused));
}

#[tokio::test]
async fn a_cancelled_search_is_still_deleted() {
    let stub = stub(Script {
        create: created(),
        responses: answers(json!([])),
        status: complete(false),
    })
    .await;
    let s2 = stub.clone();
    let task = tokio::spawn(async move {
        let limits = SearchLimits {
            deadline: Duration::from_secs(30),
            ..fast()
        };
        search(&s2.url(), None, "q", &limits, true, |_| {}).await
    });
    // Wait until polling has started, then cancel.
    for _ in 0..100 {
        if stub.seen().len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    task.abort();
    let _ = task.await;
    for _ in 0..100 {
        if !stub.deletes().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(stub.deletes().len(), 1);
    assert_eq!(stub.deletes()[0].target, format!("/api/v0/searches/{ID}"));
}
