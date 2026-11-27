//! Routes, user search, conversations and call relay credentials.

use crate::{
    account,
    auth::{self, now_ms, Auth, UserRow, USER_COLS},
    error::{bad, not_found, Error, Result},
    friends, internal, links, media, messages, music, photos, profiles,
    state::AppState,
    stories, ws,
};
use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::StatusCode,
    routing::{delete, get, post, put},
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
        .route("/api/me", get(me).put(profiles::update_profile))
        .route("/api/account/delete", post(account::delete_account))
        .route("/api/users", get(profiles::search_users))
        .route("/api/users/{id}", get(profiles::profile))
        .route("/api/users/{id}/media", get(profiles::shared_media))
        .route("/api/friends", get(friends::list))
        .route("/api/friends/{id}", delete(friends::remove))
        .route("/api/friends/requests", get(friends::requests).post(friends::send_request))
        .route("/api/friends/requests/{id}", delete(friends::cancel))
        .route("/api/friends/requests/{id}/accept", post(friends::accept))
        .route("/api/friends/requests/{id}/decline", post(friends::decline))
        .route("/api/stories", get(stories::reels).post(stories::create))
        .route("/api/stories/archive", get(stories::archive))
        .route("/api/stories/{id}", delete(stories::delete))
        .route("/api/stories/{id}/seen", post(stories::seen))
        .route("/api/stories/{id}/reshare", post(stories::reshare))
        .route("/api/stories/{id}/media", get(stories::media))
        .route("/api/stories/{id}/poster", get(stories::poster))
        .route("/api/blocks", get(account::blocked))
        .route("/api/blocks/{id}", put(account::block).delete(account::unblock))
        .route("/api/conversations", get(list_conversations).post(create_conversation))
        .route("/api/conversations/{id}/messages", get(messages::history).post(messages::send))
        .route("/api/conversations/{id}/read", post(messages::mark_read))
        .route("/api/conversations/{id}/leave", post(leave))
        .route("/api/messages/{id}", put(messages::edit).delete(messages::delete))
        .route("/api/messages/{id}/reaction", put(messages::react))
        .route("/api/media", post(media::upload).layer(DefaultBodyLimit::max(upload_limit)))
        .route("/api/media/{id}", get(media::download))
        .route("/api/links", get(links::get_links))
        .route("/api/links/navidrome", post(links::link_navidrome).delete(links::unlink_navidrome))
        .route("/api/links/photos", post(links::link_photos).delete(links::unlink_photos))
        .route("/api/music/search", get(music::search))
        .route("/api/music/{id}/stream", get(music::stream))
        .route("/api/music/{id}/cover", get(music::cover))
        .route("/api/activity", get(music::activity))
        .route("/api/photos", get(photos::list_photos))
        .route("/api/photos/thumb", get(photos::thumb))
        .route("/api/photos/import", post(photos::import))
        .route("/api/ice", get(ice))
        .route("/api/ws", get(ws::handler))
        // Authentication is a bearer header, never a cookie, so any origin is safe.
        .layer(CorsLayer::new().allow_origin(Any).allow_methods(Any).allow_headers(Any))
        .with_state(state)
}

/// The stories worker's API (`WORKER_BIND`, bearer `WORKER_TOKEN`). Never public.
pub fn internal_router(state: AppState) -> Router {
    Router::new()
        .route("/internal/jobs/claim", post(internal::claim))
        .route("/internal/jobs/{id}/input", get(internal::input))
        .route(
            "/internal/jobs/{id}/output/{slot}",
            put(internal::output).layer(DefaultBodyLimit::max(internal::MAIN_BYTES as usize + 1024)),
        )
        .route("/internal/jobs/{id}/done", post(internal::done))
        .route("/internal/jobs/{id}/fail", post(internal::fail))
        .with_state(state)
}

async fn me(Auth(user): Auth) -> Json<User> {
    Json(user)
}

/// Whether either user blocked the other.
pub async fn is_blocked(state: &AppState, a: i64, b: i64) -> Result<bool> {
    let found = sqlx::query(
        "SELECT 1 FROM blocks WHERE (user_id = ?1 AND blocked_id = ?2) OR (user_id = ?2 AND blocked_id = ?1)",
    )
    .bind(a)
    .bind(b)
    .fetch_optional(&state.db)
    .await?;
    Ok(found.is_some())
}

pub async fn member_ids(state: &AppState, conversation: i64) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as("SELECT user_id FROM members WHERE conversation_id = ?")
        .bind(conversation)
        .fetch_all(&state.db)
        .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// Returns the member ids, or 404 when the user is not part of the conversation.
