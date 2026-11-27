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
    /// Required to create an account, empty for open sign-up.
    pub invite_code: String,
    /// goofysound (Navidrome) base URL, empty to disable music.
    pub navidrome_url: String,
    /// FileBrowser Quantum base URL, empty to disable photos.
    pub filebrowser_url: String,
    /// Camera-backup folder; `{user}` is the FileBrowser account name.
    pub photos_folder: String,
    /// Shared secret with the stories worker; empty disables video stories and archive
    /// compression (and the internal listener).
    pub worker_token: String,
    /// Address of the internal worker API.
    pub worker_bind: String,
}

pub const DEFAULT_PHOTOS_FOLDER: &str = "/backups/photos/{user}";

impl Config {
    pub fn from_env() -> Self {
        let var = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.into());
        let mb = |k: &str, d: &str| var(k, d).parse::<u64>().expect(k) * 1024 * 1024;
        let base = |k: &str| var(k, "").trim().trim_end_matches('/').to_string();
        let folder = var("PHOTOS_FOLDER", "");
        Config {
            data_dir: var("DATA_DIR", "/data").into(),
            max_upload: mb("MAX_UPLOAD_MB", "100"),
            user_quota: mb("USER_QUOTA_MB", "5120"),
            total_quota: mb("TOTAL_QUOTA_MB", "51200"),
            turn_host: var("TURN_HOST", ""),
            turn_secret: var("TURN_SECRET", ""),
            invite_code: var("INVITE_CODE", "").trim().to_string(),
            navidrome_url: base("NAVIDROME_URL"),
            filebrowser_url: base("FILEBROWSER_URL"),
            photos_folder: if folder.trim().is_empty() {
                DEFAULT_PHOTOS_FOLDER.into()
            } else {
                folder.trim().trim_end_matches('/').to_string()
            },
            worker_token: var("WORKER_TOKEN", "").trim().to_string(),
            worker_bind: Some(var("WORKER_BIND", "")).filter(|b| !b.trim().is_empty())
                .unwrap_or_else(|| "0.0.0.0:8081".into()),
        }
    }
}

/// Reads the server secret, creating it on first start. It signs `k=` keys and
/// encrypts the stored FileBrowser passwords, so it must stay in the data volume.
fn load_secret(dir: &std::path::Path) -> std::io::Result<[u8; 32]> {
    use std::io::{Read, Write};
    let path = dir.join("secret.key");
    let mut key = [0u8; 32];
    match std::fs::File::open(&path) {
        Ok(mut file) => {
            file.read_exact(&mut key)?;
            return Ok(key);
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut key);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&path)?;
    file.write_all(&key)?;
    file.sync_all()?;
    Ok(key)
}

/// A FileBrowser login kept in memory.
#[derive(Clone)]
pub struct FileSession {
    pub jwt: String,
    pub source: String,
    /// Photos root of this account, `{user}` resolved.
    pub root: String,
}

