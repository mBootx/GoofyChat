//! Links to SelfHost-hub accounts (goofysound and photos), and the uses of the
//! server secret: signed `k=` keys and encrypted FileBrowser passwords.

use crate::{
    auth::{now_ms, random_hex, Auth},
    error::{bad, internal, Error, Result},
    music, photos,
    state::AppState,
};
use axum::{extract::State, http::StatusCode, Json};
use base64::Engine;
use chacha20poly1305::{
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
    ChaCha20Poly1305, Key, Nonce,
};
use hmac::{Hmac, Mac};
use proto::{LinkAccount, Links};
use sha2::Sha256;
use std::time::Duration;

type HmacSha256 = Hmac<Sha256>;

/// A key derived from the server secret, one per purpose.
fn subkey(state: &AppState, purpose: &str) -> [u8; 32] {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(&state.secret[..]).expect("any key length");
    mac.update(purpose.as_bytes());
    mac.finalize().into_bytes().into()
}

fn signer(state: &AppState, message: &str) -> HmacSha256 {
    let mut mac =
        <HmacSha256 as Mac>::new_from_slice(&subkey(state, "goofychat:sign:v1")).expect("any key length");
    mac.update(message.as_bytes());
    mac
}

/// Hex HMAC-SHA256 of the message.
pub fn sign(state: &AppState, message: &str) -> String {
    signer(state, message).finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time check of a key made by [`sign`].
pub fn verify(state: &AppState, message: &str, key: &str) -> bool {
    if key.len() != 64 || !key.is_ascii() {
        return false;
    }
    let bytes: Option<Vec<u8>> = (0..32).map(|i| u8::from_str_radix(&key[2 * i..2 * i + 2], 16).ok()).collect();
    match bytes {
        Some(bytes) => signer(state, message).verify_slice(&bytes).is_ok(),
        None => false,
    }
}

fn cipher(state: &AppState) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(Key::from_slice(&subkey(state, "goofychat:encrypt:v1")))
}

/// base64(nonce || ciphertext), bound to the user so a row cannot be moved.
pub fn encrypt(state: &AppState, user: i64, plain: &str) -> Result<String> {
    let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
    let aad = user.to_be_bytes();
    let sealed = cipher(state)
        .encrypt(&nonce, Payload { msg: plain.as_bytes(), aad: &aad })
        .map_err(|_| internal("encryption failed"))?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&sealed);
    Ok(base64::engine::general_purpose::STANDARD.encode(out))
}

pub fn decrypt(state: &AppState, user: i64, sealed: &str) -> Option<String> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(sealed).ok()?;
    if bytes.len() < 12 {
        return None;
    }
    let (nonce, data) = bytes.split_at(12);
    let aad = user.to_be_bytes();
    let plain = cipher(state).decrypt(Nonce::from_slice(nonce), Payload { msg: data, aad: &aad }).ok()?;
    String::from_utf8(plain).ok()
}

pub fn require_music(state: &AppState) -> Result<()> {
    if state.config.navidrome_url.is_empty() {
        return Err(Error(StatusCode::SERVICE_UNAVAILABLE, "goofysound n'est pas disponible sur ce serveur.".into()));
    }
    Ok(())
}

pub fn require_photos(state: &AppState) -> Result<()> {
    if state.config.filebrowser_url.is_empty() {
        return Err(Error(StatusCode::SERVICE_UNAVAILABLE, "Les photos ne sont pas disponibles sur ce serveur.".into()));
    }
    Ok(())
}

pub async fn links_of(state: &AppState, user: i64) -> Result<Links> {
    let navidrome: Option<(String,)> = sqlx::query_as("SELECT username FROM navidrome_links WHERE user_id = ?")
        .bind(user)
        .fetch_optional(&state.db)
        .await?;
    let photos: Option<(String,)> = sqlx::query_as("SELECT username FROM photo_links WHERE user_id = ?")
        .bind(user)
        .fetch_optional(&state.db)
        .await?;
    Ok(Links { navidrome: navidrome.map(|r| r.0), photos: photos.map(|r| r.0) })
}

pub async fn get_links(State(state): State<AppState>, Auth(user): Auth) -> Result<Json<Links>> {
    Ok(Json(links_of(&state, user.id).await?))
}

