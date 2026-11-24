//! Durable slskd download requests (M7 Task 4B, D-294, migration 0067). Database state only:
//! no route, job kind or network call uses this module yet; the Task 4C worker will.
//!
//! A request is owned by a profile and moves along a fixed graph (enforced again by the
//! `slskd_downloads_guard` trigger):
//!
//! ```text
//! requested ─▶ enqueue_sent ─▶ remote_active ─▶ remote_succeeded ─▶ publishing ─▶ published
//!     │              │               │                 │               ├─▶ published_not_durable ─▶ published
//!     ▼              ▼               ▼                 ▼               ▼
//!  cancelled/failed  failed / needs_reconcile   failed / cancelled / needs_reconcile
//! needs_reconcile ─▶ published / failed / cancelled   (manual resolution only)
//! ```
//!
//! Every worker step is one `UPDATE` comparing id, [`Fence`] (the row's `attempt`) and the
//! expected source state; `false` means the step lost (stale fence, another state, cancelled or
//! ownerless) and changed nothing. [`claim`] starts a new attempt and turns any step that an
//! earlier attempt left in flight (`enqueue_sent`, `publishing`) into `needs_reconcile`: the
//! remote create or the publication may or may not have happened, and nothing here retries it
//! (owner decision 4).
//!
//! Cancellation and publication: [`begin_publish`] succeeds only while the row still has an
//! owner and no cancel was asked; once a row is `publishing`, cancellation is only a request
//! the worker may honour before the publishing syscall, and the row records what the
//! publication returned ([`finish_publish`] even when a cancel was asked meanwhile). Profile
//! deletion is handled in the database (migration 0067 header). No remote transfer is
//! actually cancelled until the Task 4C worker exists.

use aether_core::profile::ProfileId;
use sqlx::{PgPool, Row};
use uuid::Uuid;

/// Longest peer user name, in UTF-8 bytes (the D-292 search bound).
pub const MAX_REMOTE_USERNAME_BYTES: usize = 255;
/// Longest peer file path, in UTF-8 bytes (the D-292 search bound).
pub const MAX_REMOTE_FILENAME_BYTES: usize = 1024;
/// Longest destination, in bytes (three components of at most 255 bytes fit).
pub const MAX_DEST_REL_BYTES: usize = 1024;
/// Longest stored slskd origin, in bytes (the D-291 URL bound).
pub const MAX_ORIGIN_BYTES: usize = 512;
/// A default per-file byte cap for callers without a configured one: 4 GiB. A storage
/// policy value, not a network or slskd limit; callers may pass any cap ≥ the expected size.
pub const DEFAULT_MAX_BYTES: u64 = 4 << 30;

/// Every state a request can be in (`slskd_downloads.state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum State {
    Requested,
    EnqueueSent,
    RemoteActive,
    RemoteSucceeded,
    Publishing,
    Published,
    PublishedNotDurable,
    Failed,
    Cancelled,
    NeedsReconcile,
}

impl State {
    pub const ALL: [State; 10] = [
        State::Requested,
        State::EnqueueSent,
        State::RemoteActive,
        State::RemoteSucceeded,
        State::Publishing,
        State::Published,
        State::PublishedNotDurable,
        State::Failed,
        State::Cancelled,
        State::NeedsReconcile,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            State::Requested => "requested",
            State::EnqueueSent => "enqueue_sent",
            State::RemoteActive => "remote_active",
            State::RemoteSucceeded => "remote_succeeded",
            State::Publishing => "publishing",
            State::Published => "published",
            State::PublishedNotDurable => "published_not_durable",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
            State::NeedsReconcile => "needs_reconcile",
        }
    }

    pub fn parse(s: &str) -> Option<State> {
        State::ALL.into_iter().find(|st| st.as_str() == s)
    }

    /// Finished for good: nothing is pending locally or remotely.
    pub const fn is_terminal(self) -> bool {
        matches!(self, State::Published | State::Failed | State::Cancelled)
    }

    /// The graph the database trigger enforces (kept equal by a test).
    pub const fn can_move_to(self, next: State) -> bool {
        use State::*;
        matches!(
            (self, next),
            (Requested, EnqueueSent | Cancelled | Failed)
                | (EnqueueSent, RemoteActive | Failed | NeedsReconcile)
                | (
                    RemoteActive,
                    RemoteSucceeded | Failed | Cancelled | NeedsReconcile
                )
                | (
                    RemoteSucceeded,
                    Publishing | Failed | Cancelled | NeedsReconcile
                )
                | (
                    Publishing,
                    Published | PublishedNotDurable | Failed | Cancelled | NeedsReconcile
                )
                | (PublishedNotDurable, Published | NeedsReconcile)
                | (NeedsReconcile, Published | Failed | Cancelled)
        )
    }
}

