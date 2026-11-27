//! Messages: history, sending, deletion, reactions and read marks.

use crate::{
    api::{is_blocked, require_member},
    auth::{now_ms, Auth},
    error::{bad, not_found, Error, Result},
    media, music,
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use proto::*;
use serde::Deserialize;
use std::{collections::HashMap, time::Duration};

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: i64,
    conversation_id: i64,
    body: String,
    created_at: i64,
    deleted: bool,
    sender_id: i64,
    username: String,
    display_name: String,
    avatar: Option<String>,
    media_id: Option<String>,
    mime: Option<String>,
    name: Option<String>,
    size: Option<i64>,
    reply_to: Option<i64>,
    reply_sender: Option<String>,
    reply_body: Option<String>,
    reply_mime: Option<String>,
    reply_deleted: Option<bool>,
    track_id: Option<String>,
    track_title: Option<String>,
    track_artist: Option<String>,
    track_album: Option<String>,
    track_duration: Option<i64>,
    track_cover: Option<bool>,
}

/// Builds the message; the state signs the track key.
fn message_of(state: &AppState, r: MessageRow) -> Message {
    let track = r.track_id.clone().map(|id| Track {
        key: music::track_key(state, &id),
        title: r.track_title.clone().unwrap_or_default(),
        artist: r.track_artist.clone().unwrap_or_default(),
        album: r.track_album.clone().unwrap_or_default(),
        duration: r.track_duration.unwrap_or(0),
        has_cover: r.track_cover.unwrap_or(false),
        id,
    });
    let mut message: Message = r.into();
    message.track = track;
    message
}

impl From<MessageRow> for Message {
    fn from(r: MessageRow) -> Self {
        let media = match (r.media_id, r.mime, r.name, r.size) {
            (Some(id), Some(mime), Some(name), Some(size)) => Some(Media { id, mime, name, size }),
            _ => None,
        };
        let reply = r.reply_to.map(|id| Quote {
            id,
            // No sender means the original is gone with its author's account.
            deleted: r.reply_deleted.unwrap_or(true),
            sender_name: r.reply_sender.unwrap_or_default(),
            body: r.reply_body.unwrap_or_default().chars().take(120).collect(),
            mime: r.reply_mime,
        });
        Message {
            id: r.id,
            conversation_id: r.conversation_id,
            sender: User {
                id: r.sender_id,
                username: r.username,
                display_name: r.display_name,
                avatar: r.avatar,
            },
            body: r.body,
            media,
            created_at: r.created_at,
            reply,
            reactions: vec![],
            deleted: r.deleted,
            track: None,
        }
    }
}

const SELECT: &str = "SELECT m.id, m.conversation_id, m.body, m.created_at, m.deleted,
    u.id AS sender_id, u.username, u.display_name, u.avatar,
    f.id AS media_id, f.mime, f.name, f.size,
    m.reply_to, ru.display_name AS reply_sender, r.body AS reply_body,
    rf.mime AS reply_mime, r.deleted AS reply_deleted,
    m.track_id, m.track_title, m.track_artist, m.track_album, m.track_duration, m.track_cover
    FROM messages m JOIN users u ON u.id = m.sender_id
    LEFT JOIN media f ON f.id = m.media_id
    LEFT JOIN messages r ON r.id = m.reply_to
    LEFT JOIN users ru ON ru.id = r.sender_id
    LEFT JOIN media rf ON rf.id = r.media_id";

async fn with_reactions(state: &AppState, rows: Vec<MessageRow>) -> Result<Vec<Message>> {
    let mut messages: Vec<Message> = rows.into_iter().map(|r| message_of(state, r)).collect();
    if messages.is_empty() {
        return Ok(messages);
    }
    let marks = vec!["?"; messages.len()].join(",");
    let sql = format!(
        "SELECT message_id, user_id, emoji FROM reactions WHERE message_id IN ({marks}) ORDER BY user_id"
    );
    let mut query = sqlx::query_as::<_, (i64, i64, String)>(&sql);
    for message in &messages {
        query = query.bind(message.id);
    }
    let mut found: HashMap<i64, Vec<Reaction>> = HashMap::new();
    for (message, user_id, emoji) in query.fetch_all(&state.db).await? {
        found.entry(message).or_default().push(Reaction { user_id, emoji });
    }
    for message in &mut messages {
        message.reactions = found.remove(&message.id).unwrap_or_default();
    }
    Ok(messages)
}

