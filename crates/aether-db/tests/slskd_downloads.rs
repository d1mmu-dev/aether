//! Migration 0067 and the slskd download state (M7 Task 4B, D-294): ownership, idempotency,
//! attempt fences, the state graph, cancellation versus publication, profile deletion and the
//! slskd connection generation. Database only; no network, job or filesystem.

use std::time::Duration;

use aether_core::cipher::Cipher;
use aether_core::profile::ProfileId;
use aether_db::PgPool;
use aether_db::settings_kv::{self, Key, KeyChange};
use aether_db::slskd_downloads::{
    self as dl, CancelOutcome, CreateError, CreateOutcome, DestRel, FailureReason, Invalid,
    NewDownload, Receipt, ReconcileReason, Resolution, State,
};
use serde_json::{Value, json};
use sqlx::migrate::Migrator;
use sqlx::postgres::PgDatabaseError;
use uuid::Uuid;

const ORIGIN: &str = "http://slskd.lan:5030";
const SIZE: u64 = 1000;

async fn account(pool: &PgPool, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, username, password_hash) VALUES ($1, $2, 'x')")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    id
}

async fn add_profile(pool: &PgPool, account: Uuid, name: &str) -> ProfileId {
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO profiles (id, user_id, name, kind) VALUES ($1, $2, $3, 'standard')")
        .bind(id)
        .bind(account)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    ProfileId::new(id)
}

fn request(profile: ProfileId, file: &str, dest: &str) -> NewDownload {
    NewDownload::new(
        profile,
        ORIGIN,
        0,
        "peer",
        file,
        SIZE,
        dl::DEFAULT_MAX_BYTES,
        DestRel::parse(dest).unwrap(),
    )
    .unwrap()
}

async fn created(pool: &PgPool, new: &NewDownload) -> Uuid {
    match dl::create(pool, new).await.unwrap() {
        CreateOutcome::Created(id) => id,
        other => panic!("not created: {other:?}"),
    }
}

async fn state(pool: &PgPool, id: Uuid) -> Option<State> {
    dl::load(pool, id).await.unwrap().map(|d| d.state)
}

fn receipt() -> Receipt {
    Receipt {
        bytes: SIZE,
        sha256: [7; 32],
    }
}

fn pg(e: &sqlx::Error) -> &PgDatabaseError {
    e.as_database_error()
        .unwrap_or_else(|| panic!("not a database error: {e:?}"))
        .downcast_ref::<PgDatabaseError>()
}

/// Drives a fresh request to `remote_succeeded`; returns the live fence.
async fn to_remote_succeeded(pool: &PgPool, id: Uuid) -> dl::Fence {
    let fence = dl::claim(pool, id).await.unwrap().unwrap().fence;
    assert!(dl::mark_enqueue_sent(pool, fence).await.unwrap());
    assert!(
        dl::mark_remote_active(pool, fence, Uuid::new_v4())
            .await
            .unwrap()
    );
    assert!(dl::mark_remote_succeeded(pool, fence).await.unwrap());
    fence
}

async fn fixture(pool: &PgPool) -> (Uuid, ProfileId, ProfileId) {
    let acct = account(pool, "alice").await;
    let a = add_profile(pool, acct, "A").await;
    let b = add_profile(pool, acct, "B").await;
    (acct, a, b)
}

// --- migration ---------------------------------------------------------------------------

fn migrator_upto(version: i64) -> Migrator {
    Migrator {
        migrations: std::borrow::Cow::Owned(
            aether_db::MIGRATOR
                .iter()
                .filter(|m| m.version <= version)
                .cloned()
                .collect(),
        ),
        ..Migrator::DEFAULT
    }
}

