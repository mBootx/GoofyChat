//! Profile, blocked users and account deletion.

use crate::{
    api::member_ids,
    auth::{check_password, Auth, UserRow},
    error::{bad, not_found, Error, Result},
    media,
    state::AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use proto::*;
use std::time::Duration;

/// Asks everyone sharing these conversations to reload them.
async fn announce(state: &AppState, conversations: &[i64]) -> Result<()> {
    for &id in conversations {
        let members = member_ids(state, id).await?;
        state.send(&members, &ServerEvent::Conversation { id });
    }
    Ok(())
}

async fn conversations_of(state: &AppState, user: i64) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as("SELECT conversation_id FROM members WHERE user_id = ?")
        .bind(user)
        .fetch_all(&state.db)
        .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

pub async fn update_profile(
    State(state): State<AppState>,
    Auth(mut user): Auth,
    Json(input): Json<Profile>,
) -> Result<Json<User>> {
    let display_name = input.display_name.trim().to_string();
    if !(1..=40).contains(&display_name.chars().count()) {
        return Err(bad("Nom affiché : 1 à 40 caractères."));
    }
    if !state.allow(user.id.to_string(), "profile", 20, Duration::from_secs(3600)) {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "Trop de changements, patientez.".into()));
    }
    let old = user.avatar.clone();
    if input.remove_avatar {
        user.avatar = None;
    } else if let Some(id) = input.avatar_id {
        sqlx::query("SELECT 1 FROM media WHERE id = ? AND owner_id = ? AND mime LIKE 'image/%'")
            .bind(&id)
            .bind(user.id)
            .fetch_optional(&state.db)
            .await?
            .ok_or_else(|| bad("Choisissez une image."))?;
        user.avatar = Some(id);
    }
    user.display_name = display_name;
    sqlx::query("UPDATE users SET display_name = ?, avatar = ? WHERE id = ?")
        .bind(&user.display_name)
        .bind(&user.avatar)
        .bind(user.id)
        .execute(&state.db)
        .await?;
    if let Some(old) = old.filter(|old| user.avatar.as_ref() != Some(old)) {
        media::discard(&state, &old).await?;
    }
    announce(&state, &conversations_of(&state, user.id).await?).await?;
    Ok(Json(user))
}

pub async fn blocked(State(state): State<AppState>, Auth(user): Auth) -> Result<Json<Vec<User>>> {
    let rows: Vec<UserRow> = sqlx::query_as(
        "SELECT u.id, u.username, u.display_name, u.avatar FROM blocks b
         JOIN users u ON u.id = b.blocked_id WHERE b.user_id = ? ORDER BY u.username",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// Tells the user's own devices only: the other person is not informed.
fn refresh_own(state: &AppState, user: i64) {
    state.send(&[user], &ServerEvent::Conversation { id: 0 });
}

pub async fn block(
    State(state): State<AppState>,
    Auth(user): Auth,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    if id == user.id {
        return Err(bad("Impossible de vous bloquer."));
    }
    sqlx::query("SELECT 1 FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(not_found)?;
    sqlx::query("INSERT OR IGNORE INTO blocks (user_id, blocked_id) VALUES (?, ?)")
        .bind(user.id)
        .bind(id)
        .execute(&state.db)
        .await?;
    // Stop a call that is currently ringing between the two.
    state.calls.lock().unwrap().retain(|_, c| {
        !((c.from.id == id && c.to == user.id) || (c.from.id == user.id && c.to == id))
    });
    refresh_own(&state, user.id);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unblock(
    State(state): State<AppState>,
    Auth(user): Auth,
    Path(id): Path<i64>,
) -> Result<StatusCode> {
    sqlx::query("DELETE FROM blocks WHERE user_id = ? AND blocked_id = ?")
        .bind(user.id)
        .bind(id)
        .execute(&state.db)
        .await?;
    refresh_own(&state, user.id);
    Ok(StatusCode::NO_CONTENT)
}

/// Removes the account, its messages and files, and its one-to-one conversations.
pub async fn delete_account(
    State(state): State<AppState>,
    Auth(user): Auth,
    Json(input): Json<DeleteAccount>,
) -> Result<StatusCode> {
    if input.password.len() > 128
        || !state.allow(user.id.to_string(), "delete-account", 5, Duration::from_secs(3600))
    {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "Trop de tentatives, réessayez plus tard.".into()));
    }
    let (hash,): (String,) = sqlx::query_as("SELECT password_hash FROM users WHERE id = ?")
        .bind(user.id)
        .fetch_one(&state.db)
        .await?;
    if !check_password(&state, input.password, hash).await? {
        // Not 401, which would sign the app out.
        return Err(Error(StatusCode::FORBIDDEN, "Mot de passe incorrect.".into()));
    }

    let conversations = conversations_of(&state, user.id).await?;
    let mut others = Vec::new();
    for &id in &conversations {
        others.push((id, member_ids(&state, id).await?));
    }
    let own: Vec<(String,)> = sqlx::query_as("SELECT id FROM media WHERE owner_id = ?")
        .bind(user.id)
        .fetch_all(&state.db)
        .await?;
    // Files other people sent in the one-to-one conversations about to disappear.
    let received: Vec<(String,)> = sqlx::query_as(
        "SELECT x.media_id FROM messages x
         JOIN conversations c ON c.id = x.conversation_id
         JOIN members me ON me.conversation_id = c.id AND me.user_id = ?1
         WHERE c.is_group = 0 AND x.sender_id != ?1 AND x.media_id IS NOT NULL",
    )
    .bind(user.id)
    .fetch_all(&state.db)
    .await?;

    let mut tx = state.db.begin().await?;
    sqlx::query(
        "DELETE FROM conversations WHERE is_group = 0
         AND id IN (SELECT conversation_id FROM members WHERE user_id = ?)",
    )
    .bind(user.id)
    .execute(&mut *tx)
    .await?;
    // Cascades to sessions, memberships, messages, reactions, blocks and media rows.
    sqlx::query("DELETE FROM users WHERE id = ?").bind(user.id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM conversations WHERE id NOT IN (SELECT conversation_id FROM members)")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;

    for (id,) in own {
        media::remove_file(&state, &id).await;
    }
    for (id,) in received {
        media::discard(&state, &id).await?;
    }
    state.calls.lock().unwrap().retain(|_, c| c.from.id != user.id && c.to != user.id);
    // Dropping the senders closes the sockets.
    state.sockets.lock().unwrap().remove(&user.id);
    for (id, mut members) in others {
        members.retain(|&m| m != user.id);
        state.send(&members, &ServerEvent::Conversation { id });
    }
    Ok(StatusCode::NO_CONTENT)
}