/// Why a request failed (`failure_reason`); a fixed code, never remote text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureReason {
    RemoteRejected,
    RemoteFailed,
    TimedOut,
    SourceMissing,
    DestinationExists,
    IngestFailed,
    SettingsChanged,
    Manual,
}

impl FailureReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            FailureReason::RemoteRejected => "remote_rejected",
            FailureReason::RemoteFailed => "remote_failed",
            FailureReason::TimedOut => "timed_out",
            FailureReason::SourceMissing => "source_missing",
            FailureReason::DestinationExists => "destination_exists",
            FailureReason::IngestFailed => "ingest_failed",
            FailureReason::SettingsChanged => "settings_changed",
            FailureReason::Manual => "manual",
        }
    }
}

/// Why a request waits for a person (`reconcile_reason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileReason {
    /// The batch create was sent but its outcome is unknown (crash or lost answer).
    EnqueueUnknown,
    /// A publication was started and its outcome is unknown.
    PublishInterrupted,
    /// Published, but making it durable failed again.
    NotDurable,
    /// slskd no longer reports the identified transfer.
    RemoteUnknown,
}

impl ReconcileReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            ReconcileReason::EnqueueUnknown => "enqueue_unknown",
            ReconcileReason::PublishInterrupted => "publish_interrupted",
            ReconcileReason::NotDurable => "not_durable",
            ReconcileReason::RemoteUnknown => "remote_unknown",
        }
    }
}

/// A request field the database would refuse; no input is quoted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    Origin,
    RemoteUsername,
    RemoteFilename,
    Size,
    Destination,
}

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Invalid::Origin => "invalid slskd origin",
            Invalid::RemoteUsername => "invalid remote user name",
            Invalid::RemoteFilename => "invalid remote file name",
            Invalid::Size => "invalid size",
            Invalid::Destination => "invalid destination",
        })
    }
}

impl std::error::Error for Invalid {}

fn non_blank_within(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max
}

/// The destination below MUSIC_PATH: exactly `<artist>/<album>/<file>`. A light structural
/// check matching the table's CHECK; the worker validates it again with
/// `aether_library::ingest::RelPath` (which this crate cannot depend on) before any I/O.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestRel(String);

