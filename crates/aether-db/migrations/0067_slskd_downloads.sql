-- M7 Task 4B (D-294): durable, profile-owned slskd download requests. Database state only: no
-- route, job kind or network code uses this table yet (Task 4C adds the worker that drives it).
--
-- One row is one requested remote file. `state` follows a fixed graph (trigger below); every
-- worker step is a compare-and-set on (id, attempt, state) in `aether_db::slskd_downloads`.
-- `attempt` is the fence: each claim increments it, so a worker whose claim was superseded can
-- no longer change the row.
--
-- Ownership (M-PROF exception, documented in D-294): `profile_id` is `ON DELETE SET NULL`, not
-- CASCADE. Deleting a profile first deletes every row with nothing pending remotely (requested
-- or finished: no personal history is kept), and marks every other row `cancel_requested`
-- (trigger `profiles_slskd_downloads_owner_gone`); the foreign key then leaves those rows as
-- ownerless tombstones that the worker still has to settle (cancel the remote transfer, finish
-- or abandon a publication, manual reconciliation). A tombstone holds only the bounded remote
-- user name, remote file name, destination and slskd origin, never a credential, and is deleted
-- as soon as it reaches a terminal state (trigger `slskd_downloads_drop_settled_orphan`).
-- Files already published into the shared library are never touched by the database.

CREATE TABLE slskd_downloads (
    id                 uuid PRIMARY KEY,
    profile_id         uuid REFERENCES profiles (id) ON DELETE SET NULL,
    -- Caller-chosen UUID for the slskd batch (`POST /api/v0/transfers/downloads/batches` takes
    -- a caller-supplied `Id` in slskd 0.26.2 source; research only, not a verified runtime or
    -- minimum version). Stable across retries so reconciliation can look it up.
    batch_id           uuid NOT NULL UNIQUE,
    -- The slskd transfer's GUID once identified; the only transfer this row may cancel.
    remote_transfer_id uuid,
    -- Settings snapshot taken at request time (never the API key): the normalized origin and
    -- the connection generation `settings_kv::slskd_connection` reported.
    slskd_origin       text NOT NULL
                       CHECK (octet_length(slskd_origin) <= 512
                              AND slskd_origin ~ '^https?://[^/?#@[:space:]]+$'),
    slskd_generation   bigint NOT NULL CHECK (slskd_generation >= 0),
    -- Peer text as accepted from search (D-292 bounds): non-blank, at most 255 / 1024 bytes.
    remote_username    text NOT NULL
                       CHECK (octet_length(remote_username) BETWEEN 1 AND 255
                              AND remote_username ~ '[^[:space:]]'),
    remote_filename    text NOT NULL
                       CHECK (octet_length(remote_filename) BETWEEN 1 AND 1024
                              AND remote_filename ~ '[^[:space:]]'),
    expected_size      bigint NOT NULL CHECK (expected_size > 0),
    max_bytes          bigint NOT NULL CHECK (max_bytes >= expected_size),
    -- `<artist>/<album>/<file>` under MUSIC_PATH (owner decision 1). The worker validates it
    -- again with `aether_library::ingest::RelPath` before touching the filesystem.
    dest_rel           text NOT NULL
                       CHECK (octet_length(dest_rel) <= 1024
                              AND dest_rel ~ '^[^/]+/[^/]+/[^/]+$'
                              AND dest_rel !~ '[[:cntrl:]\\]'
                              AND dest_rel !~ '(^|/)\.\.?(/|$)'
                              AND dest_rel !~ '(^|/)\.aether-ingest-'
                              AND dest_rel !~ '^[A-Za-z]:'),
    state              text NOT NULL DEFAULT 'requested'
                       CHECK (state IN ('requested', 'enqueue_sent', 'remote_active',
                                        'remote_succeeded', 'publishing', 'published',
                                        'published_not_durable', 'failed', 'cancelled',
                                        'needs_reconcile')),
    attempt            integer NOT NULL DEFAULT 0 CHECK (attempt >= 0),
    cancel_requested   boolean NOT NULL DEFAULT false,
    -- The queue job driving this row; bound by Task 4C together with its handler.
    job_id             uuid REFERENCES jobs (id) ON DELETE SET NULL,
    failure_reason     text CHECK (failure_reason IN ('remote_rejected', 'remote_failed',
                                                      'timed_out', 'source_missing',
                                                      'destination_exists', 'ingest_failed',
                                                      'settings_changed', 'manual')),
    reconcile_reason   text CHECK (reconcile_reason IN ('enqueue_unknown',
                                                        'publish_interrupted', 'not_durable',
                                                        'remote_unknown')),
    receipt_bytes      bigint,
    receipt_sha256     bytea CHECK (octet_length(receipt_sha256) = 32),
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now(),
    finished_at        timestamptz,
    CONSTRAINT slskd_downloads_requested_has_owner
        CHECK (profile_id IS NOT NULL OR state <> 'requested'),
    CONSTRAINT slskd_downloads_failure
        CHECK ((failure_reason IS NOT NULL) = (state = 'failed')),
    CONSTRAINT slskd_downloads_reconcile
        CHECK ((reconcile_reason IS NOT NULL) = (state = 'needs_reconcile')),
    CONSTRAINT slskd_downloads_finished
        CHECK ((finished_at IS NOT NULL) = (state IN ('published', 'failed', 'cancelled'))),
    CONSTRAINT slskd_downloads_receipt_pair
        CHECK ((receipt_bytes IS NULL) = (receipt_sha256 IS NULL)),
    CONSTRAINT slskd_downloads_receipt_size
        CHECK (receipt_bytes IS NULL OR receipt_bytes = expected_size),
    CONSTRAINT slskd_downloads_receipt_state
        CHECK (CASE
                   WHEN state IN ('published', 'published_not_durable') THEN receipt_bytes IS NOT NULL
                   WHEN state = 'needs_reconcile' THEN true
                   ELSE receipt_bytes IS NULL
               END),
    CONSTRAINT slskd_downloads_transfer_known
        CHECK (remote_transfer_id IS NOT NULL
               OR state NOT IN ('remote_active', 'remote_succeeded', 'publishing',
                                'published_not_durable'))
);

