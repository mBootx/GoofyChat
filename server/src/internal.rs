//! The internal API of the stories worker (`api::internal_router`, served on
//! `WORKER_BIND` only when `WORKER_TOKEN` is set). The worker claims one job at a time,
//! downloads its input, uploads the outputs and reports the result.

use crate::{
    auth::{now_ms, random_hex},
    error::{bad, not_found, unauthorized, Error, Result},
    media,
    state::AppState,
    stories::{self, StoryRow},
};
use axum::{
    body::Body,
    extract::{FromRequestParts, Path, Query, Request, State},
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use proto::{
    limits,
    worker::{EncodeProfile, JobKind, WorkerFailure, WorkerJob, WorkerResult},
};
use serde::Deserialize;

pub const MAIN_BYTES: i64 = 40 * limits::MB;
pub const POSTER_BYTES: i64 = 2 * limits::MB;
/// A running job not reported within this is handed out again.
pub const CLAIM_TIMEOUT_MS: i64 = 15 * 60 * 1000;
/// Attempts per job; the next claim fails it for good.
pub const MAX_ATTEMPTS: i64 = 3;

/// Constant-time comparison.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Extractor: the request carries `Authorization: Bearer <WORKER_TOKEN>`.
pub struct Worker;

impl FromRequestParts<AppState> for Worker {
    type Rejection = Error;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self> {
        let token = &state.config.worker_token;
        let given = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        if token.is_empty() || !same(given.as_bytes(), token.as_bytes()) {
            return Err(unauthorized());
        }
        Ok(Worker)
    }
}

pub fn profile(kind: JobKind, is_video: bool) -> EncodeProfile {
    match (kind, is_video) {
        (JobKind::Prepare, _) => EncodeProfile {
            max_side: 1280,
            fps: 30,
            crf: 26,
            max_kbps: 2500,
            audio_kbps: 128,
            audio_channels: 2,
            jpeg_q: 4,
            poster_side: 720,
            max_duration_ms: limits::STORY_VIDEO_MS,
        },
        (JobKind::Archive, true) => EncodeProfile {
            max_side: 854,
            fps: 24,
            crf: 32,
            max_kbps: 800,
            audio_kbps: 64,
            audio_channels: 1,
            jpeg_q: 8,
            poster_side: 480,
            max_duration_ms: 0,
        },
        (JobKind::Archive, false) => EncodeProfile {
            max_side: 1080,
            fps: 0,
            crf: 0,
            max_kbps: 0,
            audio_kbps: 0,
            audio_channels: 0,
            jpeg_q: 8,
            poster_side: 0,
            max_duration_ms: 0,
        },
    }
}

fn kind_of(kind: &str) -> JobKind {
    if kind == "archive" {
        JobKind::Archive
    } else {
        JobKind::Prepare
    }
}

#[derive(Deserialize)]
pub struct ClaimQuery {
    /// False while the worker is outside its archive hours: only prepare jobs are handed out.
    #[serde(default = "yes")]
    archive: bool,
}

fn yes() -> bool {
    true
}

/// A job that will not be retried: kept as `failed` for the log. A prepare failure makes
/// its story fail; an archive failure leaves the original as the (uncompressed) archive.
async fn give_up(state: &AppState, job: i64, story: i64, kind: JobKind, error: &str) -> Result<()> {
    tracing::warn!("story job {job} (story {story}) failed for good: {error}");
    sqlx::query("UPDATE story_jobs SET state = 'failed', error = ? WHERE id = ?")
        .bind(error.chars().take(500).collect::<String>())
        .bind(job)
        .execute(&state.db)
        .await?;
    stories::remove_parts(state, job).await;
    if kind == JobKind::Prepare {
        stories::fail_processing(state, story).await?;
    }
    Ok(())
}

/// Hands out the next job, atomically: live-story preparations first, then archives,
/// each oldest first. A running job whose worker went silent is handed out again.
pub async fn claim(
    State(state): State<AppState>,
    _: Worker,
    Query(query): Query<ClaimQuery>,
) -> Result<Response> {
    claim_at(&state, now_ms(), query.archive).await
}

pub async fn claim_at(state: &AppState, now: i64, archive: bool) -> Result<Response> {
    loop {
        // One statement: two claims can never get the same job.
        let claimed: Option<(i64, i64, String, i64)> = sqlx::query_as(
            "UPDATE story_jobs SET state = 'running', claimed_at = ?1, attempts = attempts + 1
             WHERE id = (SELECT id FROM story_jobs
                         WHERE (state = 'queued' OR (state = 'running' AND claimed_at <= ?2))
                           AND (?3 OR kind = 'prepare')
                         ORDER BY CASE kind WHEN 'prepare' THEN 0 ELSE 1 END, id LIMIT 1)
             RETURNING id, story_id, kind, attempts",
        )
        .bind(now)
        .bind(now - CLAIM_TIMEOUT_MS)
        .bind(archive)
        .fetch_optional(&state.db)
        .await?;
        let Some((id, story, kind, attempts)) = claimed else {
            return Ok(StatusCode::NO_CONTENT.into_response());
        };
        let kind = kind_of(&kind);
        if attempts > MAX_ATTEMPTS {
            give_up(state, id, story, kind, "too many attempts").await?;
            continue;
        }
        let row = stories::load(state, story).await?;
        let input = match &row {
            Some(row) => input_of(state, row).await?,
            None => None,
        };
        let Some((_, input_mime)) = input else {
            give_up(state, id, story, kind, "input missing").await?;
            continue;
        };
        let is_video = row.is_some_and(|r| r.is_video);
        let job = WorkerJob { id, kind, is_video, input_mime, profile: profile(kind, is_video) };
        return Ok(Json(job).into_response());
    }
}

/// The story's current main file and its mime.
async fn input_of(state: &AppState, row: &StoryRow) -> Result<Option<(String, String)>> {
    let Some(file) = &row.media_id else { return Ok(None) };
    let mime: Option<(String,)> = sqlx::query_as("SELECT mime FROM media WHERE id = ?")
        .bind(file)
        .fetch_optional(&state.db)
        .await?;
    Ok(mime.map(|(m,)| (file.clone(), m)))
}

/// A running job: its story and kind.
async fn running(state: &AppState, job: i64) -> Result<(StoryRow, JobKind, i64)> {
    let found: Option<(i64, String, i64)> =
        sqlx::query_as("SELECT story_id, kind, attempts FROM story_jobs WHERE id = ? AND state = 'running'")
            .bind(job)
            .fetch_optional(&state.db)
            .await?;
    let (story, kind, attempts) = found.ok_or_else(not_found)?;
    let row = stories::load(state, story).await?.ok_or_else(not_found)?;
    Ok((row, kind_of(&kind), attempts))
}

pub async fn input(
    State(state): State<AppState>,
    _: Worker,
    Path(id): Path<i64>,
    request: Request,
) -> Result<Response> {
    let (row, _, _) = running(&state, id).await?;
    let (file, _) = input_of(&state, &row).await?.ok_or_else(not_found)?;
    media::serve(&state, &file, request, "no-store").await
}

fn part_path(state: &AppState, job: i64, slot: &str) -> std::path::PathBuf {
    state.config.data_dir.join("media").join(format!("job-{job}-{slot}.part"))
}

pub async fn output(
    State(state): State<AppState>,
    _: Worker,
    Path((id, slot)): Path<(i64, String)>,
    headers: HeaderMap,
    body: Body,
) -> Result<StatusCode> {
    let (row, _, _) = running(&state, id).await?;
    let (limit, mime) = match (slot.as_str(), row.is_video) {
        ("main", true) => (MAIN_BYTES, "video/mp4"),
        ("main", false) => (MAIN_BYTES, "image/jpeg"),
        ("poster", true) => (POSTER_BYTES, "image/jpeg"),
        _ => return Err(bad("Emplacement inconnu.")),
    };
    let given = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    if given != mime {
        return Err(bad("Type de fichier inattendu."));
    }
    let bytes = axum::body::to_bytes(body, limit as usize)
        .await
        .map_err(|_| media::too_large("Fichier trop volumineux."))?;
    if bytes.is_empty() {
        return Err(bad("Fichier vide."));
    }
    // Written aside, then renamed: a previous upload of the slot is replaced whole.
    let path = part_path(&state, id, &slot);
    let tmp = path.with_extension(format!("tmp-{}", random_hex(4)));
    let mut guard = media::Unfinished(Some(tmp.clone()));
    tokio::fs::write(&tmp, &bytes).await?;
    tokio::fs::rename(&tmp, &path).await?;
    guard.0 = None;
    Ok(StatusCode::NO_CONTENT)
}

/// Turns an uploaded part into a media row of the author; returns its id.
async fn adopt(state: &AppState, job: i64, slot: &str, owner: i64, mime: &str, name: &str) -> Result<String> {
    let part = part_path(state, job, slot);
    let size = tokio::fs::metadata(&part).await?.len() as i64;
    let id = random_hex(24);
    let path = state.config.data_dir.join("media").join(&id);
    tokio::fs::rename(&part, &path).await?;
    let mut guard = media::Unfinished(Some(path));
    sqlx::query("INSERT INTO media (id, owner_id, mime, name, size, created_at) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(&id)
        .bind(owner)
        .bind(mime)
        .bind(name)
        .bind(size)
        .bind(now_ms())
        .execute(&state.db)
        .await?;
    guard.0 = None;
    Ok(id)
}

pub async fn done(
    State(state): State<AppState>,
    _: Worker,
    Path(id): Path<i64>,
    Json(result): Json<WorkerResult>,
) -> Result<StatusCode> {
    let (row, kind, _) = running(&state, id).await?;
    let has = |slot: &str| part_path(&state, id, slot).exists();
    if !has("main") || (row.is_video && !has("poster")) {
        return Err(bad("Fichier manquant."));
    }
    let (main_mime, ext) = if row.is_video { ("video/mp4", "mp4") } else { ("image/jpeg", "jpg") };
    let main = adopt(&state, id, "main", row.author_id, main_mime, &format!("story-{}.{ext}", row.id)).await?;
    let poster = if row.is_video {
        Some(adopt(&state, id, "poster", row.author_id, "image/jpeg", &format!("story-{}.jpg", row.id)).await?)
    } else {
        None
    };
    let (sql, friends_too) = match kind {
        JobKind::Prepare => (
            "UPDATE stories SET media_id = ?1, poster_id = ?2, width = ?3, height = ?4, duration_ms = ?5,
               state = 'live', expires_at = ?6 WHERE id = ?7 AND state = 'processing'",
            true,
        ),
        JobKind::Archive => (
            "UPDATE stories SET media_id = ?1, poster_id = ?2, width = ?3, height = ?4,
               duration_ms = CASE WHEN is_video = 1 THEN ?5 ELSE duration_ms END, compressed = 1
             WHERE id = ?7 AND state = 'archived' AND ?6 = ?6",
            false,
        ),
    };
    let updated = sqlx::query(sql)
        .bind(&main)
        .bind(&poster)
        .bind(result.width)
        .bind(result.height)
        .bind(result.duration_ms)
        .bind(now_ms() + limits::STORY_LIFETIME_MS)
        .bind(row.id)
        .execute(&state.db)
        .await?
        .rows_affected();
    sqlx::query("DELETE FROM story_jobs WHERE id = ?").bind(id).execute(&state.db).await?;
    // The replaced files, or the new ones when the story moved on meanwhile.
    let stale = if updated > 0 { [row.media_id.clone(), row.poster_id.clone()] } else { [Some(main), poster] };
    for file in stale.into_iter().flatten() {
        media::discard(&state, &file).await?;
    }
    if updated > 0 {
        if let Some(row) = stories::load(&state, row.id).await? {
            stories::announce(&state, &row, friends_too).await?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn fail(
    State(state): State<AppState>,
    _: Worker,
    Path(id): Path<i64>,
    Json(failure): Json<WorkerFailure>,
) -> Result<StatusCode> {
    let (row, kind, attempts) = running(&state, id).await?;
    stories::remove_parts(&state, id).await;
    if failure.retry && attempts < MAX_ATTEMPTS {
        tracing::warn!("story job {id} (story {}) failed, will retry: {}", row.id, failure.error);
        sqlx::query("UPDATE story_jobs SET state = 'queued', claimed_at = NULL, error = ? WHERE id = ?")
            .bind(failure.error.chars().take(500).collect::<String>())
            .bind(id)
            .execute(&state.db)
            .await?;
    } else {
        give_up(&state, id, row.id, kind, &failure.error).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}
