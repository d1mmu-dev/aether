//! What the podcast machinery needs besides a single call's own arguments: the database, the
//! one outbound HTTP client, and where aether keeps its files on disk.

use std::path::PathBuf;
use std::sync::Arc;

use aether_db::PgPool;
use aether_net::Net;

#[derive(Clone)]
pub struct PodcastCtx {
    pub pool: PgPool,
    pub net: Arc<Net>,
    /// Where aether keeps its own files; the episode cache lives at
    /// [`crate::cache::cache_dir`] under it.
    pub data_dir: PathBuf,
}