impl DestRel {
    pub fn parse(s: &str) -> Result<DestRel, Invalid> {
        let parts: Vec<&str> = s.split('/').collect();
        let ok = s.len() <= MAX_DEST_REL_BYTES
            && parts.len() == 3
            && parts.iter().all(|p| {
                !p.is_empty()
                    && p.len() <= 255
                    && *p != "."
                    && *p != ".."
                    && !p.starts_with(".aether-ingest-")
                    && !p.chars().any(|c| c.is_control() || c == '\\')
            })
            && !(s.len() >= 2 && s.as_bytes()[1] == b':' && s.as_bytes()[0].is_ascii_alphabetic());
        if ok {
            Ok(DestRel(s.to_string()))
        } else {
            Err(Invalid::Destination)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A validated new request. The origin and generation come from one
/// `settings_kv::slskd_connection` snapshot; the API key is never stored here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewDownload {
    profile: ProfileId,
    origin: String,
    generation: i64,
    username: String,
    filename: String,
    expected_size: i64,
    max_bytes: i64,
    dest: DestRel,
}

impl NewDownload {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        profile: ProfileId,
        origin: &str,
        generation: i64,
        remote_username: &str,
        remote_filename: &str,
        expected_size: u64,
        max_bytes: u64,
        dest: DestRel,
    ) -> Result<NewDownload, Invalid> {
        let origin_ok = origin.len() <= MAX_ORIGIN_BYTES
            && origin
                .strip_prefix("https://")
                .or_else(|| origin.strip_prefix("http://"))
                .is_some_and(|host| {
                    !host.is_empty()
                        && !host
                            .chars()
                            .any(|c| matches!(c, '/' | '?' | '#' | '@') || c.is_whitespace())
                });
        if !origin_ok || generation < 0 {
            return Err(Invalid::Origin);
        }
        if !non_blank_within(remote_username, MAX_REMOTE_USERNAME_BYTES) {
            return Err(Invalid::RemoteUsername);
        }
        if !non_blank_within(remote_filename, MAX_REMOTE_FILENAME_BYTES) {
            return Err(Invalid::RemoteFilename);
        }
        let expected_size = i64::try_from(expected_size)
            .ok()
            .filter(|n| *n > 0)
            .ok_or(Invalid::Size)?;
        let max_bytes = i64::try_from(max_bytes)
            .ok()
            .filter(|n| *n >= expected_size)
            .ok_or(Invalid::Size)?;
        Ok(NewDownload {
            profile,
            origin: origin.to_string(),
            generation,
            username: remote_username.to_string(),
            filename: remote_filename.to_string(),
            expected_size,
            max_bytes,
            dest,
        })
    }
}

/// What [`create`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateOutcome {
    Created(Uuid),
    /// The same profile already has this exact unfinished request.
    Existing(Uuid),
    /// Another unfinished request (another profile's, or a different one of this profile)
    /// holds the remote file or the destination. Says nothing about that request.
    Conflict,
}

#[derive(Debug)]
pub enum CreateError {
    /// The profile does not exist (deleted, possibly concurrently).
    ProfileGone,
    Db(sqlx::Error),
}

impl From<sqlx::Error> for CreateError {
    fn from(e: sqlx::Error) -> Self {
        CreateError::Db(e)
    }
}

impl std::fmt::Display for CreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CreateError::ProfileGone => f.write_str("profile not found"),
            CreateError::Db(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CreateError {}

const ACTIVE: &str = "state NOT IN ('published', 'failed', 'cancelled')";

/// Stores a new request in state `requested`, idempotently: at most one unfinished request
/// per remote file and per destination (partial unique indexes). Enqueues no job: the Task 4C
/// handler binds `job_id` in the same transaction as its job.
pub async fn create(pool: &PgPool, new: &NewDownload) -> Result<CreateOutcome, CreateError> {
    // A conflicting row can finish between the insert and the lookup; then insert again.
    for _ in 0..3 {
        let id = Uuid::now_v7();
        let inserted = sqlx::query(
            "INSERT INTO slskd_downloads (id, profile_id, batch_id, slskd_origin, slskd_generation,
                                          remote_username, remote_filename, expected_size,
                                          max_bytes, dest_rel)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT DO NOTHING",
        )
        .bind(id)
        .bind(new.profile.into_uuid())
        .bind(Uuid::new_v4())
        .bind(&new.origin)
        .bind(new.generation)
        .bind(&new.username)
        .bind(&new.filename)
        .bind(new.expected_size)
        .bind(new.max_bytes)
        .bind(new.dest.as_str())
        .execute(pool)
        .await;
        match inserted {
            Ok(done) if done.rows_affected() == 1 => return Ok(CreateOutcome::Created(id)),
            Ok(_) => {}
            Err(e) if e.as_database_error().and_then(|d| d.code()).as_deref() == Some("23503") => {
                return Err(CreateError::ProfileGone);
            }
            Err(e) => return Err(e.into()),
        }
        let rows = sqlx::query(&format!(
            "SELECT id, profile_id, slskd_origin, remote_username, remote_filename, dest_rel,
                    expected_size, max_bytes
               FROM slskd_downloads
              WHERE {ACTIVE}
                AND ((slskd_origin = $1 AND remote_username = $2 AND remote_filename = $3)
                     OR dest_rel = $4)"
        ))
        .bind(&new.origin)
        .bind(&new.username)
        .bind(&new.filename)
        .bind(new.dest.as_str())
        .fetch_all(pool)
        .await?;
        match rows.as_slice() {
            [] => continue,
            [row] => {
                let same = row.get::<Option<Uuid>, _>("profile_id")
                    == Some(new.profile.into_uuid())
                    && row.get::<String, _>("slskd_origin") == new.origin
                    && row.get::<String, _>("remote_username") == new.username
                    && row.get::<String, _>("remote_filename") == new.filename
                    && row.get::<String, _>("dest_rel") == new.dest.as_str()
                    && row.get::<i64, _>("expected_size") == new.expected_size
                    && row.get::<i64, _>("max_bytes") == new.max_bytes;
                return Ok(if same {
                    CreateOutcome::Existing(row.get("id"))
                } else {
                    CreateOutcome::Conflict
                });
            }
            _ => return Ok(CreateOutcome::Conflict),
        }
    }
    Ok(CreateOutcome::Conflict)
}