async fn rows(pool: &PgPool, table: &str) -> Value {
    sqlx::query_scalar(&format!(
        "SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text), '[]') FROM {table} t"
    ))
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn fresh_schema_has_owner_set_null_and_unchanged_checksums(pool: PgPool) {
    let applied: Vec<(i64, Vec<u8>)> =
        sqlx::query_as("SELECT version, checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .unwrap();
    let embedded: Vec<_> = aether_db::MIGRATOR
        .iter()
        .map(|m| (m.version, m.checksum.to_vec()))
        .collect();
    assert_eq!(applied, embedded);
    assert_eq!(applied.last().unwrap().0, 67);
    let mut fks: Vec<(String, String)> = sqlx::query_as(
        "SELECT confrelid::regclass::text, confdeltype::text FROM pg_constraint
          WHERE contype = 'f' AND conrelid = 'slskd_downloads'::regclass",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    fks.sort();
    // SET NULL (`n`) on both: profile deletion leaves tombstones, job cleanup unbinds.
    assert_eq!(
        fks,
        vec![("jobs".into(), "n".into()), ("profiles".into(), "n".into())]
    );
}

#[sqlx::test(migrations = false)]
async fn upgrade_from_0066_keeps_rows_and_serves_existing_profiles(pool: PgPool) {
    migrator_upto(66).run(&pool).await.unwrap();
    let (_, a, _) = fixture(&pool).await;
    sqlx::query(
        "INSERT INTO jobs (id, queue, payload) VALUES ($1, 'default', '{\"kind\":\"scan\"}')",
    )
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO settings_kv (key, value) VALUES ('slskdUrl', $1)")
        .bind(json!(ORIGIN))
        .execute(&pool)
        .await
        .unwrap();
    let before: Vec<Value> = futures::future::join_all(
        ["users", "profiles", "jobs", "settings_kv"].map(|t| rows(&pool, t)),
    )
    .await;
    assert!(before.iter().all(|r| r != &json!([])));

    aether_db::MIGRATOR.run(&pool).await.unwrap();

    let after: Vec<Value> = futures::future::join_all(
        ["users", "profiles", "jobs", "settings_kv"].map(|t| rows(&pool, t)),
    )
    .await;
    assert_eq!(after, before, "0067 changes no existing row");
    let id = created(&pool, &request(a, "Music/A/B/01.flac", "A/B/01.flac")).await;
    assert_eq!(state(&pool, id).await, Some(State::Requested));
    let applied: Vec<(i64, Vec<u8>)> =
        sqlx::query_as("SELECT version, checksum FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .unwrap();
    let embedded: Vec<_> = aether_db::MIGRATOR
        .iter()
        .map(|m| (m.version, m.checksum.to_vec()))
        .collect();
    assert_eq!(applied, embedded);
}

// --- validation and idempotency ----------------------------------------------------------

#[test]
fn request_fields_are_bounded_and_never_repaired() {
    let p = ProfileId::new(Uuid::now_v7());
    let dest = || DestRel::parse("A/B/c.flac").unwrap();
    let new = |user: &str, file: &str, size: u64, max: u64| {
        NewDownload::new(p, ORIGIN, 0, user, file, size, max, dest())
    };
    let user255 = "é".repeat(127) + "x"; // 255 bytes
    let file1024 = "ü".repeat(512); // 1024 bytes
    assert!(new(&user255, &file1024, 1, 1).is_ok());
    assert_eq!(
        new(&(user255.clone() + "x"), "f", 1, 1),
        Err(Invalid::RemoteUsername)
    );
    assert_eq!(new(" \t", "f", 1, 1), Err(Invalid::RemoteUsername));
    assert_eq!(
        new("u", &(file1024 + "x"), 1, 1),
        Err(Invalid::RemoteFilename)
    );
    assert_eq!(new("u", "  ", 1, 1), Err(Invalid::RemoteFilename));
    assert_eq!(new("u", "f", 0, 1), Err(Invalid::Size));
    assert_eq!(new("u", "f", u64::MAX, u64::MAX), Err(Invalid::Size));
    assert_eq!(new("u", "f", 10, 9), Err(Invalid::Size));
    for origin in [
        "slskd:5030",
        "http://h/path",
        "http://u@h",
        "ftp://h",
        "http://",
    ] {
        assert_eq!(
            NewDownload::new(p, origin, 0, "u", "f", 1, 1, dest()),
            Err(Invalid::Origin),
            "{origin}"
        );
    }
    assert_eq!(
        NewDownload::new(p, ORIGIN, -1, "u", "f", 1, 1, dest()),
        Err(Invalid::Origin)
    );
    for bad in [
        "A/c.flac",
        "A/B/C/d.flac",
        "/A/B/c",
        "A/B/",
        "A//c",
        "A/../c",
        "A/./c",
        "A/B\\x/c",
        "A/B/c\u{7}",
        "A/.aether-ingest-1/c",
        "C:/B/c",
        &format!("A/B/{}", "x".repeat(256)),
    ] {
        assert_eq!(DestRel::parse(bad), Err(Invalid::Destination), "{bad:?}");
    }
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn database_refuses_unsafe_rows_written_around_the_helpers(pool: PgPool) {
    let (_, a, _) = fixture(&pool).await;
    let insert = |dest: &'static str, user: String, size: i64| {
        let pool = pool.clone();
        async move {
            sqlx::query(
                "INSERT INTO slskd_downloads (id, profile_id, batch_id, slskd_origin,
                     slskd_generation, remote_username, remote_filename, expected_size,
                     max_bytes, dest_rel)
                 VALUES (gen_random_uuid(), $1, gen_random_uuid(), $2, 0, $3, 'f', $4, $4, $5)",
            )
            .bind(a.into_uuid())
            .bind(ORIGIN)
            .bind(user)
            .bind(size)
            .bind(dest)
            .execute(&pool)
            .await
        }
    };
    for dest in [
        "../B/c", "/A/B", "A/B", "A/B/c/d", "A/B\\/c", "A/./c", "C:/B/c",
    ] {
        let err = insert(dest, "u".into(), 1).await.unwrap_err();
        assert_eq!(pg(&err).code(), "23514", "{dest}");
    }
    let err = insert("A/B/c", "x".repeat(256), 1).await.unwrap_err();
    assert_eq!(pg(&err).code(), "23514");
    let err = insert("A/B/c", "u".into(), 0).await.unwrap_err();
    assert_eq!(pg(&err).code(), "23514");
    insert("A/B/c", "u".into(), 1).await.unwrap();
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn duplicate_returns_the_same_id_and_other_owners_get_a_bare_conflict(pool: PgPool) {
    let (_, a, b) = fixture(&pool).await;
    let new = request(a, "Music/A/B/01.flac", "A/B/01.flac");
    let id = created(&pool, &new).await;
    assert_eq!(
        dl::create(&pool, &new).await.unwrap(),
        CreateOutcome::Existing(id)
    );
    // Same remote file, other destination; same destination, other file: both conflicts.
    for other in [
        request(a, "Music/A/B/01.flac", "A/B/other.flac"),
        request(a, "Music/A/B/02.flac", "A/B/01.flac"),
        request(b, "Music/A/B/01.flac", "A/B/01.flac"),
        request(b, "Music/X/02.flac", "A/B/01.flac"),
    ] {
        assert_eq!(
            dl::create(&pool, &other).await.unwrap(),
            CreateOutcome::Conflict
        );
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM slskd_downloads")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);
    // A finished request frees both keys.
    assert_eq!(
        dl::request_cancel(&pool, a, id).await.unwrap(),
        CancelOutcome::Cancelled
    );
    let again = created(&pool, &new).await;
    assert_ne!(again, id);
    assert!(
        dl::get_owned(&pool, b, again).await.unwrap().is_none(),
        "not visible to another profile"
    );
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn concurrent_duplicates_store_one_row(pool: PgPool) {
    let (_, a, _) = fixture(&pool).await;
    let new = request(a, "Music/A/B/01.flac", "A/B/01.flac");
    let outcomes = futures::future::join_all((0..8).map(|_| dl::create(&pool, &new))).await;
    let outcomes: Vec<_> = outcomes.into_iter().map(Result::unwrap).collect();
    let ids: Vec<Uuid> = outcomes
        .iter()
        .filter_map(|o| match o {
            CreateOutcome::Created(id) => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 1, "{outcomes:?}");
    assert!(outcomes.iter().all(|o| matches!(o,
        CreateOutcome::Created(id) | CreateOutcome::Existing(id) if *id == ids[0])));
}

// --- fences and the state graph ----------------------------------------------------------

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn publication_path_records_the_receipt(pool: PgPool) {
    let (_, a, _) = fixture(&pool).await;
    let id = created(&pool, &request(a, "Music/A/B/01.flac", "A/B/01.flac")).await;
    let fence = to_remote_succeeded(&pool, id).await;
    assert!(dl::begin_publish(&pool, fence).await.unwrap());
    // The receipt must match the expected size (database CHECK), else nothing changes.
    let short = Receipt {
        bytes: SIZE - 1,
        ..receipt()
    };
    let err = dl::finish_publish(&pool, fence, short, true)
        .await
        .unwrap_err();
    assert_eq!(pg(&err).code(), "23514");
    assert!(
        dl::finish_publish(&pool, fence, receipt(), false)
            .await
            .unwrap()
    );
    assert_eq!(state(&pool, id).await, Some(State::PublishedNotDurable));
    let fence = dl::claim(&pool, id).await.unwrap().unwrap().fence;
    assert!(dl::mark_durable(&pool, fence).await.unwrap());
    let row = dl::get_owned(&pool, a, id).await.unwrap().unwrap();
    assert_eq!(row.state, State::Published);
    assert_eq!(row.receipt, Some(receipt()));
    assert!(row.remote_transfer_id.is_some());
    assert!(dl::claim(&pool, id).await.unwrap().is_none(), "finished");
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn stale_fence_changes_nothing(pool: PgPool) {
    let (_, a, _) = fixture(&pool).await;
    let id = created(&pool, &request(a, "Music/A/B/01.flac", "A/B/01.flac")).await;
    let old = dl::claim(&pool, id).await.unwrap().unwrap().fence;
    let new = dl::claim(&pool, id).await.unwrap().unwrap().fence;
    assert!(new.attempt() > old.attempt());
    assert!(!dl::mark_enqueue_sent(&pool, old).await.unwrap());
    assert!(
        !dl::mark_failed(&pool, old, FailureReason::RemoteFailed)
            .await
            .unwrap()
    );
    assert_eq!(state(&pool, id).await, Some(State::Requested));
    assert!(dl::mark_enqueue_sent(&pool, new).await.unwrap());
    assert!(
        !dl::mark_remote_active(&pool, old, Uuid::new_v4())
            .await
            .unwrap()
    );
    assert!(
        dl::mark_remote_active(&pool, new, Uuid::new_v4())
            .await
            .unwrap()
    );
    // A step from the wrong state also loses, with the right fence.
    assert!(!dl::begin_publish(&pool, new).await.unwrap());
    assert_eq!(state(&pool, id).await, Some(State::RemoteActive));
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn interrupted_steps_need_reconciliation_and_are_never_resent(pool: PgPool) {
    let (_, a, _) = fixture(&pool).await;
    let sent = created(&pool, &request(a, "Music/A/B/01.flac", "A/B/01.flac")).await;
    let fence = dl::claim(&pool, sent).await.unwrap().unwrap().fence;
    assert!(dl::mark_enqueue_sent(&pool, fence).await.unwrap());
    // The worker dies before slskd's answer is recorded; the next attempt recovers.
    let next = dl::claim(&pool, sent).await.unwrap().unwrap();
    assert_eq!(next.state, State::NeedsReconcile);
    assert!(
        !dl::mark_remote_active(&pool, fence, Uuid::new_v4())
            .await
            .unwrap()
    );
    assert!(!dl::mark_enqueue_sent(&pool, next.fence).await.unwrap());
    let row = dl::load(&pool, sent).await.unwrap().unwrap();
    assert_eq!(row.reconcile_reason.as_deref(), Some("enqueue_unknown"));
    assert!(
        dl::claim(&pool, sent).await.unwrap().is_none(),
        "manual only"
    );

    let publishing = created(&pool, &request(a, "Music/A/B/02.flac", "A/B/02.flac")).await;
    let fence = to_remote_succeeded(&pool, publishing).await;
    assert!(dl::begin_publish(&pool, fence).await.unwrap());
    let next = dl::claim(&pool, publishing).await.unwrap().unwrap();
    assert_eq!(next.state, State::NeedsReconcile);
    assert!(
        !dl::finish_publish(&pool, fence, receipt(), true)
            .await
            .unwrap()
    );
    let row = dl::load(&pool, publishing).await.unwrap().unwrap();
    assert_eq!(row.reconcile_reason.as_deref(), Some("publish_interrupted"));

    // Manual resolution finishes the row and fences every older attempt.
    assert!(
        dl::resolve(&pool, publishing, Resolution::Published(receipt()))
            .await
            .unwrap()
    );
    assert!(
        !dl::resolve(&pool, publishing, Resolution::Failed)
            .await
            .unwrap()
    );
    assert_eq!(state(&pool, publishing).await, Some(State::Published));
    assert!(dl::resolve(&pool, sent, Resolution::Failed).await.unwrap());
    let row = dl::load(&pool, sent).await.unwrap().unwrap();
    assert_eq!(
        (row.state, row.failure_reason.as_deref()),
        (State::Failed, Some("manual"))
    );

    // A worker can also hand a row over explicitly.
    let active = created(&pool, &request(a, "Music/A/B/03.flac", "A/B/03.flac")).await;
    let fence = to_remote_succeeded(&pool, active).await;
    assert!(
        dl::mark_needs_reconcile(&pool, fence, ReconcileReason::RemoteUnknown)
            .await
            .unwrap()
    );
    assert_eq!(state(&pool, active).await, Some(State::NeedsReconcile));
}

/// Inserts a row directly in `state` with the columns that state requires.
async fn row_in(pool: &PgPool, profile: ProfileId, state: State, n: usize) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO slskd_downloads (id, profile_id, batch_id, remote_transfer_id, slskd_origin,
             slskd_generation, remote_username, remote_filename, expected_size, max_bytes,
             dest_rel, state, failure_reason, reconcile_reason, receipt_bytes, receipt_sha256,
             finished_at)
         VALUES ($1, $2, gen_random_uuid(), gen_random_uuid(), $3, 0, 'peer', $4, 1000, 1000,
                 $5, $6::text,
                 CASE WHEN $6::text = 'failed' THEN 'manual' END,
                 CASE WHEN $6::text = 'needs_reconcile' THEN 'remote_unknown' END,
                 CASE WHEN $6::text IN ('published', 'published_not_durable') THEN 1000 END,
                 CASE WHEN $6::text IN ('published', 'published_not_durable') THEN $7::bytea END,
                 CASE WHEN $6::text IN ('published', 'failed', 'cancelled') THEN now() END)",
    )
    .bind(id)
    .bind(profile.into_uuid())
    .bind(ORIGIN)
    .bind(format!("f{n}"))
    .bind(format!("A/B/f{n}"))
    .bind(state.as_str())
    .bind(vec![7u8; 32])
    .execute(pool)
    .await
    .unwrap();
    id
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn database_graph_equals_the_rust_graph(pool: PgPool) {
    let (_, a, _) = fixture(&pool).await;
    let mut n = 0;
    for from in State::ALL {
        for to in State::ALL.into_iter().filter(|t| *t != from) {
            n += 1;
            let id = row_in(&pool, a, from, n).await;
            let moved = sqlx::query(
                "UPDATE slskd_downloads SET state = $2::text,
                     failure_reason = CASE WHEN $2::text = 'failed' THEN 'manual' END,
                     reconcile_reason = CASE WHEN $2::text = 'needs_reconcile'
                                             THEN 'remote_unknown' END,
                     receipt_bytes = CASE WHEN $2::text IN ('published', 'published_not_durable')
                                          THEN 1000 END,
                     receipt_sha256 = CASE WHEN $2::text IN ('published', 'published_not_durable')
                                           THEN $3::bytea END,
                     finished_at = CASE WHEN $2::text IN ('published', 'failed', 'cancelled')
                                        THEN now() END
                 WHERE id = $1",
            )
            .bind(id)
            .bind(to.as_str())
            .bind(vec![7u8; 32])
            .execute(&pool)
            .await;
            match moved {
                Ok(_) => assert!(from.can_move_to(to), "{from:?} -> {to:?} allowed by DB"),
                Err(e) => {
                    assert!(!from.can_move_to(to), "{from:?} -> {to:?}: {e:?}");
                    assert_eq!(pg(&e).hint(), Some("aether:download_transition"));
                }
            }
        }
    }
    for t in State::ALL.into_iter().filter(|s| s.is_terminal()) {
        assert!(State::ALL.iter().all(|n| !t.can_move_to(*n)));
    }
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn request_fields_fence_and_cancel_flag_cannot_be_rewritten(pool: PgPool) {
    let (_, a, b) = fixture(&pool).await;
    let id = created(&pool, &request(a, "Music/A/B/01.flac", "A/B/01.flac")).await;
    dl::claim(&pool, id).await.unwrap().unwrap();
    let mut conn = pool.acquire().await.unwrap();
    for (sql, bind) in [
        (
            "UPDATE slskd_downloads SET dest_rel = 'X/Y/z' WHERE id = $1",
            None,
        ),
        (
            "UPDATE slskd_downloads SET remote_filename = 'other' WHERE id = $1",
            None,
        ),
        (
            "UPDATE slskd_downloads SET slskd_generation = 9 WHERE id = $1",
            None,
        ),
        ("UPDATE slskd_downloads SET attempt = 0 WHERE id = $1", None),
        (
            "UPDATE slskd_downloads SET profile_id = $2 WHERE id = $1",
            Some(b.into_uuid()),
        ),
    ] {
        let mut q = sqlx::query(sql).bind(id);
        if let Some(v) = bind {
            q = q.bind(v);
        }
        let err = q.execute(&mut *conn).await.unwrap_err();
        assert_eq!(pg(&err).hint(), Some("aether:download_immutable"), "{sql}");
    }
    sqlx::query("UPDATE slskd_downloads SET cancel_requested = true WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await
        .unwrap();
    let err = sqlx::query("UPDATE slskd_downloads SET cancel_requested = false WHERE id = $1")
        .bind(id)
        .execute(&mut *conn)
        .await
        .unwrap_err();
    assert_eq!(pg(&err).hint(), Some("aether:download_immutable"));
}

// --- cancellation versus publication -----------------------------------------------------

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn owner_cancel_before_publication_wins_and_after_it_does_not(pool: PgPool) {
    let (_, a, b) = fixture(&pool).await;
    // Nothing sent yet: cancelled at once, and the worker can no longer send.
    let fresh = created(&pool, &request(a, "Music/A/B/01.flac", "A/B/01.flac")).await;
    let fence = dl::claim(&pool, fresh).await.unwrap().unwrap().fence;
    assert_eq!(
        dl::request_cancel(&pool, b, fresh).await.unwrap(),
        CancelOutcome::NotFound
    );
    assert_eq!(
        state(&pool, fresh).await,
        Some(State::Requested),
        "foreign cancel is a no-op"
    );
    assert_eq!(
        dl::request_cancel(&pool, a, fresh).await.unwrap(),
        CancelOutcome::Cancelled
    );
    assert!(!dl::mark_enqueue_sent(&pool, fence).await.unwrap());

    // Downloaded but not yet publishing: the cancel blocks publication.
    let done = created(&pool, &request(a, "Music/A/B/02.flac", "A/B/02.flac")).await;
    let fence = to_remote_succeeded(&pool, done).await;
    assert_eq!(
        dl::request_cancel(&pool, a, done).await.unwrap(),
        CancelOutcome::Requested
    );
    assert!(!dl::begin_publish(&pool, fence).await.unwrap());
    assert!(dl::mark_cancelled(&pool, fence).await.unwrap());
    assert_eq!(state(&pool, done).await, Some(State::Cancelled));

    // Publication already begun: the cancel is only a request; a completed publication is
    // recorded as published, never failed or rolled back.
    let late = created(&pool, &request(a, "Music/A/B/03.flac", "A/B/03.flac")).await;
    let fence = to_remote_succeeded(&pool, late).await;
    assert!(dl::begin_publish(&pool, fence).await.unwrap());
    assert_eq!(
        dl::request_cancel(&pool, a, late).await.unwrap(),
        CancelOutcome::Requested
    );
    assert!(
        dl::finish_publish(&pool, fence, receipt(), true)
            .await
            .unwrap()
    );
    let row = dl::get_owned(&pool, a, late).await.unwrap().unwrap();
    assert_eq!((row.state, row.cancel_requested), (State::Published, true));
    assert_eq!(
        dl::request_cancel(&pool, a, late).await.unwrap(),
        CancelOutcome::AlreadyFinished
    );
    // A worker cannot "cancel" without an asked cancellation.
    let quiet = created(&pool, &request(a, "Music/A/B/04.flac", "A/B/04.flac")).await;
    let fence = to_remote_succeeded(&pool, quiet).await;
    assert!(!dl::mark_cancelled(&pool, fence).await.unwrap());
}

// --- profile deletion --------------------------------------------------------------------

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn profile_deletion_cancels_unfinished_and_keeps_no_history(pool: PgPool) {
    let (_, a, b) = fixture(&pool).await;
    let requested = created(&pool, &request(a, "f/1", "A/B/1")).await;
    let failed = created(&pool, &request(a, "f/2", "A/B/2")).await;
    let fence = dl::claim(&pool, failed).await.unwrap().unwrap().fence;
    assert!(
        dl::mark_failed(&pool, fence, FailureReason::RemoteRejected)
            .await
            .unwrap()
    );
    let published = created(&pool, &request(a, "f/3", "A/B/3")).await;
    let fence = to_remote_succeeded(&pool, published).await;
    assert!(dl::begin_publish(&pool, fence).await.unwrap());
    assert!(
        dl::finish_publish(&pool, fence, receipt(), true)
            .await
            .unwrap()
    );
    let active = created(&pool, &request(a, "f/4", "A/B/4")).await;
    let active_fence = dl::claim(&pool, active).await.unwrap().unwrap().fence;
    assert!(dl::mark_enqueue_sent(&pool, active_fence).await.unwrap());
    assert!(
        dl::mark_remote_active(&pool, active_fence, Uuid::new_v4())
            .await
            .unwrap()
    );
    let downloaded = created(&pool, &request(a, "f/5", "A/B/5")).await;
    let downloaded_fence = to_remote_succeeded(&pool, downloaded).await;
    let publishing = created(&pool, &request(a, "f/6", "A/B/6")).await;
    let publishing_fence = to_remote_succeeded(&pool, publishing).await;
    assert!(dl::begin_publish(&pool, publishing_fence).await.unwrap());
    let others = created(&pool, &request(b, "f/7", "A/B/7")).await;

    sqlx::query("DELETE FROM profiles WHERE id = $1")
        .bind(a.into_uuid())
        .execute(&pool)
        .await
        .unwrap();

    // Nothing pending remotely: gone with the profile.
    for id in [requested, failed, published] {
        assert_eq!(state(&pool, id).await, None);
    }
    // Pending: ownerless tombstones marked for cancellation, state untouched.
    for (id, st) in [
        (active, State::RemoteActive),
        (downloaded, State::RemoteSucceeded),
        (publishing, State::Publishing),
    ] {
        let row = dl::load(&pool, id).await.unwrap().unwrap();
        assert_eq!(
            (row.profile, row.state, row.cancel_requested),
            (None, st, true)
        );
    }
    assert_eq!(
        state(&pool, others).await,
        Some(State::Requested),
        "other profile untouched"
    );
    // A request for the deleted profile is refused.
    assert!(matches!(
        dl::create(&pool, &request(a, "f/8", "A/B/8")).await,
        Err(CreateError::ProfileGone)
    ));
    // Ownerless rows cannot publish, but settle normally and then disappear.
    assert!(!dl::begin_publish(&pool, downloaded_fence).await.unwrap());
    assert!(dl::mark_cancelled(&pool, downloaded_fence).await.unwrap());
    assert!(dl::mark_cancelled(&pool, active_fence).await.unwrap());
    assert!(
        dl::finish_publish(&pool, publishing_fence, receipt(), true)
            .await
            .unwrap()
    );
    for id in [active, downloaded, publishing] {
        assert_eq!(state(&pool, id).await, None, "settled tombstone kept");
    }
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn profile_deletion_and_begin_publish_serialize_on_the_row(pool: PgPool) {
    let (acct, a, _) = fixture(&pool).await;
    let id = created(&pool, &request(a, "f/1", "A/B/1")).await;
    let fence = to_remote_succeeded(&pool, id).await;
    // The deletion's trigger has locked the row; begin_publish waits and then loses.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("DELETE FROM profiles WHERE id = $1")
        .bind(a.into_uuid())
        .execute(&mut *tx)
        .await
        .unwrap();
    let racer = tokio::spawn({
        let pool = pool.clone();
        async move { dl::begin_publish(&pool, fence).await.unwrap() }
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !racer.is_finished(),
        "begin_publish must wait for the deletion"
    );
    tx.commit().await.unwrap();
    assert!(
        !racer.await.unwrap(),
        "deletion committed first: no publication"
    );
    let row = dl::load(&pool, id).await.unwrap().unwrap();
    assert_eq!(
        (row.state, row.profile, row.cancel_requested),
        (State::RemoteSucceeded, None, true)
    );

    // The other order: publication begun first keeps going after the account is deleted
    // (cascade through every profile), and its result is recorded, not failed.
    let b = add_profile(&pool, acct, "B2").await;
    let id = created(&pool, &request(b, "f/2", "A/B/2")).await;
    let fence = to_remote_succeeded(&pool, id).await;
    assert!(dl::begin_publish(&pool, fence).await.unwrap());
    sqlx::query("DELETE FROM users WHERE id = $1")
        .bind(acct)
        .execute(&pool)
        .await
        .unwrap();
    let row = dl::load(&pool, id).await.unwrap().unwrap();
    assert_eq!((row.state, row.profile), (State::Publishing, None));
    assert!(
        dl::finish_publish(&pool, fence, receipt(), false)
            .await
            .unwrap()
    );
    // Published but not durable is not settled: the tombstone stays for the re-sync.
    assert_eq!(state(&pool, id).await, Some(State::PublishedNotDurable));
}

// --- slskd connection generation ---------------------------------------------------------

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn connection_generation_changes_only_with_url_or_key(pool: PgPool) {
    let cipher = Cipher::random();
    let (p, c) = (&pool, &cipher);
    let generation = || async move {
        settings_kv::slskd_connection(p, c)
            .await
            .unwrap()
            .generation
    };
    assert_eq!(generation().await, 0);
    let save = |url: Option<Option<&'static str>>, key: KeyChange| async move {
        settings_kv::save_slskd(p, c, url, key).await.unwrap()
    };
    save(Some(Some(ORIGIN)), KeyChange::Keep).await;
    assert_eq!(generation().await, 1, "URL set");
    save(Some(Some(ORIGIN)), KeyChange::Keep).await;
    assert_eq!(generation().await, 1, "same URL");
    save(None, KeyChange::Set("key-one".into())).await;
    assert_eq!(generation().await, 2, "key set");
    save(None, KeyChange::Set("key-one".into())).await;
    assert_eq!(generation().await, 2, "same key");
    save(None, KeyChange::Keep).await;
    assert_eq!(generation().await, 2, "keep");
    settings_kv::set_plain(&pool, Key::SoulseekEnabled, Some(&json!(true)))
        .await
        .unwrap();
    let conn = settings_kv::slskd_connection(&pool, &cipher).await.unwrap();
    assert_eq!(
        (
            conn.enabled,
            conn.url.as_deref(),
            conn.api_key.as_deref(),
            conn.generation
        ),
        (true, Some(ORIGIN), Some("key-one"), 2),
        "enabled switch is read fresh, not versioned"
    );
    save(None, KeyChange::Clear).await;
    assert_eq!(generation().await, 3, "key cleared");
    save(None, KeyChange::Clear).await;
    assert_eq!(generation().await, 3, "nothing to clear");
    save(None, KeyChange::Set("key-two".into())).await;
    save(Some(Some("http://other:5030")), KeyChange::Keep).await;
    assert_eq!(
        generation().await,
        5,
        "key set, then URL change dropping the key"
    );
    let conn = settings_kv::slskd_connection(&pool, &cipher).await.unwrap();
    assert_eq!(conn.api_key, None);
    let debug = format!("{conn:?}");
    assert!(!debug.contains("key-"), "{debug}");
    // Stored as a plain number with no key material, outside the settings API's keys.
    let (value, secret): (Option<Value>, Option<Vec<u8>>) =
        sqlx::query_as("SELECT value, secret_enc FROM settings_kv WHERE key = 'slskdGeneration'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!((value, secret), (Some(json!(5)), None));
    assert!(Key::ALL.iter().all(|k| k.name() != "slskdGeneration"));
}

#[sqlx::test(migrator = "aether_db::MIGRATOR")]
async fn connection_generation_rolls_back_with_a_failed_save(pool: PgPool) {
    let cipher = Cipher::random();
    settings_kv::save_slskd(&pool, &cipher, Some(Some(ORIGIN)), KeyChange::Keep)
        .await
        .unwrap();
    // Break the key write after the URL change: the whole pair save must roll back.
    sqlx::query("ALTER TABLE settings_kv ADD CONSTRAINT no_key CHECK (key <> 'slskdApiKey')")
        .execute(&pool)
        .await
        .unwrap();
    let err = settings_kv::save_slskd(
        &pool,
        &cipher,
        Some(Some("http://other:5030")),
        KeyChange::Set("k".into()),
    )
    .await;
    assert!(err.is_err());
    let conn = settings_kv::slskd_connection(&pool, &cipher).await.unwrap();
    assert_eq!((conn.url.as_deref(), conn.generation), (Some(ORIGIN), 1));
}
