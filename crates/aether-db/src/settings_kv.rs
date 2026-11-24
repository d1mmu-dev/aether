//! Key-value system settings (spec §6.1, §15.1). Keys and types are fixed in code; an absent
//! row is the default. Secrets are encrypted with the M2 cipher, AAD `settings_kv:<key>`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use aether_core::cipher::Cipher;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    AcoustidApiKey,
    FanartApiKey,
    FanartEnabled,
    AutoEnrichMetadata,
    EnrichmentConcurrency,
    AutotagAutoApply,
    /// ISO 3166 alpha-2, lowercase; the Apple podcast chart country (M6 Task 17, D-038 amended).
    PodcastChartCountry,
    /// M7: the Soulseek integration switch; off unless an admin turns it on (D-291).
    SoulseekEnabled,
    /// M7: the slskd origin, validated by `aether_net::slskd::SlskdUrl` before it is stored.
    SlskdUrl,
    /// M7: the slskd API key (secret).
    SlskdApiKey,
}

impl Key {
    pub const ALL: [Key; 10] = [
        Key::AcoustidApiKey,
        Key::FanartApiKey,
        Key::FanartEnabled,
        Key::AutoEnrichMetadata,
        Key::EnrichmentConcurrency,
        Key::AutotagAutoApply,
        Key::PodcastChartCountry,
        Key::SoulseekEnabled,
        Key::SlskdUrl,
        Key::SlskdApiKey,
    ];

