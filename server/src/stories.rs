//! Stories: a photo or a short video shown to friends for 24 hours, then kept in the
//! author's private archive. Videos are prepared by the stories worker (`internal.rs`)
//! before they go live; archives are compressed by it.
//!
//! Story files are never served by media id: `GET /api/stories/{id}/media|poster?k=`
//! takes a live key (valid while the story is live) or an archive key (author only).

use crate::{
    auth::{now_ms, user_by_id, Auth},
    error::{bad, not_found, Error, Result},
    friends, imaging,
    links::{sign, verify},
    media,
    state::AppState,
};
use axum::{
    extract::{Path, Query, Request, State},
    http::StatusCode,
    response::Response,
    Json,
};
use proto::*;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

/// A running prepare job older than this has lost its worker: the story fails.
pub const STUCK_RUNNING_MS: i64 = 30 * 60 * 1000;
/// A video still waiting for the worker after this fails (the worker is down).
pub const STUCK_QUEUED_MS: i64 = 6 * 3600 * 1000;
/// Failed stories are deleted with their files after this.
pub const FAILED_KEPT_MS: i64 = 24 * 3600 * 1000;

#[derive(sqlx::FromRow, Clone, Debug)]
pub struct StoryRow {
    pub id: i64,
    pub author_id: i64,
    /// Own files (none for a reshare).
    pub media_id: Option<String>,
    pub poster_id: Option<String>,
    pub source_id: Option<i64>,
    pub is_video: bool,
    pub state: String,
    pub width: i64,
    pub height: i64,
    pub duration_ms: i64,
    pub created_at: i64,
    pub expires_at: i64,
    /// The files served: the source's for a reshare.
    pub file_id: Option<String>,
    pub poster: Option<String>,
    pub compressed: bool,
}

pub const SELECT: &str = "SELECT s.id, s.author_id, s.media_id, s.poster_id, s.source_id, s.is_video,
    s.state, s.width, s.height, s.duration_ms, s.created_at, s.expires_at,
    COALESCE(s.media_id, src.media_id) AS file_id, COALESCE(s.poster_id, src.poster_id) AS poster,
    CASE WHEN s.source_id IS NULL THEN s.compressed ELSE COALESCE(src.compressed, 0) END AS compressed
    FROM stories s LEFT JOIN stories src ON src.id = s.source_id";

pub async fn load(state: &AppState, id: i64) -> Result<Option<StoryRow>> {
    Ok(sqlx::query_as(&format!("{SELECT} WHERE s.id = ?")).bind(id).fetch_optional(&state.db).await?)
}

fn state_of(s: &str) -> StoryState {
    match s {
        "live" => StoryState::Live,
        "archived" => StoryState::Archived,
        "failed" => StoryState::Failed,
        _ => StoryState::Processing,
    }
}

pub fn live_key(state: &AppState, id: i64) -> String {
    sign(state, &format!("story:{id}"))
}

pub fn archive_key(state: &AppState, id: i64) -> String {
    sign(state, &format!("story-archive:{id}"))
}

/// The JSON form. Archived stories carry the archive key, all others the live key.
pub fn story_of(state: &AppState, row: &StoryRow, seen: bool) -> Story {
    let story_state = state_of(&row.state);
    Story {
        id: row.id,
        user_id: row.author_id,
        is_video: row.is_video,
        state: story_state,
        created_at: row.created_at,
        expires_at: row.expires_at,
        duration_ms: row.duration_ms,
        width: row.width,
        height: row.height,
        has_poster: row.is_video && row.poster.is_some(),
        key: if story_state == StoryState::Archived { archive_key(state, row.id) } else { live_key(state, row.id) },
        seen,
        compressed: row.compressed,
        reshared_from: row.source_id,
    }
}

/// `Story` event to the author's devices, and to their friends when `friends_too`.
pub async fn announce(state: &AppState, row: &StoryRow, friends_too: bool) -> Result<()> {
    let Some(user) = user_by_id(state, row.author_id).await? else { return Ok(()) };
    let mine = ServerEvent::Story { story: story_of(state, row, true), user: user.clone() };
    state.send(&[row.author_id], &mine);
    if friends_too {
        let theirs = ServerEvent::Story { story: story_of(state, row, false), user };
        state.send(&friends::friend_ids(state, row.author_id).await?, &theirs);
    }
    Ok(())
}