pub async fn require_member(state: &AppState, conversation: i64, user: i64) -> Result<Vec<i64>> {
    let ids = member_ids(state, conversation).await?;
    if ids.contains(&user) {
        Ok(ids)
    } else {
        Err(not_found())
    }
}

async fn list_conversations(
    State(state): State<AppState>,
    Auth(user): Auth,
) -> Result<Json<Vec<Conversation>>> {
    let rows: Vec<(i64, String, bool, i64, i64)> = sqlx::query_as(
        "SELECT c.id, c.name, c.is_group, c.created_at,
           (SELECT COUNT(*) FROM messages x WHERE x.conversation_id = c.id
              AND x.id > me.last_read AND x.sender_id != me.user_id AND x.deleted = 0)
         FROM conversations c JOIN members me ON me.conversation_id = c.id
         WHERE me.user_id = ?",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    let mut list = Vec::with_capacity(rows.len());
    for (id, name, is_group, created_at, unread) in rows {
        let members: Vec<UserRow> = sqlx::query_as(&format!(
            "SELECT {USER_COLS} FROM members m
             JOIN users u ON u.id = m.user_id WHERE m.conversation_id = ? ORDER BY u.display_name"
        ))
        .bind(id)
        .fetch_all(&state.db)
        .await?;
        let members: Vec<User> = members.into_iter().map(Into::into).collect();
        // One-to-one: locked unless the two are friends (and not blocked).
        let locked = !is_group
            && match members.iter().find(|m| m.id != user.id) {
                Some(other) => friends::locked_pair(&state, user.id, other.id).await?,
                None => true,
            };
        let reads: Vec<(i64, i64)> =
            sqlx::query_as("SELECT user_id, last_read FROM members WHERE conversation_id = ?")
                .bind(id)
                .fetch_all(&state.db)
                .await?;
        let last_message = messages::last(&state, id).await?;
        let activity = last_message.as_ref().map_or(created_at, |m| m.created_at);
        list.push((
            activity,
            Conversation {
                id,
                name,
                is_group,
                members,
                last_message,
                unread,
                reads: reads
                    .into_iter()
                    .map(|(user_id, message_id)| ReadMark { user_id, message_id })
                    .collect(),
                locked,
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
    for &id in &others {
        sqlx::query("SELECT 1 FROM users WHERE id = ?")
            .bind(id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| bad("Utilisateur introuvable."))?;
        if is_blocked(&state, user.id, id).await? {
            return Err(Error(StatusCode::FORBIDDEN, "Impossible d'ajouter cette personne.".into()));
        }
    }
    let name = input.name.map(|n| n.trim().chars().take(60).collect::<String>()).unwrap_or_default();
    let is_group = !name.is_empty() || others.len() > 1;
    if is_group && name.is_empty() {
        return Err(bad("Donnez un nom au groupe."));
    }
    for &id in &others {
        if !friends::are_friends(&state, user.id, id).await? {
            let message = if is_group {
                "Vous ne pouvez ajouter que vos amis."
            } else {
                "Vous devez être amis pour discuter."
            };
            return Err(Error(StatusCode::FORBIDDEN, message.into()));
        }
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

/// Leaves a group. The last member to leave deletes it.
async fn leave(
    State(state): State<AppState>,
    Auth(user): Auth,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    let members = require_member(&state, id, user.id).await?;
    let (is_group,): (bool,) = sqlx::query_as("SELECT is_group FROM conversations WHERE id = ?")
        .bind(id)
        .fetch_one(&state.db)
        .await?;
    if !is_group {
        return Err(bad("Seuls les groupes peuvent être quittés."));
    }
    let files: Vec<(String,)> = sqlx::query_as(
        "SELECT media_id FROM messages WHERE conversation_id = ? AND media_id IS NOT NULL",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM members WHERE conversation_id = ? AND user_id = ?")
        .bind(id)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    let emptied = sqlx::query(
        "DELETE FROM conversations WHERE id = ?1
         AND NOT EXISTS (SELECT 1 FROM members WHERE conversation_id = ?1)",
    )
    .bind(id)
    .execute(&mut *tx)
    .await?
    .rows_affected()
        > 0;
    tx.commit().await?;
    if emptied {
        for (file,) in files {
            media::discard(&state, &file).await?;
        }
    }
    state.send(&members, &ServerEvent::Conversation { id });
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