    /// The camelCase key of spec §15.1, used as the row key and the JSON field.
    pub fn name(self) -> &'static str {
        match self {
            Key::AcoustidApiKey => "acoustidApiKey",
            Key::FanartApiKey => "fanartApiKey",
            Key::FanartEnabled => "fanartEnabled",
            Key::AutoEnrichMetadata => "autoEnrichMetadata",
            Key::EnrichmentConcurrency => "enrichmentConcurrency",
            Key::AutotagAutoApply => "autotagAutoApply",
            Key::PodcastChartCountry => "podcastChartCountry",
            Key::SoulseekEnabled => "soulseekEnabled",
            Key::SlskdUrl => "slskdUrl",
            Key::SlskdApiKey => "slskdApiKey",
        }
    }

    pub fn is_secret(self) -> bool {
        matches!(
            self,
            Key::AcoustidApiKey | Key::FanartApiKey | Key::SlskdApiKey
        )
    }

    fn aad(self) -> Vec<u8> {
        [b"settings_kv:".as_slice(), self.name().as_bytes()].concat()
    }

    fn from_name(name: &str) -> Option<Key> {
        Key::ALL.into_iter().find(|k| k.name() == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoApply {
    #[default]
    Off,
    Db,
    Files,
}

impl AutoApply {
    pub fn as_str(self) -> &'static str {
        match self {
            AutoApply::Off => "off",
            AutoApply::Db => "db",
            AutoApply::Files => "files",
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct KvSettings {
    pub acoustid_api_key: Option<String>,
    pub fanart_api_key: Option<String>,
    pub fanart_enabled: bool,
    pub auto_enrich_metadata: bool,
    pub enrichment_concurrency: u8,
    pub autotag_auto_apply: AutoApply,
    pub podcast_chart_country: String,
    pub soulseek_enabled: bool,
    pub slskd_url: Option<String>,
    pub slskd_api_key: Option<String>,
}

impl Default for KvSettings {
    fn default() -> Self {
        Self {
            acoustid_api_key: None,
            fanart_api_key: None,
            fanart_enabled: false,
            auto_enrich_metadata: true,
            enrichment_concurrency: 1,
            autotag_auto_apply: AutoApply::Off,
            podcast_chart_country: "us".to_string(),
            soulseek_enabled: false,
            slskd_url: None,
            slskd_api_key: None,
        }
    }
}

// Never prints a secret.
impl std::fmt::Debug for KvSettings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KvSettings")
            .field(
                "acoustid_api_key",
                &self.acoustid_api_key.as_ref().map(|_| ".."),
            )
            .field(
                "fanart_api_key",
                &self.fanart_api_key.as_ref().map(|_| ".."),
            )
            .field("fanart_enabled", &self.fanart_enabled)
            .field("auto_enrich_metadata", &self.auto_enrich_metadata)
            .field("enrichment_concurrency", &self.enrichment_concurrency)
            .field("autotag_auto_apply", &self.autotag_auto_apply)
            .field("podcast_chart_country", &self.podcast_chart_country)
            .field("soulseek_enabled", &self.soulseek_enabled)
            .field("slskd_url", &self.slskd_url)
            .field("slskd_api_key", &self.slskd_api_key.as_ref().map(|_| ".."))
            .finish()
    }
}

/// Logs an undecryptable secret at most once an hour per key; the key name only.
fn warn_undecryptable(key: Key) {
    static LAST: OnceLock<Mutex<HashMap<Key, Instant>>> = OnceLock::new();
    let mut last = LAST
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    if last
        .get(&key)
        .is_none_or(|t| now.duration_since(*t) >= Duration::from_secs(3600))
    {
        last.insert(key, now);
        tracing::warn!(
            key = key.name(),
            "stored secret cannot be decrypted; treating it as unset"
        );
    }
}

pub async fn load(pool: &PgPool, cipher: &Cipher) -> sqlx::Result<KvSettings> {
    let rows = sqlx::query!("SELECT key, value, secret_enc FROM settings_kv")
        .fetch_all(pool)
        .await?;
    let mut out = KvSettings::default();
    for row in rows {
        let Some(key) = Key::from_name(&row.key) else {
            continue;
        };
        if key.is_secret() {
            let Some(blob) = row.secret_enc else { continue };
            let plain = cipher
                .decrypt(&blob, &key.aad())
                .ok()
                .and_then(|b| String::from_utf8(b).ok());
            if plain.is_none() {
                warn_undecryptable(key);
            }
            match key {
                Key::AcoustidApiKey => out.acoustid_api_key = plain,
                Key::SlskdApiKey => out.slskd_api_key = plain,
                _ => out.fanart_api_key = plain,
            }
            continue;
        }
        let Some(value) = row.value else { continue };
        match key {
            Key::FanartEnabled => out.fanart_enabled = value.as_bool().unwrap_or(false),
            Key::AutoEnrichMetadata => out.auto_enrich_metadata = value.as_bool().unwrap_or(true),
            Key::EnrichmentConcurrency => {
                if let Some(n) = value.as_u64() {
                    out.enrichment_concurrency = n.clamp(1, 5) as u8;
                }
            }
            Key::AutotagAutoApply => {
                out.autotag_auto_apply = serde_json::from_value(value).unwrap_or_default();
            }
            Key::PodcastChartCountry => {
                if let Some(s) = value.as_str() {
                    out.podcast_chart_country = s.to_string();
                }
            }
            Key::SoulseekEnabled => out.soulseek_enabled = value.as_bool().unwrap_or(false),
            Key::SlskdUrl => out.slskd_url = value.as_str().map(str::to_string),
            _ => {}
        }
    }
    Ok(out)
}

/// Stores a plain setting; `None` deletes the row (back to the default).
pub async fn set_plain(
    pool: impl sqlx::PgExecutor<'_>,
    key: Key,
    value: Option<&Value>,
) -> sqlx::Result<()> {
    assert!(!key.is_secret(), "{} is a secret", key.name());
    match value {
        None => delete(pool, key).await,
        Some(value) => {
            sqlx::query!(
                r#"INSERT INTO settings_kv (key, value, secret_enc) VALUES ($1, $2, NULL)
                   ON CONFLICT (key) DO UPDATE
                   SET value = EXCLUDED.value, secret_enc = NULL, updated_at = now()"#,
                key.name(),
                value
            )
            .execute(pool)
            .await?;
            Ok(())
        }
    }
}

/// Stores a secret encrypted; `None` deletes the row.
pub async fn set_secret(
    pool: impl sqlx::PgExecutor<'_>,
    cipher: &Cipher,
    key: Key,
    value: Option<&str>,
) -> sqlx::Result<()> {
    assert!(key.is_secret(), "{} is not a secret", key.name());
    match value {
        None => delete(pool, key).await,
        Some(value) => {
            let blob = cipher.encrypt(value.as_bytes(), &key.aad());
            sqlx::query!(
                r#"INSERT INTO settings_kv (key, value, secret_enc) VALUES ($1, NULL, $2)
                   ON CONFLICT (key) DO UPDATE
                   SET value = NULL, secret_enc = EXCLUDED.secret_enc, updated_at = now()"#,
                key.name(),
                blob
            )
            .execute(pool)
            .await?;
            Ok(())
        }
    }
}

async fn delete(pool: impl sqlx::PgExecutor<'_>, key: Key) -> sqlx::Result<()> {
    sqlx::query!("DELETE FROM settings_kv WHERE key = $1", key.name())
        .execute(pool)
        .await?;
    Ok(())
}

/// How a save changes the slskd API key.
#[derive(Clone, PartialEq, Eq)]
pub enum KeyChange {
    /// Absent or blank in the body: not a fresh key.
    Keep,
    /// `null`: delete the stored key.
    Clear,
    /// A fresh, already validated key.
    Set(String),
}

impl std::fmt::Debug for KeyChange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Keep => "Keep",
            Self::Clear => "Clear",
            Self::Set(_) => "Set([redacted])",
        })
    }
}

