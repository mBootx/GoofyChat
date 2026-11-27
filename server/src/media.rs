use crate::{
    auth::{now_ms, random_hex, Auth},
    error::{bad, not_found, Error, Result},
    state::AppState,
};
use axum::{
    extract::{Multipart, Path, Request, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use proto::Media;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tower::ServiceExt;
use tower_http::services::ServeFile;

/// Only passive media types: nothing a browser would execute (no SVG, no HTML).
pub fn allowed_mime(mime: &str) -> bool {
    const ALLOWED: &[&str] = &[
        "image/jpeg", "image/png", "image/gif", "image/webp", "image/heic", "image/heif",
        "video/mp4", "video/webm", "video/quicktime", "video/3gpp", "video/x-matroska",
        "audio/mpeg", "audio/mp4", "audio/aac", "audio/ogg", "audio/webm", "audio/wav",
        "audio/x-wav", "audio/flac", "audio/amr", "audio/x-m4a",
    ];
    ALLOWED.contains(&mime)
}

pub fn too_large(msg: &str) -> Error {
    Error(StatusCode::PAYLOAD_TOO_LARGE, msg.into())
}

pub async fn upload(
    State(state): State<AppState>,
    Auth(user): Auth,
    mut multipart: Multipart,
) -> Result<Json<Media>> {
    if !state.allow(user.id.to_string(), "upload", 60, Duration::from_secs(600)) {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "Trop d'envois, patientez.".into()));
    }
    let _busy = Busy::claim(&state, user.id)?;
    let mut field = multipart
        .next_field()
        .await
        .map_err(|_| bad("Envoi invalide."))?
        .ok_or_else(|| bad("Fichier manquant."))?;
    // Drop codec parameters, e.g. "audio/webm;codecs=opus".
    let mime = field
        .content_type()
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if !allowed_mime(&mime) {
        return Err(bad("Seuls les photos, vidéos et fichiers audio sont acceptés."));
    }
    let name: String = field.file_name().unwrap_or("fichier").chars().take(120).collect();

    let remaining = remaining(&state, user.id).await?;

    let id = random_hex(24);
    let path = state.config.data_dir.join("media").join(&id);
    // Removes the file on any failure, including a dropped connection.
    let mut guard = Unfinished(Some(path.clone()));
    let mut file = tokio::fs::File::create(&path).await?;
    let mut size = 0u64;
    async {
        while let Some(chunk) = field.chunk().await.map_err(|_| bad("Envoi interrompu."))? {
            size += chunk.len() as u64;
            if size > state.config.max_upload {
                return Err(too_large("Fichier trop volumineux."));
            }
            if size > remaining {
                return Err(too_large("Espace de stockage épuisé."));
            }
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        if size == 0 {
            return Err(bad("Fichier vide."));
        }
        sqlx::query("INSERT INTO media (id, owner_id, mime, name, size, created_at) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(&id)
            .bind(user.id)
            .bind(&mime)
            .bind(&name)
            .bind(size as i64)
            .bind(now_ms())
            .execute(&state.db)
            .await?;
        Ok::<(), Error>(())
    }
    .await?;
    guard.0 = None;
    Ok(Json(Media { id, mime, name, size: size as i64 }))
}

/// Bytes the user may still store: the smaller of their quota and the server's.
pub async fn remaining(state: &AppState, user: i64) -> Result<u64> {
    let (mine, total): (i64, i64) = sqlx::query_as(
        "SELECT COALESCE(SUM(CASE WHEN owner_id = ? THEN size END), 0), COALESCE(SUM(size), 0) FROM media",
    )
    .bind(user)
    .fetch_one(&state.db)
    .await?;
    Ok((state.config.user_quota.saturating_sub(mine as u64))
        .min(state.config.total_quota.saturating_sub(total as u64)))
}

pub struct Busy(AppState, i64);

impl Busy {
    pub fn claim(state: &AppState, user: i64) -> Result<Self> {
        if !state.uploading.lock().unwrap().insert(user) {
            return Err(Error(StatusCode::TOO_MANY_REQUESTS, "Un envoi est déjà en cours.".into()));
        }
        Ok(Busy(state.clone(), user))
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        self.0.uploading.lock().unwrap().remove(&self.1);
    }
}

pub struct Unfinished(pub Option<std::path::PathBuf>);

impl Drop for Unfinished {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// SQL condition: the media row `{id}` is used by nothing (messages, avatars, avatar
/// stills, banners, story files and posters).
fn unused(id: &str) -> String {
    format!(
        "NOT EXISTS (SELECT 1 FROM messages WHERE media_id = {id})
         AND NOT EXISTS (SELECT 1 FROM users WHERE avatar = {id})
         AND NOT EXISTS (SELECT 1 FROM users WHERE avatar_still = {id})
         AND NOT EXISTS (SELECT 1 FROM users WHERE banner = {id})
         AND NOT EXISTS (SELECT 1 FROM stories WHERE media_id = {id})
         AND NOT EXISTS (SELECT 1 FROM stories WHERE poster_id = {id})"
    )
}

/// Whether anything uses this media row.
pub async fn in_use(state: &AppState, id: &str) -> Result<bool> {
    let free = sqlx::query(&format!("SELECT 1 WHERE {}", unused("?1")))
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
    Ok(free.is_none())
}

/// Reads a whole media file (only for small files: images).
pub async fn read(state: &AppState, id: &str) -> Result<Vec<u8>> {
    Ok(tokio::fs::read(state.config.data_dir.join("media").join(id)).await?)
}

/// Stores bytes made by the server as a new media row of `owner`; returns its id.
pub async fn store(state: &AppState, owner: i64, mime: &str, name: &str, bytes: &[u8]) -> Result<String> {
    let id = random_hex(24);
    let path = state.config.data_dir.join("media").join(&id);
    let mut guard = Unfinished(Some(path.clone()));
    tokio::fs::write(&path, bytes).await?;
    sqlx::query("INSERT INTO media (id, owner_id, mime, name, size, created_at) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(&id)
        .bind(owner)
        .bind(mime)
        .bind(name)
        .bind(bytes.len() as i64)
        .bind(now_ms())
        .execute(&state.db)
        .await?;
    guard.0 = None;
    Ok(id)
}

/// Deletes expired sessions, failed stories, uploads never used, and stray files.
pub async fn cleanup(state: &AppState) -> Result<()> {
    let day = 24 * 3600 * 1000;
    sqlx::query("DELETE FROM sessions WHERE created_at <= ?")
        .bind(now_ms() - crate::auth::SESSION_MS)
        .execute(&state.db)
        .await?;
    crate::stories::cleanup_failed(state, now_ms()).await?;
    sqlx::query(&format!("DELETE FROM media WHERE created_at < ?1 AND {}", unused("media.id")))
    .bind(now_ms() - day)
    .execute(&state.db)
    .await?;
    let mut files = tokio::fs::read_dir(state.config.data_dir.join("media")).await?;
    while let Some(file) = files.next_entry().await? {
        // Recent files may still be uploading.
        let age = file.metadata().await?.modified()?.elapsed().unwrap_or_default();
        if age < Duration::from_secs(3600) {
            continue;
        }
        let id = file.file_name().to_string_lossy().into_owned();
        let known = sqlx::query("SELECT 1 FROM media WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.db)
            .await?;
        if known.is_none() {
            tokio::fs::remove_file(file.path()).await?;
        }
    }
    Ok(())
}

/// Removes the file of an upload that nothing uses any more.
pub async fn discard(state: &AppState, id: &str) -> Result<()> {
    let removed = sqlx::query(&format!("DELETE FROM media WHERE id = ?1 AND {}", unused("?1")))
    .bind(id)
    .execute(&state.db)
    .await?;
    if removed.rows_affected() > 0 {
        remove_file(state, id).await;
    }
    Ok(())
}

/// `id` must come from the database. A failure is left to the next cleanup.
pub async fn remove_file(state: &AppState, id: &str) {
    if let Err(e) = tokio::fs::remove_file(state.config.data_dir.join("media").join(id)).await {
        tracing::warn!("remove media: {e}");
    }
}

/// Serves a file with Range support so video and audio can seek.
pub async fn download(
    State(state): State<AppState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Response> {
    serve(&state, &id, request, "private, max-age=31536000").await
}

/// Serves a media row's file with its mime, Range support and `nosniff`.
pub async fn serve(state: &AppState, id: &str, request: Request, cache: &'static str) -> Result<Response> {
    let mime: Option<(String,)> = sqlx::query_as("SELECT mime FROM media WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await?;
    let (mime,) = mime.ok_or_else(not_found)?;
    // The id was found in the database, so it is a hex string we generated.
    let path = state.config.data_dir.join("media").join(id);
    let mut response = ServeFile::new(path)
        .oneshot(request)
        .await
        .map_err(crate::error::internal)?
        .into_response();
    let headers = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&mime) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    Ok(response)
}