/// A stored request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub id: Uuid,
    /// `None` once the owning profile was deleted (a tombstone still being settled).
    pub profile: Option<ProfileId>,
    pub batch_id: Uuid,
    pub remote_transfer_id: Option<Uuid>,
    pub slskd_origin: String,
    pub slskd_generation: i64,
    pub remote_username: String,
    pub remote_filename: String,
    pub expected_size: u64,
    pub max_bytes: u64,
    pub dest_rel: String,
    pub state: State,
    pub attempt: i32,
    pub cancel_requested: bool,
    pub job_id: Option<Uuid>,
    pub failure_reason: Option<String>,
    pub reconcile_reason: Option<String>,
    pub receipt: Option<Receipt>,
}

const COLUMNS: &str = "id, profile_id, batch_id, remote_transfer_id, slskd_origin,
    slskd_generation, remote_username, remote_filename, expected_size, max_bytes, dest_rel,
    state, attempt, cancel_requested, job_id, failure_reason, reconcile_reason, receipt_bytes,
    receipt_sha256";

fn decode(row: &sqlx::postgres::PgRow) -> sqlx::Result<Download> {
    let state: String = row.try_get("state")?;
    let state = State::parse(&state).ok_or_else(|| sqlx::Error::Decode("unknown state".into()))?;
    let size = |c: &str| -> sqlx::Result<u64> {
        u64::try_from(row.try_get::<i64, _>(c)?).map_err(|e| sqlx::Error::Decode(e.into()))
    };
    let receipt = match (
        row.try_get::<Option<i64>, _>("receipt_bytes")?,
        row.try_get::<Option<Vec<u8>>, _>("receipt_sha256")?,
    ) {
        (Some(bytes), Some(sha)) => Some(Receipt {
            bytes: u64::try_from(bytes).map_err(|e| sqlx::Error::Decode(e.into()))?,
            sha256: sha
                .try_into()
                .map_err(|_| sqlx::Error::Decode("receipt hash length".into()))?,
        }),
        _ => None,
    };
    Ok(Download {
        id: row.try_get("id")?,
        profile: row
            .try_get::<Option<Uuid>, _>("profile_id")?
            .map(ProfileId::new),
        batch_id: row.try_get("batch_id")?,
        remote_transfer_id: row.try_get("remote_transfer_id")?,
        slskd_origin: row.try_get("slskd_origin")?,
        slskd_generation: row.try_get("slskd_generation")?,
        remote_username: row.try_get("remote_username")?,
        remote_filename: row.try_get("remote_filename")?,
        expected_size: size("expected_size")?,
        max_bytes: size("max_bytes")?,
        dest_rel: row.try_get("dest_rel")?,
        state,
        attempt: row.try_get("attempt")?,
        cancel_requested: row.try_get("cancel_requested")?,
        job_id: row.try_get("job_id")?,
        failure_reason: row.try_get("failure_reason")?,
        reconcile_reason: row.try_get("reconcile_reason")?,
        receipt,
    })
}