/// Adds the columns missing from a database created by an earlier version, then runs
/// the one-time data migrations not yet recorded in `meta`.
pub async fn migrate(db: &SqlitePool) -> Result<(), sqlx::Error> {
    const COLUMNS: &[(&str, &str, &str)] = &[
        ("users", "avatar", "TEXT"),
        ("users", "bio", "TEXT NOT NULL DEFAULT ''"),
        ("users", "banner", "TEXT"),
        ("users", "avatar_still", "TEXT"),
        ("users", "name_font", "TEXT NOT NULL DEFAULT ''"),
        ("users", "name_color", "TEXT NOT NULL DEFAULT ''"),
        ("messages", "edited_at", "INTEGER"),
        ("messages", "reply_to", "INTEGER"),
        ("messages", "deleted", "INTEGER NOT NULL DEFAULT 0"),
        ("messages", "track_id", "TEXT"),
        ("messages", "track_title", "TEXT"),
        ("messages", "track_artist", "TEXT"),
        ("messages", "track_album", "TEXT"),
        ("messages", "track_duration", "INTEGER"),
        ("messages", "track_cover", "INTEGER"),
    ];
    for (table, column, kind) in COLUMNS {
        let found = sqlx::query("SELECT 1 FROM pragma_table_info(?) WHERE name = ?")
            .bind(table)
            .bind(column)
            .fetch_optional(db)
            .await?;
        if found.is_none() {
            sqlx::raw_sql(&format!("ALTER TABLE {table} ADD COLUMN {column} {kind}")).execute(db).await?;
        }
    }
    // Indexes on added columns can only exist once the columns do.
    sqlx::raw_sql("CREATE INDEX IF NOT EXISTS idx_messages_track ON messages(track_id)")
        .execute(db)
        .await?;

    // 0.4.0: people who already share a one-to-one conversation become friends, unless
    // one blocked the other. Pairs who only share a group do not.
    let mut tx = db.begin().await?;
    let done = sqlx::query("SELECT 1 FROM meta WHERE key = 'friends_from_direct'")
        .fetch_optional(&mut *tx)
        .await?;
    if done.is_none() {
        sqlx::query(
            "INSERT OR IGNORE INTO friendships (user_a, user_b, created_at)
             SELECT a.user_id, b.user_id, c.created_at
             FROM conversations c
             JOIN members a ON a.conversation_id = c.id
             JOIN members b ON b.conversation_id = c.id AND b.user_id > a.user_id
             WHERE c.is_group = 0
               AND NOT EXISTS (SELECT 1 FROM blocks
                               WHERE (user_id = a.user_id AND blocked_id = b.user_id)
                                  OR (user_id = b.user_id AND blocked_id = a.user_id))",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO meta (key, value) VALUES ('friends_from_direct', ?)")
            .bind(crate::auth::now_ms().to_string())
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Who is online, in memory. A user is online while at least one of their sockets is
/// active; going offline is announced after a grace delay (see `presence.rs`).
#[derive(Default)]
pub struct Presence {
    /// Active sockets per user.
    pub active: HashMap<i64, usize>,
    /// Users whose friends were told they are online.
    pub online: HashSet<i64>,
    /// Bumped each time a user's last active socket goes, so stale offline timers do nothing.
    pub epoch: HashMap<i64, u64>,
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
    /// 32 random bytes from `DATA_DIR/secret.key`.
    pub secret: Arc<[u8; 32]>,
    /// For API calls: short timeout.
    pub http: reqwest::Client,
    /// For streamed bodies (songs, imports): no total timeout.
    pub streaming: reqwest::Client,
    /// FileBrowser logins by Goofy Chat user.
    pub files: Arc<Mutex<HashMap<i64, FileSession>>>,
    /// Who listens to what, from the last `getNowPlaying` poll.
    pub activity: Arc<Mutex<HashMap<i64, proto::Activity>>>,
    /// Photo month folders by user, briefly cached between pages.
    pub months: Arc<Mutex<HashMap<i64, (Instant, Arc<Vec<crate::photos::Month>>)>>>,
    /// Account used by the next `getNowPlaying` poll.
    pub poller: Arc<Mutex<usize>>,
    /// Online state of users (see `presence.rs`).
    pub presence: Arc<Mutex<Presence>>,
    /// How long a user stays online after their last active socket went.
    pub offline_grace: Duration,
    /// Image decoding (avatars, banners, story photos): one at a time bounds the memory.
    pub imaging: Arc<Semaphore>,
}

impl AppState {
    pub async fn new(config: Config) -> Result<Self, Box<dyn std::error::Error>> {
        tokio::fs::create_dir_all(config.data_dir.join("media")).await?;
        let secret = load_secret(&config.data_dir)?;
        let agent = concat!("GoofyChat/", env!("CARGO_PKG_VERSION"));
        let http = reqwest::Client::builder()
            .user_agent(agent)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let streaming = reqwest::Client::builder()
            .user_agent(agent)
            .connect_timeout(Duration::from_secs(5))
            .read_timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let options = SqliteConnectOptions::new()
            .filename(config.data_dir.join("chat.db"))
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let db = SqlitePoolOptions::new().max_connections(8).connect_with(options).await?;
        sqlx::raw_sql(include_str!("../schema.sql")).execute(&db).await?;
        migrate(&db).await?;
        Ok(AppState {
            db,
            config: Arc::new(config),
            sockets: Default::default(),
            calls: Default::default(),
            hits: Default::default(),
            hashing: Arc::new(Semaphore::new(4)),
            uploading: Default::default(),
            secret: Arc::new(secret),
            http,
            streaming,
            files: Default::default(),
            activity: Default::default(),
            months: Default::default(),
            poller: Default::default(),
            presence: Default::default(),
            offline_grace: Duration::from_secs(15),
            imaging: Arc::new(Semaphore::new(1)),
        })
    }

    /// Whether video stories and archive compression are available.
    pub fn has_worker(&self) -> bool {
        !self.config.worker_token.is_empty()
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
