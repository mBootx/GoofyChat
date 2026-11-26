use crate::{
    auth::{self, now_ms, Auth},
    error::{bad, not_found, Error, Result},
    media,
    state::AppState,
    ws,
};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use base64::Engine;
use hmac::{Hmac, Mac};
use proto::*;
use serde::Deserialize;
use std::time::Duration;
use tower_http::cors::{Any, CorsLayer};

pub fn router(state: AppState) -> Router {
    let upload_limit = state.config.max_upload as usize + 1024 * 1024;
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/register", post(auth::register))
        .route("/api/login", post(auth::login))
        .route("/api/logout", post(auth::logout))
        .route("/api/me", get(me))
        .route("/api/users", get(search_users))
        .route("/api/conversations", get(list_conversations).post(create_conversation))
        .route("/api/conversations/{id}/messages", get(history).post(send_message))
        .route("/api/conversations/{id}/read", post(mark_read))
        .route("/api/media", post(media::upload).layer(DefaultBodyLimit::max(upload_limit)))
        .route("/api/media/{id}", get(media::download))
        .route("/api/ice", get(ice))
        .route("/api/ws", get(ws::handler))
        // Authentication is a bearer header, never a cookie, so any origin is safe.
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .with_state(state)
}

async fn me(Auth(user): Auth) -> Json<User> {
    Json(user)
}

#[derive(Deserialize)]
struct Search {
    q: String,
}

async fn search_users(
    State(state): State<AppState>,
    Auth(user): Auth,
    Query(search): Query<Search>,
) -> Result<Json<Vec<User>>> {
    let q = search.q.trim().to_lowercase();
    if q.chars().count() < 2 {
        return Ok(Json(vec![]));
    }
    let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id, username, display_name FROM users
         WHERE id != ? AND username LIKE ? ESCAPE '\\' ORDER BY username LIMIT 20",
    )
    .bind(user.id)
    .bind(format!("{escaped}%"))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows.into_iter().map(to_user).collect()))
}

fn to_user((id, username, display_name): (i64, String, String)) -> User {
    User { id, username, display_name }
}

pub async fn member_ids(state: &AppState, conversation: i64) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as("SELECT user_id FROM members WHERE conversation_id = ?")
        .bind(conversation)
        .fetch_all(&state.db)
        .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// Returns the member ids, or 404 when the user is not part of the conversation.
async fn require_member(state: &AppState, conversation: i64, user: i64) -> Result<Vec<i64>> {
    let ids = member_ids(state, conversation).await?;
    if ids.contains(&user) {
        Ok(ids)
    } else {
        Err(not_found())
    }
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    id: i64,
    conversation_id: i64,
    body: String,
    created_at: i64,
    sender_id: i64,
    username: String,
    display_name: String,
    media_id: Option<String>,
    mime: Option<String>,
    name: Option<String>,
    size: Option<i64>,
}

impl From<MessageRow> for Message {
    fn from(r: MessageRow) -> Self {
        let media = match (r.media_id, r.mime, r.name, r.size) {
            (Some(id), Some(mime), Some(name), Some(size)) => Some(Media { id, mime, name, size }),
            _ => None,
        };
        Message {
            id: r.id,
            conversation_id: r.conversation_id,
            sender: User { id: r.sender_id, username: r.username, display_name: r.display_name },
            body: r.body,
            media,
            created_at: r.created_at,
        }
    }
}

const MESSAGE_SELECT: &str = "SELECT m.id, m.conversation_id, m.body, m.created_at,
    u.id AS sender_id, u.username, u.display_name,
    f.id AS media_id, f.mime, f.name, f.size
    FROM messages m JOIN users u ON u.id = m.sender_id LEFT JOIN media f ON f.id = m.media_id";

async fn list_conversations(
    State(state): State<AppState>,
    Auth(user): Auth,
) -> Result<Json<Vec<Conversation>>> {
    let rows: Vec<(i64, String, bool, i64, i64)> = sqlx::query_as(
        "SELECT c.id, c.name, c.is_group, c.created_at,
           (SELECT COUNT(*) FROM messages x WHERE x.conversation_id = c.id
              AND x.id > me.last_read AND x.sender_id != me.user_id)
         FROM conversations c JOIN members me ON me.conversation_id = c.id
         WHERE me.user_id = ?",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    let mut list = Vec::with_capacity(rows.len());
    for (id, name, is_group, created_at, unread) in rows {
        let members: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT u.id, u.username, u.display_name FROM members m
             JOIN users u ON u.id = m.user_id WHERE m.conversation_id = ? ORDER BY u.display_name",
        )
        .bind(id)
        .fetch_all(&state.db)
        .await?;
        let last: Option<MessageRow> = sqlx::query_as(&format!(
            "{MESSAGE_SELECT} WHERE m.conversation_id = ? ORDER BY m.id DESC LIMIT 1"
        ))
        .bind(id)
        .fetch_optional(&state.db)
        .await?;
        let last_message: Option<Message> = last.map(Into::into);
        let activity = last_message.as_ref().map_or(created_at, |m| m.created_at);
        list.push((
            activity,
            Conversation {
                id,
                name,
                is_group,
                members: members.into_iter().map(to_user).collect(),
                last_message,
                unread,
            },
        ));
    }
    list.sort_by_key(|(activity, c)| (std::cmp::Reverse(*activity), std::cmp::Reverse(c.id)));
    Ok(Json(list.into_iter().map(|(_, c)| c).collect()))
}

#[derive(serde::Serialize, Deserialize)]
pub struct Created {
    pub id: i64,
}