/// Checks sizes and the attempt budget shared by both services.
fn check_attempt(state: &AppState, user: i64, input: &LinkAccount) -> Result<(String, String)> {
    let username = input.username.trim().to_string();
    if username.is_empty() || username.chars().count() > 64 || username.chars().any(char::is_control) {
        return Err(bad("Identifiant invalide."));
    }
    if input.password.is_empty() || input.password.len() > 256 {
        return Err(bad("Mot de passe invalide."));
    }
    if !state.allow(user.to_string(), "link", 10, Duration::from_secs(3600)) {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "Trop de tentatives, réessayez plus tard.".into()));
    }
    Ok((username, input.password.clone()))
}

pub async fn link_navidrome(
    State(state): State<AppState>,
    Auth(user): Auth,
    Json(input): Json<LinkAccount>,
) -> Result<Json<Links>> {
    require_music(&state)?;
    let (username, password) = check_attempt(&state, user.id, &input)?;
    let creds = music::Creds::from_password(username, &password, random_hex(6));
    drop(password);
    music::ping(&state, &creds).await?;
    let key = creds.username.to_lowercase();
    let taken: Option<(i64,)> = sqlx::query_as("SELECT user_id FROM navidrome_links WHERE username_key = ?")
        .bind(&key)
        .fetch_optional(&state.db)
        .await?;
    let conflict = || Error(StatusCode::CONFLICT, "Ce compte goofysound est déjà lié à un autre compte.".into());
    if taken.is_some_and(|(owner,)| owner != user.id) {
        return Err(conflict());
    }
    let previous: Option<(String,)> =
        sqlx::query_as("SELECT username_key FROM navidrome_links WHERE user_id = ?")
            .bind(user.id)
            .fetch_optional(&state.db)
            .await?;
    let saved = sqlx::query(
        "INSERT INTO navidrome_links (user_id, username, username_key, salt, token, created_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(user_id) DO UPDATE SET username = excluded.username,
           username_key = excluded.username_key, salt = excluded.salt, token = excluded.token,
           created_at = excluded.created_at",
    )
    .bind(user.id)
    .bind(&creds.username)
    .bind(&key)
    .bind(&creds.salt)
    .bind(&creds.token)
    .bind(now_ms())
    .execute(&state.db)
    .await;
    match saved {
        Ok(_) => {}
        Err(sqlx::Error::Database(e)) if e.is_unique_violation() => return Err(conflict()),
        Err(e) => return Err(e.into()),
    }
    if previous.is_some_and(|(old,)| old != key) {
        music::forget_activity(&state, user.id).await;
    }
    Ok(Json(links_of(&state, user.id).await?))
}

/// Allowed even when the service is off, so the stored token can always be removed.
pub async fn unlink_navidrome(State(state): State<AppState>, Auth(user): Auth) -> Result<StatusCode> {
    sqlx::query("DELETE FROM navidrome_links WHERE user_id = ?").bind(user.id).execute(&state.db).await?;
    music::forget_activity(&state, user.id).await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn link_photos(
    State(state): State<AppState>,
    Auth(user): Auth,
    Json(input): Json<LinkAccount>,
) -> Result<Json<Links>> {
    require_photos(&state)?;
    let (username, password) = check_attempt(&state, user.id, &input)?;
    let session = photos::login(&state, &username, &password).await?;
    let sealed = encrypt(&state, user.id, &password)?;
    drop(password);
    sqlx::query(
        "INSERT INTO photo_links (user_id, username, password, created_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(user_id) DO UPDATE SET username = excluded.username,
           password = excluded.password, created_at = excluded.created_at",
    )
    .bind(user.id)
    .bind(&username)
    .bind(&sealed)
    .bind(now_ms())
    .execute(&state.db)
    .await?;
    state.months.lock().unwrap().remove(&user.id);
    state.files.lock().unwrap().insert(user.id, session);
    Ok(Json(links_of(&state, user.id).await?))
}

pub async fn unlink_photos(State(state): State<AppState>, Auth(user): Auth) -> Result<StatusCode> {
    sqlx::query("DELETE FROM photo_links WHERE user_id = ?").bind(user.id).execute(&state.db).await?;
    forget_photos(&state, user.id);
    Ok(StatusCode::NO_CONTENT)
}

pub fn forget_photos(state: &AppState, user: i64) {
    state.files.lock().unwrap().remove(&user);
    state.months.lock().unwrap().remove(&user);
}