/// `StoryGone` to the author's devices, and to their friends when `friends_too`.
async fn gone(state: &AppState, id: i64, author: i64, friends_too: bool) -> Result<()> {
    let to = if friends_too { friends::self_and_friends(state, author).await? } else { vec![author] };
    state.send(&to, &ServerEvent::StoryGone { id, user_id: author });
    Ok(())
}

fn too_many_live() -> Error {
    bad("Trop de stories en ligne.")
}

/// Refuses a new live story beyond the limits (count, then rate).
async fn check_limits(state: &AppState, author: i64) -> Result<()> {
    let (live,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM stories WHERE author_id = ? AND state IN ('live', 'processing')",
    )
    .bind(author)
    .fetch_one(&state.db)
    .await?;
    if live as usize >= limits::STORIES_LIVE {
        return Err(too_many_live());
    }
    if !state.allow(author.to_string(), "story", 30, Duration::from_secs(86400)) {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "Trop de stories aujourd'hui.".into()));
    }
    Ok(())
}

pub async fn create(
    State(state): State<AppState>,
    Auth(user): Auth,
    Json(input): Json<NewStory>,
) -> Result<Json<Story>> {
    let found: Option<(String, i64)> = sqlx::query_as("SELECT mime, size FROM media WHERE id = ? AND owner_id = ?")
        .bind(&input.media_id)
        .bind(user.id)
        .fetch_optional(&state.db)
        .await?;
    let (mime, size) = found.ok_or_else(|| bad("Fichier introuvable."))?;
    if media::in_use(&state, &input.media_id).await? {
        return Err(bad("Fichier déjà utilisé."));
    }
    let is_video = match mime.as_str() {
        "image/jpeg" | "image/png" | "image/webp" => false,
        "video/mp4" | "video/webm" | "video/quicktime" => true,
        _ => return Err(bad("Une story est une photo ou une vidéo.")),
    };
    let max = if is_video { limits::STORY_VIDEO_BYTES } else { limits::STORY_PHOTO_BYTES };
    if size > max {
        return Err(media::too_large("Story trop volumineuse."));
    }
    if is_video && !state.has_worker() {
        return Err(Error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Les stories vidéo ne sont pas disponibles sur ce serveur.".into(),
        ));
    }
    check_limits(&state, user.id).await?;
    let now = now_ms();
    let id = if is_video {
        let mut tx = state.db.begin().await?;
        let id = sqlx::query(
            "INSERT INTO stories (author_id, media_id, is_video, state, created_at, expires_at)
             VALUES (?, ?, 1, 'processing', ?, 0)",
        )
        .bind(user.id)
        .bind(&input.media_id)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();
        sqlx::query("INSERT INTO story_jobs (story_id, kind, created_at) VALUES (?, 'prepare', ?)")
            .bind(id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        id
    } else {
        let bytes = media::read(&state, &input.media_id).await?;
        let info = imaging::run(&state, move || imaging::probe(&bytes))
            .await?
            .filter(|i| i.format != image::ImageFormat::Gif)
            .ok_or_else(|| bad("Image illisible."))?;
        sqlx::query(
            "INSERT INTO stories (author_id, media_id, is_video, state, width, height, duration_ms, created_at, expires_at)
             VALUES (?, ?, 0, 'live', ?, ?, ?, ?, ?)",
        )
        .bind(user.id)
        .bind(&input.media_id)
        .bind(info.width as i64)
        .bind(info.height as i64)
        .bind(limits::STORY_PHOTO_MS)
        .bind(now)
        .bind(now + limits::STORY_LIFETIME_MS)
        .execute(&state.db)
        .await?
        .last_insert_rowid()
    };
    let row = load(&state, id).await?.ok_or_else(not_found)?;
    announce(&state, &row, !is_video).await?;
    Ok(Json(story_of(&state, &row, true)))
}