-- Idempotency: one unfinished request per remote file and per destination.
CREATE UNIQUE INDEX slskd_downloads_active_remote
    ON slskd_downloads (slskd_origin, remote_username, remote_filename)
    WHERE state NOT IN ('published', 'failed', 'cancelled');
CREATE UNIQUE INDEX slskd_downloads_active_dest
    ON slskd_downloads (dest_rel)
    WHERE state NOT IN ('published', 'failed', 'cancelled');
-- Owner listing and the profile-deletion trigger.
CREATE INDEX slskd_downloads_profile ON slskd_downloads (profile_id, created_at);

-- The state graph, write-once columns, the monotone fence and the irreversible cancel flag,
-- enforced for every writer (not only the Rust helpers).
CREATE FUNCTION slskd_downloads_guard() RETURNS trigger
    LANGUAGE plpgsql AS
$$
BEGIN
    IF NEW.id IS DISTINCT FROM OLD.id
       OR NEW.batch_id IS DISTINCT FROM OLD.batch_id
       OR NEW.slskd_origin IS DISTINCT FROM OLD.slskd_origin
       OR NEW.slskd_generation IS DISTINCT FROM OLD.slskd_generation
       OR NEW.remote_username IS DISTINCT FROM OLD.remote_username
       OR NEW.remote_filename IS DISTINCT FROM OLD.remote_filename
       OR NEW.expected_size IS DISTINCT FROM OLD.expected_size
       OR NEW.max_bytes IS DISTINCT FROM OLD.max_bytes
       OR NEW.dest_rel IS DISTINCT FROM OLD.dest_rel
       OR NEW.created_at IS DISTINCT FROM OLD.created_at
       OR (NEW.profile_id IS DISTINCT FROM OLD.profile_id AND NEW.profile_id IS NOT NULL)
       OR (OLD.remote_transfer_id IS NOT NULL
           AND NEW.remote_transfer_id IS DISTINCT FROM OLD.remote_transfer_id)
       OR (OLD.job_id IS NOT NULL AND NEW.job_id IS NOT NULL AND NEW.job_id <> OLD.job_id)
       OR NEW.attempt < OLD.attempt
       OR (OLD.cancel_requested AND NOT NEW.cancel_requested) THEN
        RAISE EXCEPTION 'slskd download field cannot change this way'
            USING ERRCODE = 'P0001', HINT = 'aether:download_immutable';
    END IF;
    IF NEW.state <> OLD.state AND (OLD.state, NEW.state) NOT IN (
        ('requested', 'enqueue_sent'), ('requested', 'cancelled'), ('requested', 'failed'),
        ('enqueue_sent', 'remote_active'), ('enqueue_sent', 'failed'),
        ('enqueue_sent', 'needs_reconcile'),
        ('remote_active', 'remote_succeeded'), ('remote_active', 'failed'),
        ('remote_active', 'cancelled'), ('remote_active', 'needs_reconcile'),
        ('remote_succeeded', 'publishing'), ('remote_succeeded', 'failed'),
        ('remote_succeeded', 'cancelled'), ('remote_succeeded', 'needs_reconcile'),
        ('publishing', 'published'), ('publishing', 'published_not_durable'),
        ('publishing', 'failed'), ('publishing', 'cancelled'), ('publishing', 'needs_reconcile'),
        ('published_not_durable', 'published'), ('published_not_durable', 'needs_reconcile'),
        ('needs_reconcile', 'published'), ('needs_reconcile', 'failed'),
        ('needs_reconcile', 'cancelled')) THEN
        RAISE EXCEPTION 'slskd download cannot move from % to %', OLD.state, NEW.state
            USING ERRCODE = 'P0001', HINT = 'aether:download_transition';
    END IF;
    NEW.updated_at := now();
    RETURN NEW;