/// The request as its owner sees it; `None` for another profile's or a missing row.
pub async fn get_owned(
    pool: &PgPool,
    profile: ProfileId,
    id: Uuid,
) -> sqlx::Result<Option<Download>> {
    sqlx::query(&format!(
        "SELECT {COLUMNS} FROM slskd_downloads WHERE id = $1 AND profile_id = $2"
    ))
    .bind(id)
    .bind(profile.into_uuid())
    .fetch_optional(pool)
    .await?
    .as_ref()
    .map(decode)
    .transpose()
}

/// The request for the worker (any owner, tombstones included).
pub async fn load(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<Download>> {
    sqlx::query(&format!(
        "SELECT {COLUMNS} FROM slskd_downloads WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?
    .as_ref()
    .map(decode)
    .transpose()
}

/// Proof of one claimed attempt; only [`claim`] makes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fence {
    id: Uuid,
    attempt: i32,
}

impl Fence {
    pub fn id(self) -> Uuid {
        self.id
    }

    pub fn attempt(self) -> i32 {
        self.attempt
    }
}

/// The row as a new attempt found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claimed {
    pub fence: Fence,
    /// After recovery: an interrupted `enqueue_sent`/`publishing` is now `NeedsReconcile`.
    pub state: State,
    pub cancel_requested: bool,
    pub owner_gone: bool,
}

/// Starts a new attempt on an unfinished row, superseding every earlier fence. A step an
/// earlier attempt left in flight becomes `needs_reconcile` (never retried automatically).
/// `None` for a missing, finished or `needs_reconcile` row (manual resolution only).
pub async fn claim(pool: &PgPool, id: Uuid) -> sqlx::Result<Option<Claimed>> {
    let row = sqlx::query(
        "UPDATE slskd_downloads
            SET attempt = attempt + 1,
                state = CASE WHEN state IN ('enqueue_sent', 'publishing')
                             THEN 'needs_reconcile' ELSE state END,
                reconcile_reason = CASE state WHEN 'enqueue_sent' THEN 'enqueue_unknown'
                                              WHEN 'publishing' THEN 'publish_interrupted'
                                              ELSE reconcile_reason END
          WHERE id = $1
            AND state IN ('requested', 'enqueue_sent', 'remote_active', 'remote_succeeded',
                          'publishing', 'published_not_durable')
          RETURNING attempt, state, cancel_requested, profile_id IS NULL AS owner_gone",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    row.map(|row| {
        let state: String = row.try_get("state")?;
        Ok(Claimed {
            fence: Fence {
                id,
                attempt: row.try_get("attempt")?,
            },
            state: State::parse(&state)
                .ok_or_else(|| sqlx::Error::Decode("unknown state".into()))?,
            cancel_requested: row.try_get("cancel_requested")?,
            owner_gone: row.try_get("owner_gone")?,
        })
    })
    .transpose()
}

/// Runs one fenced step: `UPDATE ... SET <set> WHERE id AND attempt AND state = ANY(from)
/// <guard>`. `$1` id, `$2` attempt, `$3` from-states; extra binds start at `$4`.
macro_rules! run_step {
    ($pool:expr, $fence:expr, $set:expr, $from:expr, $guard:expr $(, $bind:expr)*) => {{
        let sql = format!(
            "UPDATE slskd_downloads SET {}
              WHERE id = $1 AND attempt = $2 AND state = ANY($3){}",
            $set, $guard
        );
        let from: Vec<&'static str> = $from.iter().map(|s: &State| s.as_str()).collect();
        let done = sqlx::query(&sql)
            .bind($fence.id)
            .bind($fence.attempt)
            .bind(from)
            $(.bind($bind))*
            .execute($pool)
            .await?;
        Ok(done.rows_affected() == 1)
    }};
}