/// The caller's own reel (processing, live, failed), then friends' live reels: unseen
/// first, then by latest story.
pub async fn reels(State(state): State<AppState>, Auth(me): Auth) -> Result<Json<Vec<StoryReel>>> {
    let now = now_ms();
    let own: Vec<StoryRow> = sqlx::query_as(&format!(
        "{SELECT} WHERE s.author_id = ?1 AND s.state IN ('processing', 'live', 'failed')
           AND (s.state != 'live' OR s.expires_at > ?2)
         ORDER BY s.created_at, s.id"
    ))
    .bind(me.id)
    .bind(now)
    .fetch_all(&state.db)
    .await?;
    let theirs: Vec<StoryRow> = sqlx::query_as(&format!(
        "{SELECT} WHERE s.state = 'live' AND s.expires_at > ?2 AND s.author_id IN
           (SELECT CASE WHEN user_a = ?1 THEN user_b ELSE user_a END FROM friendships
            WHERE user_a = ?1 OR user_b = ?1)
         ORDER BY s.created_at, s.id"
    ))
    .bind(me.id)
    .bind(now)
    .fetch_all(&state.db)
    .await?;
    let seen: HashSet<i64> = sqlx::query_as::<_, (i64,)>("SELECT story_id FROM story_views WHERE user_id = ?")
        .bind(me.id)
        .fetch_all(&state.db)
        .await?
        .into_iter()
        .map(|r| r.0)
        .collect();

    let mut list = Vec::new();
    if !own.is_empty() {
        let stories = own.iter().map(|r| story_of(&state, r, true)).collect();
        list.push(StoryReel { user: me.clone(), stories, unseen: false });
    }
    let mut by_author: HashMap<i64, Vec<Story>> = HashMap::new();
    for row in &theirs {
        by_author.entry(row.author_id).or_default().push(story_of(&state, row, seen.contains(&row.id)));
    }
    let mut friends_reels = Vec::new();
    for (author, stories) in by_author {
        let Some(user) = user_by_id(&state, author).await? else { continue };
        let unseen = stories.iter().any(|s| !s.seen);
        friends_reels.push(StoryReel { user, stories, unseen });
    }
    let latest = |r: &StoryReel| r.stories.iter().map(|s| s.created_at).max().unwrap_or(0);
    friends_reels.sort_by_key(|r| (!r.unseen, std::cmp::Reverse(latest(r)), r.user.id));
    list.extend(friends_reels);
    Ok(Json(list))
}

/// A live story of a friend (or of the caller).
async fn live_for(state: &AppState, me: i64, id: i64) -> Result<StoryRow> {
    let row = load(state, id).await?.ok_or_else(not_found)?;
    let live = row.state == "live" && row.expires_at > now_ms();
    if !live || (row.author_id != me && !friends::are_friends(state, me, row.author_id).await?) {
        return Err(not_found());
    }
    Ok(row)
}

