//! Session stores for `tower-sessions` (ADR 0012).
//!
//! [`PgSessionStore`] keeps records in the `sessions` table through
//! `db::sessions`; only the random id travels in the cookie. A record is
//! stored as JSON bytes with its expiry, and expired rows are never loaded.
//! [`SessionStoreHandle`] lets the router take any store (tests pass an
//! in-memory one).

use std::fmt;
use std::sync::Arc;

use app::Clock;
use async_trait::async_trait;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, SessionStore};

/// How many fresh ids `create` tries before giving up (a collision of
/// 128-bit random ids is practically impossible; this only bounds the loop).
const CREATE_ATTEMPTS: usize = 4;

/// Sessions in Postgres.
#[derive(Clone)]
pub struct PgSessionStore {
    pool: db::PgPool,
    clock: Arc<dyn Clock>,
}

impl fmt::Debug for PgSessionStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PgSessionStore").finish_non_exhaustive()
    }
}

impl PgSessionStore {
    /// Sessions in `pool`; `clock` decides what has expired.
    #[must_use]
    pub fn new(pool: db::PgPool, clock: Arc<dyn Clock>) -> Self {
        Self { pool, clock }
    }

    /// Deletes expired sessions; returns how many.
    ///
    /// # Errors
    /// [`session_store::Error::Backend`] without detail if the database
    /// cannot be written.
    pub async fn delete_expired(&self) -> session_store::Result<u64> {
        db::sessions::delete_expired(&self.pool, self.clock.now().0)
            .await
            .map_err(|_| backend())
    }
}

fn backend() -> session_store::Error {
    session_store::Error::Backend("session storage unavailable".to_owned())
}

fn encode(record: &Record) -> session_store::Result<(Vec<u8>, i64)> {
    let data =
        serde_json::to_vec(record).map_err(|e| session_store::Error::Encode(e.to_string()))?;
    let expires_ms = i64::try_from(record.expiry_date.unix_timestamp_nanos() / 1_000_000)
        .map_err(|_| session_store::Error::Encode("expiry out of range".to_owned()))?;
    Ok((data, expires_ms))
}

#[async_trait]
impl SessionStore for PgSessionStore {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        for _ in 0..CREATE_ATTEMPTS {
            let (data, expires_ms) = encode(record)?;
            let id = record.id.to_string();
            if db::sessions::insert_new(&self.pool, &id, &data, expires_ms)
                .await
                .map_err(|_| backend())?
            {
                return Ok(());
            }
            record.id = Id::default();
        }
        Err(backend())
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        let (data, expires_ms) = encode(record)?;
        db::sessions::save(&self.pool, &record.id.to_string(), &data, expires_ms)
            .await
            .map_err(|_| backend())
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        let id = session_id.to_string();
        let Some(bytes) = db::sessions::load(&self.pool, &id, self.clock.now().0)
            .await
            .map_err(|_| backend())?
        else {
            return Ok(None);
        };
        let record: Record = serde_json::from_slice(&bytes)
            .map_err(|e| session_store::Error::Decode(e.to_string()))?;
        // A record stored under another id is never handed out.
        Ok((record.id == *session_id).then_some(record))
    }

    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        db::sessions::delete(&self.pool, &session_id.to_string())
            .await
            .map_err(|_| backend())
    }
}

/// Any [`SessionStore`] behind an `Arc`, so the router is not generic over
/// the store.
#[derive(Clone)]
pub struct SessionStoreHandle(Arc<dyn SessionStore>);

impl fmt::Debug for SessionStoreHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("SessionStoreHandle").field(&self.0).finish()
    }
}

impl SessionStoreHandle {
    /// Wraps `store`.
    #[must_use]
    pub fn new(store: impl SessionStore) -> Self {
        Self(Arc::new(store))
    }
}

#[async_trait]
impl SessionStore for SessionStoreHandle {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        self.0.create(record).await
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        self.0.save(record).await
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        self.0.load(session_id).await
    }

    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        self.0.delete(session_id).await
    }
}
