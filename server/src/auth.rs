use crate::{
    error::{bad, internal, unauthorized, Error, Result},
    state::AppState,
};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{FromRequestParts, State},
    http::{header, request::Parts, HeaderMap, StatusCode},
    Json,
};
use proto::{Login, Register, Session, User};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::{
    net::IpAddr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as i64
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn token_hash(token: &str) -> String {
    Sha256::digest(token.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Client address as set by the reverse proxy. The port is not exposed publicly.
fn client_ip(headers: &HeaderMap) -> String {
    let ip = headers
        .get("x-real-ip")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<IpAddr>().ok());
    match ip {
        Some(IpAddr::V4(v4)) => v4.to_string(),
        // One subscriber usually owns a whole /64.
        Some(IpAddr::V6(v6)) => format!("{:?}", &v6.segments()[..4]),
        None => "local".into(),
    }
}

/// Sessions older than this must sign in again.
pub const SESSION_MS: i64 = 90 * 24 * 3600 * 1000;

fn too_many() -> Error {
    Error(StatusCode::TOO_MANY_REQUESTS, "Trop de tentatives, réessayez plus tard.".into())
}

#[derive(sqlx::FromRow)]
pub struct UserRow {
    id: i64,
    username: String,
    display_name: String,
    avatar: Option<String>,
}

impl From<UserRow> for User {
    fn from(r: UserRow) -> Self {
        User { id: r.id, username: r.username, display_name: r.display_name, avatar: r.avatar }
    }
}

pub async fn user_for_token(state: &AppState, token: &str) -> Result<User> {
    let row: Option<UserRow> = sqlx::query_as(
        "SELECT u.id, u.username, u.display_name, u.avatar FROM sessions s
         JOIN users u ON u.id = s.user_id WHERE s.token_hash = ? AND s.created_at > ?",
    )
    .bind(token_hash(token))
    .bind(now_ms() - SESSION_MS)
    .fetch_optional(&state.db)
    .await?;
    row.map(Into::into).ok_or_else(unauthorized)
}

pub async fn check_password(state: &AppState, password: String, hash: String) -> Result<bool> {
    let _slot = state.hashing.acquire().await.map_err(internal)?;
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash)
            .map(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
            .unwrap_or(false)
    })
    .await
    .map_err(internal)
}

fn bearer(headers: &HeaderMap) -> Result<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(unauthorized)
}

/// Extractor for the authenticated user.
pub struct Auth(pub User);

impl FromRequestParts<AppState> for Auth {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        Ok(Auth(user_for_token(state, bearer(&parts.headers)?).await?))
    }
}

async fn new_session(state: &AppState, user: User) -> Result<Json<Session>> {
    let token = random_hex(32);
    sqlx::query("INSERT INTO sessions (token_hash, user_id, created_at) VALUES (?, ?, ?)")
        .bind(token_hash(&token))
        .bind(user.id)
        .bind(now_ms())
        .execute(&state.db)
        .await?;
    Ok(Json(Session { token, user }))
}

pub async fn register(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Register>,
) -> Result<Json<Session>> {
    let username = input.username.trim().to_lowercase();
    let display_name = input.display_name.trim().to_string();
    if !(3..=24).contains(&username.len())
        || !username.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
    {
        return Err(bad("Identifiant : 3 à 24 caractères (lettres, chiffres, _ ou .)."));
    }
    if !(1..=40).contains(&display_name.chars().count()) {
        return Err(bad("Nom affiché : 1 à 40 caractères."));
    }
    if !(8..=128).contains(&input.password.len()) {
        return Err(bad("Mot de passe : 8 à 128 caractères."));
    }
    if !state.allow(client_ip(&headers), "register", 5, Duration::from_secs(3600)) {
        return Err(too_many());
    }
    // Checked after the rate limit so the code cannot be guessed quickly.
    let invite = &state.config.invite_code;
    if !invite.is_empty() && input.invite.trim() != invite {
        return Err(Error(StatusCode::FORBIDDEN, "Code d'invitation incorrect.".into()));
    }
    let password = input.password;
    let _slot = state.hashing.acquire().await.map_err(internal)?;
    let hash = tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
    })
    .await
    .map_err(internal)?
    .map_err(internal)?;
    let inserted = sqlx::query(
        "INSERT INTO users (username, display_name, password_hash, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(&username)
    .bind(&display_name)
    .bind(hash)
    .bind(now_ms())
    .execute(&state.db)
    .await;
    let id = match inserted {
        Ok(r) => r.last_insert_rowid(),
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
            return Err(Error(StatusCode::CONFLICT, "Cet identifiant est déjà pris.".into()))
        }
        Err(e) => return Err(e.into()),
    };
    new_session(&state, User { id, username, display_name, avatar: None }).await
}

pub async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Login>,
) -> Result<Json<Session>> {
    let username = input.username.trim().to_lowercase();
    let window = Duration::from_secs(300);
    if username.len() > 24
        || input.password.len() > 128
        || !state.allow(client_ip(&headers), "login", 20, window)
        || !state.allow(username.clone(), "login-user", 15, window)
    {
        return Err(too_many());
    }
    let wrong = || Error(StatusCode::UNAUTHORIZED, "Identifiant ou mot de passe incorrect.".into());
    let row: Option<(i64, String, String, Option<String>)> =
        sqlx::query_as("SELECT id, display_name, password_hash, avatar FROM users WHERE username = ?")
            .bind(&username)
            .fetch_optional(&state.db)
            .await?;
    let (id, display_name, hash, avatar) = row.ok_or_else(wrong)?;
    if !check_password(&state, input.password, hash).await? {
        return Err(wrong());
    }
    new_session(&state, User { id, username, display_name, avatar }).await
}

pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<StatusCode> {
    sqlx::query("DELETE FROM sessions WHERE token_hash = ?")
        .bind(token_hash(bearer(&headers)?))
        .execute(&state.db)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