pub async fn seen(State(state): State<AppState>, Auth(me): Auth, Path(id): Path<i64>) -> Result<StatusCode> {
    let row = live_for(&state, me.id, id).await?;
    // One's own stories are always seen.
    if row.author_id != me.id {
        sqlx::query("INSERT OR IGNORE INTO story_views (story_id, user_id, seen_at) VALUES (?, ?, ?)")
            .bind(id)
            .bind(me.id)
            .bind(now_ms())
            .execute(&state.db)
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Removes the worker's half-uploaded outputs of a story's jobs.
pub async fn remove_parts(state: &AppState, job: i64) {
    for slot in ["main", "poster"] {
        let path = state.config.data_dir.join("media").join(format!("job-{job}-{slot}.part"));
        let _ = tokio::fs::remove_file(path).await;
    }
}

/// Deletes a story row (its reshares and jobs cascade) and the files it owned.
async fn remove(state: &AppState, row: &StoryRow) -> Result<()> {
    let jobs: Vec<(i64,)> = sqlx::query_as("SELECT id FROM story_jobs WHERE story_id = ?")
        .bind(row.id)
        .fetch_all(&state.db)
        .await?;
    sqlx::query("DELETE FROM stories WHERE id = ?").bind(row.id).execute(&state.db).await?;
    for (job,) in jobs {
        remove_parts(state, job).await;
    }
    for file in [&row.media_id, &row.poster_id].into_iter().flatten() {
        media::discard(state, file).await?;
    }
    Ok(())
}

pub async fn delete(State(state): State<AppState>, Auth(me): Auth, Path(id): Path<i64>) -> Result<StatusCode> {
    let row = load(&state, id).await?.filter(|r| r.author_id == me.id).ok_or_else(not_found)?;
    let reshares: Vec<(i64, String)> = sqlx::query_as("SELECT id, state FROM stories WHERE source_id = ?")
        .bind(id)
        .fetch_all(&state.db)
        .await?;
    remove(&state, &row).await?;
    gone(&state, id, me.id, row.state == "live").await?;
    for (reshare, reshare_state) in reshares {
        gone(&state, reshare, me.id, reshare_state == "live").await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct Before {
    before: Option<i64>,
}

const ARCHIVE_PAGE: usize = 60;

pub async fn archive(
    State(state): State<AppState>,
    Auth(me): Auth,
    Query(page): Query<Before>,
) -> Result<Json<StoryPage>> {
    let rows: Vec<StoryRow> = sqlx::query_as(&format!(
        "{SELECT} WHERE s.author_id = ? AND s.state = 'archived' AND s.source_id IS NULL AND s.id < ?
         ORDER BY s.id DESC LIMIT ?"
    ))
    .bind(me.id)
    .bind(page.before.unwrap_or(i64::MAX))
    .bind(ARCHIVE_PAGE as i64)
    .fetch_all(&state.db)
    .await?;
    let stories: Vec<Story> = rows.iter().map(|r| story_of(&state, r, true)).collect();
    let next = (stories.len() == ARCHIVE_PAGE).then(|| stories.last().map(|s| s.id)).flatten();
    Ok(Json(StoryPage { stories, next }))
}

pub async fn reshare(State(state): State<AppState>, Auth(me): Auth, Path(id): Path<i64>) -> Result<Json<Story>> {
    let source = load(&state, id).await?.filter(|r| r.author_id == me.id).ok_or_else(not_found)?;
    if source.state != "archived" || source.source_id.is_some() {
        return Err(bad("Seule une story archivée peut être partagée à nouveau."));
    }
    check_limits(&state, me.id).await?;
    let now = now_ms();
    let new = sqlx::query(
        "INSERT INTO stories (author_id, source_id, is_video, state, width, height, duration_ms, created_at, expires_at)
         VALUES (?, ?, ?, 'live', ?, ?, ?, ?, ?)",
    )
    .bind(me.id)
    .bind(id)
    .bind(source.is_video)
    .bind(source.width)
    .bind(source.height)
    .bind(source.duration_ms)
    .bind(now)
    .bind(now + limits::STORY_LIFETIME_MS)
    .execute(&state.db)
    .await?
    .last_insert_rowid();
    let row = load(&state, new).await?.ok_or_else(not_found)?;
    announce(&state, &row, true).await?;
    Ok(Json(story_of(&state, &row, true)))
}

#[derive(Deserialize)]
pub struct Signed {
    #[serde(default)]
    k: String,
}

async fn serve(state: &AppState, id: i64, key: &str, request: Request, poster: bool) -> Result<Response> {
    let invalid = || Error(StatusCode::FORBIDDEN, "Lien invalide.".into());
    let live = verify(state, &format!("story:{id}"), key);
    if !live && !verify(state, &format!("story-archive:{id}"), key) {
        return Err(invalid());
    }
    let row = load(state, id).await?.ok_or_else(not_found)?;
    if matches!(row.state.as_str(), "processing" | "failed") {
        return Err(not_found());
    }
    if live && !(row.state == "live" && row.expires_at > now_ms()) {
        return Err(Error(StatusCode::FORBIDDEN, "Story expirée.".into()));
    }
    if !live && row.state != "archived" {
        return Err(invalid());
    }
    let file = if poster { row.poster } else { row.file_id };
    let file = file.ok_or_else(not_found)?;
    media::serve(state, &file, request, "private, max-age=86400").await
}

pub async fn media(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(signed): Query<Signed>,
    request: Request,
) -> Result<Response> {
    serve(&state, id, &signed.k, request, false).await
}

pub async fn poster(
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Query(signed): Query<Signed>,
    request: Request,
) -> Result<Response> {
    serve(&state, id, &signed.k, request, true).await
}

/// Moves stories along at `now`: expired reshares go, expired originals are archived
/// (and queued for compression), and videos stuck in processing fail.
pub async fn expire(state: &AppState, now: i64) -> Result<()> {
    let reshares: Vec<StoryRow> = sqlx::query_as(&format!(
        "{SELECT} WHERE s.state = 'live' AND s.source_id IS NOT NULL AND s.expires_at <= ?"
    ))
    .bind(now)
    .fetch_all(&state.db)
    .await?;
    for row in reshares {
        remove(state, &row).await?;
        gone(state, row.id, row.author_id, true).await?;
    }

    let originals: Vec<StoryRow> = sqlx::query_as(&format!(
        "{SELECT} WHERE s.state = 'live' AND s.source_id IS NULL AND s.expires_at <= ?"
    ))
    .bind(now)
    .fetch_all(&state.db)
    .await?;
    for row in originals {
        let mut tx = state.db.begin().await?;
        let moved = sqlx::query("UPDATE stories SET state = 'archived' WHERE id = ? AND state = 'live'")
            .bind(row.id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if moved > 0 && state.has_worker() {
            sqlx::query("INSERT INTO story_jobs (story_id, kind, created_at) VALUES (?, 'archive', ?)")
                .bind(row.id)
                .bind(now)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        if moved == 0 {
            continue;
        }
        state.send(
            &friends::friend_ids(state, row.author_id).await?,
            &ServerEvent::StoryGone { id: row.id, user_id: row.author_id },
        );
        if let Some(row) = load(state, row.id).await? {
            announce(state, &row, false).await?;
        }
    }

    // Videos wait in line for the one worker; they fail only when their job was lost,
    // when the worker took too long, or when no worker came for hours.
    let stuck: Vec<(i64,)> = sqlx::query_as(
        "SELECT s.id FROM stories s WHERE s.state = 'processing' AND (
             s.created_at <= ?2
             OR NOT EXISTS (SELECT 1 FROM story_jobs j WHERE j.story_id = s.id AND j.state != 'failed')
             OR EXISTS (SELECT 1 FROM story_jobs j WHERE j.story_id = s.id
                        AND j.state = 'running' AND j.claimed_at <= ?1))",
    )
    .bind(now - STUCK_RUNNING_MS)
    .bind(now - STUCK_QUEUED_MS)
    .fetch_all(&state.db)
    .await?;
    for (id,) in stuck {
        fail_processing(state, id).await?;
    }
    Ok(())
}

/// A video that will never go live: `failed`, its jobs removed, the author told.
pub async fn fail_processing(state: &AppState, id: i64) -> Result<()> {
    let jobs: Vec<(i64,)> = sqlx::query_as("SELECT id FROM story_jobs WHERE story_id = ?")
        .bind(id)
        .fetch_all(&state.db)
        .await?;
    let failed = sqlx::query("UPDATE stories SET state = 'failed' WHERE id = ? AND state = 'processing'")
        .bind(id)
        .execute(&state.db)
        .await?
        .rows_affected();
    if failed == 0 {
        return Ok(());
    }
    sqlx::query("DELETE FROM story_jobs WHERE story_id = ? AND state != 'failed'").bind(id).execute(&state.db).await?;
    for (job,) in jobs {
        remove_parts(state, job).await;
    }
    if let Some(row) = load(state, id).await? {
        announce(state, &row, false).await?;
    }
    Ok(())
}

/// Deletes failed stories older than a day, with their files (hourly cleanup).
pub async fn cleanup_failed(state: &AppState, now: i64) -> Result<()> {
    let rows: Vec<StoryRow> =
        sqlx::query_as(&format!("{SELECT} WHERE s.state = 'failed' AND s.created_at <= ?"))
            .bind(now - FAILED_KEPT_MS)
            .fetch_all(&state.db)
            .await?;
    for row in rows {
        remove(state, &row).await?;
        gone(state, row.id, row.author_id, false).await?;
    }
    Ok(())
}

pub async fn expire_loop(state: AppState) {
    let mut every = tokio::time::interval(Duration::from_secs(30));
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        every.tick().await;
        if let Err(e) = expire(&state, now_ms()).await {
            tracing::error!("stories: {}", e.1);
        }
    }
}