async fn by_id(state: &AppState, id: i64) -> Result<Message> {
    let sql = format!("{SELECT} WHERE m.id = ?");
    let rows: Vec<MessageRow> = sqlx::query_as(&sql).bind(id).fetch_all(&state.db).await?;
    with_reactions(state, rows).await?.pop().ok_or_else(not_found)
}

pub async fn last(state: &AppState, conversation: i64) -> Result<Option<Message>> {
    let sql = format!("{SELECT} WHERE m.conversation_id = ? ORDER BY m.id DESC LIMIT 1");
    let row: Option<MessageRow> =
        sqlx::query_as(&sql).bind(conversation).fetch_optional(&state.db).await?;
    Ok(row.map(|r| message_of(state, r)))
}

#[derive(Deserialize)]
pub struct Page {
    before: Option<i64>,
}

pub async fn history(
    State(state): State<AppState>,
    Auth(user): Auth,
    Path(id): Path<i64>,
    Query(page): Query<Page>,
) -> Result<Json<Vec<Message>>> {
    require_member(&state, id, user.id).await?;
    let sql = format!("{SELECT} WHERE m.conversation_id = ? AND m.id < ? ORDER BY m.id DESC LIMIT 50");
    let mut rows: Vec<MessageRow> = sqlx::query_as(&sql)
        .bind(id)
        .bind(page.before.unwrap_or(i64::MAX))
        .fetch_all(&state.db)
        .await?;
    rows.reverse();
    Ok(Json(with_reactions(&state, rows).await?))
}

fn slow_down() -> Error {
    Error(StatusCode::TOO_MANY_REQUESTS, "Doucement ! Patientez un instant.".into())
}

/// Refuses to write in a one-to-one conversation where either side blocked the other.
async fn require_open(state: &AppState, conversation: i64, members: &[i64], user: i64) -> Result<()> {
    let (is_group,): (bool,) = sqlx::query_as("SELECT is_group FROM conversations WHERE id = ?")
        .bind(conversation)
        .fetch_one(&state.db)
        .await?;
    if !is_group {
        for &other in members.iter().filter(|&&m| m != user) {
            if is_blocked(state, user, other).await? {
                return Err(Error(StatusCode::FORBIDDEN, "Vous ne pouvez pas écrire à cette personne.".into()));
            }
        }
    }
    Ok(())
}

pub async fn send(
    State(state): State<AppState>,
    Auth(user): Auth,
    Path(id): Path<i64>,
    Json(input): Json<NewMessage>,
) -> Result<Json<Message>> {
    let members = require_member(&state, id, user.id).await?;
    require_open(&state, id, &members, user.id).await?;
    let body = input.body.trim().to_string();
    if body.is_empty() && input.media_id.is_none() && input.track_id.is_none() {
        return Err(bad("Message vide."));
    }
    if body.chars().count() > 4000 {
        return Err(bad("Message trop long."));
    }
    if !state.allow(user.id.to_string(), "message", 60, Duration::from_secs(60))
        || !state.allow(user.id.to_string(), "message-day", 3000, Duration::from_secs(86400))
    {
        return Err(slow_down());
    }
    if let Some(media_id) = &input.media_id {
        // Only the uploader may attach a file.
        sqlx::query("SELECT 1 FROM media WHERE id = ? AND owner_id = ?")
            .bind(media_id)
            .bind(user.id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| bad("Fichier introuvable."))?;
    }
    if let Some(reply_to) = input.reply_to {
        // Quoting across conversations would leak the quoted text.
        sqlx::query("SELECT 1 FROM messages WHERE id = ? AND conversation_id = ? AND deleted = 0")
            .bind(reply_to)
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| bad("Message d'origine introuvable."))?;
    }
    // Looked up last: it calls goofysound, so the cheap checks come first.
    let track = match &input.track_id {
        Some(track_id) => Some(music::lookup(&state, user.id, track_id).await?),
        None => None,
    };
    let message_id = sqlx::query(
        "INSERT INTO messages (conversation_id, sender_id, body, media_id, reply_to, created_at,
           track_id, track_title, track_artist, track_album, track_duration, track_cover)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(user.id)
    .bind(&body)
    .bind(&input.media_id)
    .bind(input.reply_to)
    .bind(now_ms())
    .bind(track.as_ref().map(|t| &t.id))
    .bind(track.as_ref().map(|t| &t.title))
    .bind(track.as_ref().map(|t| &t.artist))
    .bind(track.as_ref().map(|t| &t.album))
    .bind(track.as_ref().map(|t| t.duration))
    .bind(track.as_ref().map(|t| t.has_cover))
    .execute(&state.db)
    .await?
    .last_insert_rowid();
    let message = by_id(&state, message_id).await?;
    state.send(&members, &ServerEvent::Message { message: message.clone() });
    Ok(Json(message))
}