END
$$;
CREATE TRIGGER slskd_downloads_guard BEFORE UPDATE ON slskd_downloads
    FOR EACH ROW EXECUTE FUNCTION slskd_downloads_guard();

-- An ownerless row is kept only while something is still pending; once settled it goes.
CREATE FUNCTION slskd_downloads_drop_settled_orphan() RETURNS trigger
    LANGUAGE plpgsql AS
$$
BEGIN
    DELETE FROM slskd_downloads WHERE id = NEW.id;
    RETURN NULL;
END
$$;
CREATE TRIGGER slskd_downloads_drop_settled_orphan AFTER UPDATE ON slskd_downloads
    FOR EACH ROW
    WHEN (NEW.profile_id IS NULL AND NEW.state IN ('published', 'failed', 'cancelled'))
    EXECUTE FUNCTION slskd_downloads_drop_settled_orphan();

-- Owner decision 3: deleting a profile cancels its unfinished downloads. Runs before the
-- profile row goes (and before the foreign key's SET NULL). Order matters under READ COMMITTED:
-- the DELETE first, then the UPDATE with a fresh snapshot, so a row a worker moves from
-- `requested` to `enqueue_sent` in between is caught by the UPDATE. A row the worker has
-- already moved to `publishing` keeps its state: publication is the linearization point, and
-- the worker records whatever the publication returns (a published file stays in the library).
CREATE FUNCTION slskd_downloads_owner_gone() RETURNS trigger
    LANGUAGE plpgsql AS
$$
BEGIN
    DELETE FROM slskd_downloads
     WHERE profile_id = OLD.id AND state IN ('requested', 'published', 'failed', 'cancelled');
    UPDATE slskd_downloads SET cancel_requested = true
     WHERE profile_id = OLD.id AND NOT cancel_requested;
    RETURN OLD;
END
$$;
CREATE TRIGGER profiles_slskd_downloads_owner_gone BEFORE DELETE ON profiles
    FOR EACH ROW EXECUTE FUNCTION slskd_downloads_owner_gone();
