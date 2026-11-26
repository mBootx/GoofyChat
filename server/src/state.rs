use proto::{CallSignal, ServerEvent, User};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, Semaphore};

#[derive(Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub max_upload: u64,
    pub user_quota: u64,
    pub total_quota: u64,
    /// Host name of the TURN server, empty to disable.
    pub turn_host: String,
    pub turn_secret: String,
}

impl Config {
    pub fn from_env() -> Self {
        let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.into());
        let mb = |k: &str, d: &str| var(k, d).parse::<u64>().expect(k) * 1024 * 1024;
        Config {
            data_dir: var("DATA_DIR", "/data").into(),
            max_upload: mb("MAX_UPLOAD_MB", "100"),
            user_quota: mb("USER_QUOTA_MB", "2048"),
            total_quota: mb("TOTAL_QUOTA_MB", "51200"),
            turn_host: var("TURN_HOST", ""),
            turn_secret: var("TURN_SECRET", ""),
        }
    }
}

pub struct PendingCall {
    pub from: User,
    pub to: i64,
    pub signal: CallSignal,
    pub at: Instant,
}

pub const RING_TIMEOUT: Duration = Duration::from_secs(45);

type Sockets = HashMap<i64, Vec<mpsc::Sender<ServerEvent>>>;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub config: Arc<Config>,
    pub sockets: Arc<Mutex<Sockets>>,
    /// Ringing calls, replayed to a device that connects while ringing.
    pub calls: Arc<Mutex<HashMap<String, PendingCall>>>,
    hits: Arc<Mutex<HashMap<(String, &'static str), Vec<Instant>>>>,
    /// Bounds the memory used by concurrent password hashing.
    pub hashing: Arc<Semaphore>,
    /// Users with an upload in progress: one at a time keeps quotas exact.
    pub uploading: Arc<Mutex<HashSet<i64>>>,
}

impl AppState {
    pub async fn new(config: Config) -> Result<Self, Box<dyn std::error::Error>> {
        tokio::fs::create_dir_all(config.data_dir.join("media")).await?;
        let options = SqliteConnectOptions::new()
            .filename(config.data_dir.join("chat.db"))
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let db = SqlitePoolOptions::new().max_connections(8).connect_with(options).await?;
        sqlx::raw_sql(include_str!("../schema.sql")).execute(&db).await?;
        Ok(AppState {
            db,
            config: Arc::new(config),
            sockets: Default::default(),
            calls: Default::default(),
            hits: Default::default(),
            hashing: Arc::new(Semaphore::new(4)),
            uploading: Default::default(),
        })
    }

    /// Sliding-window rate limit. Returns false once `max` is reached.
    pub fn allow(&self, key: String, bucket: &'static str, max: usize, window: Duration) -> bool {
        let mut hits = self.hits.lock().unwrap();
        if hits.len() > 10_000 {
            hits.retain(|_, v| v.iter().any(|t| t.elapsed() < Duration::from_secs(86400)));
        }
        let list = hits.entry((key, bucket)).or_default();
        list.retain(|t| t.elapsed() < window);
        if list.len() >= max {
            return false;
        }
        list.push(Instant::now());
        true
    }

    /// Delivers an event to every connected device of the given users.
    pub fn send(&self, users: &[i64], event: &ServerEvent) {
        let mut sockets = self.sockets.lock().unwrap();
        for id in users {
            if let Some(list) = sockets.get_mut(id) {
                // A full queue means a stalled client: drop it, it will reconnect.
                list.retain(|tx| tx.try_send(event.clone()).is_ok());
            }
        }
    }
}