/// The conversation, sender and members of a message the user can see.
async fn visible(state: &AppState, message: i64, user: i64) -> Result<(i64, i64, Vec<i64>)> {
    let found: Option<(i64, i64)> = sqlx::query_as(
        "SELECT conversation_id, sender_id FROM messages WHERE id = ? AND deleted = 0",
    )
    .bind(message)
    .fetch_optional(&state.db)
    .await?;
    let (conversation, sender) = found.ok_or_else(not_found)?;
    let members = require_member(state, conversation, user).await?;
    Ok((conversation, sender, members))
}

pub async fn delete(
    State(state): State<AppState>,
    Auth(user): Auth,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    let (_, sender, members) = visible(&state, id, user.id).await?;
    if sender != user.id {
        return Err(Error(StatusCode::FORBIDDEN, "Vous ne pouvez supprimer que vos messages.".into()));
    }
    let (media_id,): (Option<String>,) = sqlx::query_as("SELECT media_id FROM messages WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    let mut tx = state.db.begin().await?;
    sqlx::query(
        "UPDATE messages SET deleted = 1, body = '', media_id = NULL, track_id = NULL, track_title = NULL,
           track_artist = NULL, track_album = NULL, track_duration = NULL, track_cover = NULL WHERE id = ?",
    )
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM reactions WHERE message_id = ?").bind(id).execute(&mut *tx).await?;
    tx.commit().await?;
    if let Some(media_id) = media_id {
        media::discard(&state, &media_id).await?;
    }
    let message = by_id(&state, id).await?;
    state.send(&members, &ServerEvent::Update { message });
    Ok(StatusCode::NO_CONTENT)
}

pub async fn react(
    State(state): State<AppState>,
    Auth(user): Auth,
    Path(id): Path<i64>,
    Json(input): Json<React>,
) -> Result<StatusCode> {
    let (conversation, _, members) = visible(&state, id, user.id).await?;
    require_open(&state, conversation, &members, user.id).await?;
    if !state.allow(user.id.to_string(), "reaction", 60, Duration::from_secs(60)) {
        return Err(slow_down());
    }
    match input.emoji {
        Some(emoji) => {
            if !REACTIONS.contains(&emoji.as_str()) {
                return Err(bad("Réaction inconnue."));
            }
            sqlx::query(
                "INSERT INTO reactions (message_id, user_id, emoji) VALUES (?, ?, ?)
                 ON CONFLICT(message_id, user_id) DO UPDATE SET emoji = excluded.emoji",
            )
            .bind(id)
            .bind(user.id)
            .bind(emoji)
            .execute(&state.db)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM reactions WHERE message_id = ? AND user_id = ?")
                .bind(id)
                .bind(user.id)
                .execute(&state.db)
                .await?;
        }
    }
    let message = by_id(&state, id).await?;
    state.send(&members, &ServerEvent::Update { message });
    Ok(StatusCode::NO_CONTENT)
}

pub async fn mark_read(
    State(state): State<AppState>,
    Auth(user): Auth,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    let members = require_member(&state, id, user.id).await?;
    let moved: Option<(i64,)> = sqlx::query_as(
        "UPDATE members SET last_read = (SELECT MAX(id) FROM messages WHERE conversation_id = ?1)
         WHERE conversation_id = ?1 AND user_id = ?2
           AND last_read < (SELECT COALESCE(MAX(id), 0) FROM messages WHERE conversation_id = ?1)
         RETURNING last_read",
    )
    .bind(id)
    .bind(user.id)
    .fetch_optional(&state.db)
    .await?;
    // Only announce progress: clients mark as read often.
    if let Some((message_id,)) = moved {
        let mark = ReadMark { user_id: user.id, message_id };
        state.send(&members, &ServerEvent::Read { conversation_id: id, mark });
    }
    Ok(StatusCode::NO_CONTENT)
}