/// `requested -> enqueue_sent`, written **before** the batch create is sent: a crash after
/// this point is recovered as `needs_reconcile`, never by sending again. Refused once a cancel
/// was asked or the owner is gone.
pub async fn mark_enqueue_sent(pool: &PgPool, fence: Fence) -> sqlx::Result<bool> {
    run_step!(
        pool,
        fence,
        "state = 'enqueue_sent'",
        &[State::Requested],
        " AND NOT cancel_requested AND profile_id IS NOT NULL"
    )
}

/// `enqueue_sent -> remote_active` once slskd identified the transfer.
pub async fn mark_remote_active(pool: &PgPool, fence: Fence, transfer: Uuid) -> sqlx::Result<bool> {
    run_step!(
        pool,
        fence,
        "state = 'remote_active', remote_transfer_id = $4",
        &[State::EnqueueSent],
        "",
        transfer
    )
}

/// `remote_active -> remote_succeeded`: slskd reports the transfer completed.
pub async fn mark_remote_succeeded(pool: &PgPool, fence: Fence) -> sqlx::Result<bool> {
    run_step!(
        pool,
        fence,
        "state = 'remote_succeeded'",
        &[State::RemoteActive],
        ""
    )
}

/// `remote_succeeded -> publishing`, the last point where cancellation or profile deletion
/// wins: refused when a cancel was asked or the owner is gone. Profile deletion locks and
/// rewrites the same row (trigger), so the two are serialized on it: whichever commits first
/// decides.
pub async fn begin_publish(pool: &PgPool, fence: Fence) -> sqlx::Result<bool> {
    run_step!(
        pool,
        fence,
        "state = 'publishing'",
        &[State::RemoteSucceeded],
        " AND NOT cancel_requested AND profile_id IS NOT NULL"
    )
}

/// The publication's receipt (`aether_library::ingest::Ingested` bytes and SHA-256).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Receipt {
    pub bytes: u64,
    pub sha256: [u8; 32],
}

/// `publishing -> published` (`durable`) or `published_not_durable`, recording the receipt
/// whose size must equal the expected size. Applies even when a cancel was asked meanwhile:
/// after publication nothing is rolled back.
pub async fn finish_publish(
    pool: &PgPool,
    fence: Fence,
    receipt: Receipt,
    durable: bool,
) -> sqlx::Result<bool> {
    let bytes = i64::try_from(receipt.bytes).map_err(|e| sqlx::Error::Encode(e.into()))?;
    let set = if durable {
        "state = 'published', receipt_bytes = $4, receipt_sha256 = $5, finished_at = now()"
    } else {
        "state = 'published_not_durable', receipt_bytes = $4, receipt_sha256 = $5"
    };
    run_step!(
        pool,
        fence,
        set,
        &[State::Publishing],
        "",
        bytes,
        receipt.sha256.to_vec()
    )
}

/// `published_not_durable -> published` after the worker re-synced the destination.
pub async fn mark_durable(pool: &PgPool, fence: Fence) -> sqlx::Result<bool> {
    run_step!(
        pool,
        fence,
        "state = 'published', finished_at = now()",
        &[State::PublishedNotDurable],
        ""
    )
}

/// Fails an unfinished, unpublished request.
pub async fn mark_failed(pool: &PgPool, fence: Fence, reason: FailureReason) -> sqlx::Result<bool> {
    run_step!(
        pool,
        fence,
        "state = 'failed', failure_reason = $4, finished_at = now()",
        &[
            State::Requested,
            State::EnqueueSent,
            State::RemoteActive,
            State::RemoteSucceeded,
            State::Publishing,
        ],
        "",
        reason.as_str()
    )
}

/// Confirms an asked cancellation (remote transfer cancelled, or the ingest returned
/// `Cancelled` before publication).
pub async fn mark_cancelled(pool: &PgPool, fence: Fence) -> sqlx::Result<bool> {
    run_step!(
        pool,
        fence,
        "state = 'cancelled', finished_at = now()",
        &[
            State::RemoteActive,
            State::RemoteSucceeded,
            State::Publishing
        ],
        " AND cancel_requested"
    )
}