async fn create_conversation(
    State(state): State<AppState>,
    Auth(user): Auth,
    Json(input): Json<NewConversation>,
) -> Result<Json<Created>> {
    let mut others = input.member_ids;
    others.retain(|&id| id != user.id);
    others.sort_unstable();
    others.dedup();
    if others.is_empty() || others.len() > 49 {
        return Err(bad("Choisissez entre 1 et 49 personnes."));
    }
    for id in &others {
        sqlx::query("SELECT 1 FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| bad("Utilisateur introuvable."))?;
    }
    let name = input.name.map(|n| n.trim().chars().take(60).collect::<String>()).unwrap_or_default();
    let is_group = !name.is_empty() || others.len() > 1;
    if is_group && name.is_empty() {
        return Err(bad("Donnez un nom au groupe."));
    }
    let direct_key = (!is_group).then(|| {
        let (a, b) = (user.id.min(others[0]), user.id.max(others[0]));
        format!("{a}:{b}")
    });
    if !state.allow(user.id.to_string(), "conversation", 30, Duration::from_secs(3600)) {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "Trop de conversations créées.".into()));
    }

    let mut tx = state.db.begin().await?;
    // NULL direct keys never conflict, so groups are always inserted.
    let inserted = sqlx::query(
        "INSERT INTO conversations (name, is_group, direct_key, created_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(direct_key) DO NOTHING",
    )
    .bind(&name)
    .bind(is_group)
    .bind(&direct_key)
    .bind(now_ms())
    .execute(&mut *tx)
    .await?;
    if inserted.rows_affected() == 0 {
        let (id,): (i64,) = sqlx::query_as("SELECT id FROM conversations WHERE direct_key = ?")
            .bind(&direct_key)
            .fetch_one(&mut *tx)
            .await?;
        return Ok(Json(Created { id }));
    }
    let id = inserted.last_insert_rowid();
    others.push(user.id);
    for member in &others {
        sqlx::query("INSERT INTO members (conversation_id, user_id) VALUES (?, ?)")
            .bind(id)
            .bind(member)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    state.send(&others, &ServerEvent::Conversation { id });
    Ok(Json(Created { id }))
}

#[derive(Deserialize)]
struct Page {
    before: Option<i64>,
}

async fn history(
    State(state): State<AppState>,
    Auth(user): Auth,
    Path(id): Path<i64>,
    Query(page): Query<Page>,
) -> Result<Json<Vec<Message>>> {
    require_member(&state, id, user.id).await?;
    let rows: Vec<MessageRow> = sqlx::query_as(&format!(
        "{MESSAGE_SELECT} WHERE m.conversation_id = ? AND m.id < ? ORDER BY m.id DESC LIMIT 50"
    ))
    .bind(id)
    .bind(page.before.unwrap_or(i64::MAX))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows.into_iter().rev().map(Into::into).collect()))
}

async fn send_message(
    State(state): State<AppState>,
    Auth(user): Auth,
    Path(id): Path<i64>,
    Json(input): Json<NewMessage>,
) -> Result<Json<Message>> {
    let members = require_member(&state, id, user.id).await?;
    let body = input.body.trim().to_string();
    if body.is_empty() && input.media_id.is_none() {
        return Err(bad("Message vide."));
    }
    if body.chars().count() > 4000 {
        return Err(bad("Message trop long."));
    }
    if !state.allow(user.id.to_string(), "message", 60, Duration::from_secs(60))
        || !state.allow(user.id.to_string(), "message-day", 3000, Duration::from_secs(86400))
    {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "Doucement ! Patientez un instant.".into()));
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
    let message_id = sqlx::query(
        "INSERT INTO messages (conversation_id, sender_id, body, media_id, created_at) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(user.id)
    .bind(&body)
    .bind(&input.media_id)
    .bind(now_ms())
    .execute(&state.db)
    .await?
    .last_insert_rowid();
    let row: MessageRow = sqlx::query_as(&format!("{MESSAGE_SELECT} WHERE m.id = ?"))
        .bind(message_id)
        .fetch_one(&state.db)
        .await?;
    let message: Message = row.into();
    state.send(&members, &ServerEvent::Message { message: message.clone() });
    Ok(Json(message))
}

async fn mark_read(
    State(state): State<AppState>,
    Auth(user): Auth,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    require_member(&state, id, user.id).await?;
    sqlx::query(
        "UPDATE members SET last_read =
           COALESCE((SELECT MAX(id) FROM messages WHERE conversation_id = ?1), 0)
         WHERE conversation_id = ?1 AND user_id = ?2",
    )
    .bind(id)
    .bind(user.id)
    .execute(&state.db)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Time-limited TURN credentials (coturn "use-auth-secret" scheme).
async fn ice(State(state): State<AppState>, Auth(user): Auth) -> Result<Json<Vec<IceServer>>> {
    if !state.allow(user.id.to_string(), "ice", 30, Duration::from_secs(600)) {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "Trop d'appels, patientez.".into()));
    }
    let host = &state.config.turn_host;
    if host.is_empty() || state.config.turn_secret.is_empty() {
        return Ok(Json(vec![]));
    }
    let username = format!("{}:{}", now_ms() / 1000 + 3 * 3600, user.id);
    let mut mac = Hmac::<sha1::Sha1>::new_from_slice(state.config.turn_secret.as_bytes())
        .map_err(crate::error::internal)?;
    mac.update(username.as_bytes());
    let credential = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
    Ok(Json(vec![
        IceServer { urls: vec![format!("stun:{host}:7880")], username: None, credential: None },
        IceServer {
            urls: vec![
                format!("turn:{host}:7880?transport=udp"),
                format!("turn:{host}:7880?transport=tcp"),
            ],
            username: Some(username),
            credential: Some(credential),
        },
    ]))
}