/// Settings row holding the slskd connection generation (D-294). Not a [`Key`]: the settings
/// API reads and writes only `Key::ALL`, so no request body can set it and `GET` never shows it.
const SLSKD_GENERATION: &str = "slskdGeneration";

/// Saves the slskd URL/key pair in one transaction (review I-1, D-291). `url`: `None` leaves the
/// URL alone, `Some(None)` clears it, `Some(Some(origin))` stores the normalized origin. A key
/// belongs to the address it was saved for: when the URL changes (set, replaced or cleared) the
/// stored key is deleted unless `key` sets a fresh one. The current URL is read from the table,
/// not a cache, under a transaction advisory lock, so competing pair saves (key-only ones too,
/// and also while neither row exists) are serialized.
///
/// When the stored URL or key actually changes, the connection generation is incremented in the
/// same transaction (D-294): a different key, a cleared key or a changed URL bumps it; `Keep`,
/// an equal URL or re-saving the same key does not. `soulseekEnabled` is not part of it (the
/// worker reads that switch fresh). The generation holds no key material.
pub async fn save_slskd(
    pool: &PgPool,
    cipher: &Cipher,
    url: Option<Option<&str>>,
    key: KeyChange,
) -> sqlx::Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(7003, hashtext('settings_kv:slskd'))")
        .execute(&mut *tx)
        .await?;
    let current: Option<String> =
        sqlx::query_scalar("SELECT value #>> '{}' FROM settings_kv WHERE key = $1")
            .bind(Key::SlskdUrl.name())
            .fetch_optional(&mut *tx)
            .await?
            .flatten();
    let url_changed = url.is_some_and(|new| new != current.as_deref());
    if let Some(new) = url.filter(|_| url_changed) {
        let new = new.map(|u| Value::String(u.to_string()));
        set_plain(&mut *tx, Key::SlskdUrl, new.as_ref()).await?;
    }
    let mut changed = url_changed;
    match key {
        KeyChange::Set(k) => {
            let stored: Option<Vec<u8>> =
                sqlx::query_scalar("SELECT secret_enc FROM settings_kv WHERE key = $1")
                    .bind(Key::SlskdApiKey.name())
                    .fetch_optional(&mut *tx)
                    .await?
                    .flatten();
            let same = stored
                .and_then(|blob| cipher.decrypt(&blob, &Key::SlskdApiKey.aad()).ok())
                .is_some_and(|plain| plain == k.as_bytes());
            changed |= !same;
            set_secret(&mut *tx, cipher, Key::SlskdApiKey, Some(&k)).await?;
        }
        KeyChange::Clear => changed |= delete_counted(&mut *tx, Key::SlskdApiKey).await?,
        KeyChange::Keep if url_changed => {
            delete_counted(&mut *tx, Key::SlskdApiKey).await?;
        }
        KeyChange::Keep => {}
    }
    if changed {
        sqlx::query(
            "INSERT INTO settings_kv (key, value) VALUES ($1, '1'::jsonb)
             ON CONFLICT (key) DO UPDATE
             SET value = to_jsonb(coalesce((settings_kv.value #>> '{}')::bigint, 0) + 1),
                 updated_at = now()",
        )
        .bind(SLSKD_GENERATION)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await
}

async fn delete_counted(pool: impl sqlx::PgExecutor<'_>, key: Key) -> sqlx::Result<bool> {
    let done = sqlx::query("DELETE FROM settings_kv WHERE key = $1")
        .bind(key.name())
        .execute(pool)
        .await?;
    Ok(done.rows_affected() > 0)
}

/// One consistent read of the slskd connection settings, straight from the table (not the
/// cache), for the download path: a request stores `url` and `generation`, never the key.
#[derive(Clone, PartialEq, Eq)]
pub struct SlskdConnection {
    pub enabled: bool,
    pub url: Option<String>,
    pub api_key: Option<String>,
    /// 0 until the URL or key is first changed; incremented by every such change.
    pub generation: i64,
}

impl std::fmt::Debug for SlskdConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlskdConnection")
            .field("enabled", &self.enabled)
            .field("url", &self.url)
            .field("api_key", &self.api_key.as_ref().map(|_| ".."))
            .field("generation", &self.generation)
            .finish()
    }
}

pub async fn slskd_connection(pool: &PgPool, cipher: &Cipher) -> sqlx::Result<SlskdConnection> {
    // One statement, one snapshot: the URL, key and generation belong together.
    let rows: Vec<(String, Option<Value>, Option<Vec<u8>>)> =
        sqlx::query_as("SELECT key, value, secret_enc FROM settings_kv WHERE key = ANY($1)")
            .bind(vec![
                Key::SoulseekEnabled.name(),
                Key::SlskdUrl.name(),
                Key::SlskdApiKey.name(),
                SLSKD_GENERATION,
            ])
            .fetch_all(pool)
            .await?;
    let mut out = SlskdConnection {
        enabled: false,
        url: None,
        api_key: None,
        generation: 0,
    };
    for (key, value, secret) in rows {
        match key.as_str() {
            k if k == Key::SoulseekEnabled.name() => {
                out.enabled = value.and_then(|v| v.as_bool()).unwrap_or(false);
            }
            k if k == Key::SlskdUrl.name() => {
                out.url = value.and_then(|v| v.as_str().map(str::to_string));
            }
            k if k == Key::SlskdApiKey.name() => {
                out.api_key = secret.and_then(|blob| {
                    let plain = cipher
                        .decrypt(&blob, &Key::SlskdApiKey.aad())
                        .ok()
                        .and_then(|b| String::from_utf8(b).ok());
                    if plain.is_none() {
                        warn_undecryptable(Key::SlskdApiKey);
                    }
                    plain
                });
            }
            _ => out.generation = value.and_then(|v| v.as_i64()).unwrap_or(0),
        }
    }
    Ok(out)
}

type Cached = (Instant, Arc<KvSettings>);

/// Decoded settings kept in memory for a while (upstream's 60 s `getSystemSettings` cache).
#[derive(Clone)]
pub struct SettingsCache {
    pool: PgPool,
    cipher: Arc<Cipher>,
    ttl: Duration,
    slot: Arc<Mutex<Option<Cached>>>,
}

impl SettingsCache {
    pub fn new(pool: PgPool, cipher: Arc<Cipher>) -> Self {
        Self {
            pool,
            cipher,
            ttl: Duration::from_secs(60),
            slot: Arc::default(),
        }
    }

    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    fn fresh(&self) -> Option<Arc<KvSettings>> {
        let slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        slot.as_ref()
            .filter(|(at, _)| at.elapsed() < self.ttl)
            .map(|(_, s)| s.clone())
    }

    pub async fn get(&self) -> sqlx::Result<Arc<KvSettings>> {
        if let Some(settings) = self.fresh() {
            return Ok(settings);
        }
        let settings = Arc::new(load(&self.pool, &self.cipher).await?);
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) =
            Some((Instant::now(), settings.clone()));
        Ok(settings)
    }

    /// Makes `s` the cached value at once.
    pub fn replace(&self, s: KvSettings) {
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), Arc::new(s)));
    }
}