/// Hands an uncertain request to a person.
pub async fn mark_needs_reconcile(
    pool: &PgPool,
    fence: Fence,
    reason: ReconcileReason,
) -> sqlx::Result<bool> {
    run_step!(
        pool,
        fence,
        "state = 'needs_reconcile', reconcile_reason = $4",
        &[
            State::EnqueueSent,
            State::RemoteActive,
            State::RemoteSucceeded,
            State::Publishing,
            State::PublishedNotDurable,
        ],
        "",
        reason.as_str()
    )
}

/// What [`request_cancel`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelOutcome {
    /// Nothing was sent yet: cancelled at once.
    Cancelled,
    /// Remote or publication work may be under way: the worker settles it.
    Requested,
    /// Already published, failed or cancelled; unchanged.
    AlreadyFinished,
    /// No such request of this profile.
    NotFound,
}

/// The owner asks to cancel. A published (even not yet durable) request is finished.
pub async fn request_cancel(
    pool: &PgPool,
    profile: ProfileId,
    id: Uuid,
) -> sqlx::Result<CancelOutcome> {
    let state: Option<String> = sqlx::query_scalar(
        "UPDATE slskd_downloads
            SET state = CASE WHEN state = 'requested' THEN 'cancelled' ELSE state END,
                finished_at = CASE WHEN state = 'requested' THEN now() ELSE finished_at END,
                cancel_requested = true
          WHERE id = $1 AND profile_id = $2
            AND state IN ('requested', 'enqueue_sent', 'remote_active', 'remote_succeeded',
                          'publishing', 'needs_reconcile')
          RETURNING state",
    )
    .bind(id)
    .bind(profile.into_uuid())
    .fetch_optional(pool)
    .await?;
    if let Some(state) = state {
        return Ok(if state == "cancelled" {
            CancelOutcome::Cancelled
        } else {
            CancelOutcome::Requested
        });
    }
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM slskd_downloads WHERE id = $1 AND profile_id = $2)",
    )
    .bind(id)
    .bind(profile.into_uuid())
    .fetch_one(pool)
    .await?;
    Ok(if exists {
        CancelOutcome::AlreadyFinished
    } else {
        CancelOutcome::NotFound
    })
}

/// A person's decision on a `needs_reconcile` request (the admin permission check belongs to
/// the Task 4C route).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Published(Receipt),
    Failed,
    Cancelled,
}

/// Resolves a `needs_reconcile` request; also advances the fence so no older worker attempt
/// can touch the row afterwards. `false` when the row is not waiting for reconciliation.
pub async fn resolve(pool: &PgPool, id: Uuid, resolution: Resolution) -> sqlx::Result<bool> {
    const BUMP: &str = "reconcile_reason = NULL, finished_at = now(), attempt = attempt + 1";
    const WHERE: &str = "WHERE id = $1 AND state = 'needs_reconcile'";
    let query = match resolution {
        Resolution::Published(r) => {
            let bytes = i64::try_from(r.bytes).map_err(|e| sqlx::Error::Encode(e.into()))?;
            sqlx::query(&format!(
                "UPDATE slskd_downloads SET state = 'published', receipt_bytes = $2,
                        receipt_sha256 = $3, {BUMP} {WHERE}"
            ))
            .bind(id)
            .bind(bytes)
            .bind(r.sha256.to_vec())
            .execute(pool)
            .await?
        }
        Resolution::Failed => {
            sqlx::query(&format!(
                "UPDATE slskd_downloads SET state = 'failed', failure_reason = 'manual',
                        receipt_bytes = NULL, receipt_sha256 = NULL, {BUMP} {WHERE}"
            ))
            .bind(id)
            .execute(pool)
            .await?
        }
        Resolution::Cancelled => {
            sqlx::query(&format!(
                "UPDATE slskd_downloads SET state = 'cancelled', receipt_bytes = NULL,
                        receipt_sha256 = NULL, {BUMP} {WHERE}"
            ))
            .bind(id)
            .execute(pool)
            .await?
        }
    };
    Ok(query.rows_affected() == 1)
}
