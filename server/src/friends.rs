//! Friends: requests by exact username, the friends list, and the rules that depend on
//! friendship (who can start a conversation, which one-to-one conversations are locked,
//! who can see a profile).

use crate::{
    api::is_blocked,
    auth::{now_ms, user_by_id, Auth, UserRow, USER_COLS},
    error::{bad, Error, Result},
    presence,
    state::AppState,
};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use proto::*;
use std::time::Duration;

/// A friendship row stores the smaller id first.
pub fn pair(a: i64, b: i64) -> (i64, i64) {
    (a.min(b), a.max(b))
}

/// When the two became friends, if they are.
pub async fn friends_since(state: &AppState, a: i64, b: i64) -> Result<Option<i64>> {
    let (a, b) = pair(a, b);
    let row: Option<(i64,)> = sqlx::query_as("SELECT created_at FROM friendships WHERE user_a = ? AND user_b = ?")
        .bind(a)
        .bind(b)
        .fetch_optional(&state.db)
        .await?;
    Ok(row.map(|r| r.0))
}

pub async fn are_friends(state: &AppState, a: i64, b: i64) -> Result<bool> {
    Ok(a != b && friends_since(state, a, b).await?.is_some())
}

/// The user's friends (never blocked: blocking ends a friendship).
pub async fn friend_ids(state: &AppState, user: i64) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT CASE WHEN user_a = ?1 THEN user_b ELSE user_a END FROM friendships
         WHERE user_a = ?1 OR user_b = ?1",
    )
    .bind(user)
    .fetch_all(&state.db)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// The user's devices and their friends: the audience of stories and profile changes.
pub async fn self_and_friends(state: &AppState, user: i64) -> Result<Vec<i64>> {
    let mut ids = friend_ids(state, user).await?;
    ids.push(user);
    Ok(ids)
}

/// The one-to-one conversation of a pair, if it exists.
pub async fn direct_id(state: &AppState, a: i64, b: i64) -> Result<Option<i64>> {
    if a == b {
        return Ok(None);
    }
    let (a, b) = pair(a, b);
    let row: Option<(i64,)> = sqlx::query_as("SELECT id FROM conversations WHERE direct_key = ?")
        .bind(format!("{a}:{b}"))
        .fetch_optional(&state.db)
        .await?;
    Ok(row.map(|r| r.0))
}

/// A one-to-one conversation between these two is locked: not friends, or blocked.
pub async fn locked_pair(state: &AppState, a: i64, b: i64) -> Result<bool> {
    Ok(!are_friends(state, a, b).await? || is_blocked(state, a, b).await?)
}

/// Whether `me` may see `other`'s profile: not blocked either way, and themselves, a
/// friend, a pending request either way, or someone sharing a conversation.
pub async fn visible(state: &AppState, me: i64, other: i64) -> Result<bool> {
    let found = sqlx::query(
        "SELECT 1 FROM users WHERE id = ?2
           AND NOT EXISTS (SELECT 1 FROM blocks
                WHERE (user_id = ?1 AND blocked_id = ?2) OR (user_id = ?2 AND blocked_id = ?1))
           AND (?1 = ?2
             OR EXISTS (SELECT 1 FROM friendships WHERE user_a = MIN(?1, ?2) AND user_b = MAX(?1, ?2))
             OR EXISTS (SELECT 1 FROM friend_requests
                  WHERE (from_id = ?1 AND to_id = ?2) OR (from_id = ?2 AND to_id = ?1))
             OR EXISTS (SELECT 1 FROM members a JOIN members b ON b.conversation_id = a.conversation_id
                  WHERE a.user_id = ?1 AND b.user_id = ?2))",
    )
    .bind(me)
    .bind(other)
    .fetch_optional(&state.db)
    .await?;
    Ok(found.is_some())
}

/// How `me` relates to `other`.
pub async fn relation(state: &AppState, me: i64, other: i64) -> Result<Relation> {
    if me == other {
        return Ok(Relation::Me);
    }
    if are_friends(state, me, other).await? {
        return Ok(Relation::Friend);
    }
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT from_id FROM friend_requests WHERE (from_id = ?1 AND to_id = ?2) OR (from_id = ?2 AND to_id = ?1)",
    )
    .bind(me)
    .bind(other)
    .fetch_all(&state.db)
    .await?;
    Ok(match rows.first() {
        Some((from,)) if *from == me => Relation::Outgoing,
        Some(_) => Relation::Incoming,
        None => Relation::None,
    })
}

/// Stops a call ringing between the two.
pub fn end_calls(state: &AppState, a: i64, b: i64) {
    state.calls.lock().unwrap().retain(|_, c| {
        !((c.from.id == a && c.to == b) || (c.from.id == b && c.to == a))
    });
}

/// Tells both users their one-to-one conversation changed (locked or unlocked).
pub async fn announce_direct(state: &AppState, a: i64, b: i64) -> Result<()> {
    if let Some(id) = direct_id(state, a, b).await? {
        state.send(&[a, b], &ServerEvent::Conversation { id });
    }
    Ok(())
}

fn reload(state: &AppState, users: &[i64]) {
    state.send(users, &ServerEvent::Friends { notice: None });
}

/// Deletes the friendship and the requests between two users, both ways.
/// Returns whether anything was deleted.
pub async fn separate(state: &AppState, a: i64, b: i64) -> Result<bool> {
    let (x, y) = pair(a, b);
    let mut tx = state.db.begin().await?;
    let friends = sqlx::query("DELETE FROM friendships WHERE user_a = ? AND user_b = ?")
        .bind(x)
        .bind(y)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let requests = sqlx::query(
        "DELETE FROM friend_requests WHERE (from_id = ?1 AND to_id = ?2) OR (from_id = ?2 AND to_id = ?1)",
    )
    .bind(a)
    .bind(b)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(friends + requests > 0)
}

/// Makes two users friends and removes the requests between them.
async fn befriend(state: &AppState, a: i64, b: i64) -> Result<()> {
    let (x, y) = pair(a, b);
    let mut tx = state.db.begin().await?;
    sqlx::query("INSERT OR IGNORE INTO friendships (user_a, user_b, created_at) VALUES (?, ?, ?)")
        .bind(x)
        .bind(y)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "DELETE FROM friend_requests WHERE (from_id = ?1 AND to_id = ?2) OR (from_id = ?2 AND to_id = ?1)",
    )
    .bind(a)
    .bind(b)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn friend_of(state: &AppState, me: i64, user: User, since: i64) -> Result<Friend> {
    Ok(Friend {
        online: presence::is_online(state, user.id),
        direct_id: direct_id(state, me, user.id).await?,
        user,
        since,
    })
}

#[derive(sqlx::FromRow)]
struct DatedUser {
    #[sqlx(flatten)]
    user: UserRow,
    at: i64,
}

pub async fn list(State(state): State<AppState>, Auth(me): Auth) -> Result<Json<Vec<Friend>>> {
    let rows: Vec<DatedUser> = sqlx::query_as(&format!(
        "SELECT {USER_COLS}, f.created_at AS at FROM friendships f
         JOIN users u ON u.id = CASE WHEN f.user_a = ?1 THEN f.user_b ELSE f.user_a END
         WHERE f.user_a = ?1 OR f.user_b = ?1
         ORDER BY u.display_name COLLATE NOCASE, u.username"
    ))
    .bind(me.id)
    .fetch_all(&state.db)
    .await?;
    let mut list = Vec::with_capacity(rows.len());
    for row in rows {
        list.push(friend_of(&state, me.id, row.user.into(), row.at).await?);
    }
    Ok(Json(list))
}

pub async fn requests(State(state): State<AppState>, Auth(me): Auth) -> Result<Json<FriendRequests>> {
    let query = |other: &str, mine: &str| {
        format!(
            "SELECT {USER_COLS}, r.created_at AS at FROM friend_requests r
             JOIN users u ON u.id = r.{other}
             WHERE r.{mine} = ?1
               AND NOT EXISTS (SELECT 1 FROM blocks
                    WHERE (user_id = ?1 AND blocked_id = u.id) OR (user_id = u.id AND blocked_id = ?1))
             ORDER BY r.created_at DESC, u.id DESC"
        )
    };
    let load = |sql: String| {
        let state = state.clone();
        async move {
            let rows: Vec<DatedUser> = sqlx::query_as(&sql).bind(me.id).fetch_all(&state.db).await?;
            Ok::<_, Error>(
                rows.into_iter()
                    .map(|r| FriendRequest { user: r.user.into(), created_at: r.at })
                    .collect::<Vec<_>>(),
            )
        }
    };
    Ok(Json(FriendRequests {
        incoming: load(query("from_id", "to_id")).await?,
        outgoing: load(query("to_id", "from_id")).await?,
    }))
}

/// Trim, one leading "@" removed, lowercase; `None` unless 3 to 24 of `[a-z0-9_.]`.
pub fn normalize_username(input: &str) -> Option<String> {
    let name = input.trim();
    let name = name.strip_prefix('@').unwrap_or(name).to_lowercase();
    let valid = (3..=24).contains(&name.len())
        && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.');
    valid.then_some(name)
}

fn no_such_member() -> Error {
    Error(StatusCode::NOT_FOUND, "Aucun membre avec cet identifiant.".into())
}

pub async fn send_request(
    State(state): State<AppState>,
    Auth(me): Auth,
    Json(input): Json<NewFriendRequest>,
) -> Result<Json<FriendRequestOutcome>> {
    let username = normalize_username(&input.username).ok_or_else(|| bad("Identifiant invalide."))?;
    if username == me.username {
        return Err(bad("C'est votre propre identifiant."));
    }
    if !state.allow(me.id.to_string(), "friend-request", 30, Duration::from_secs(3600)) {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "Trop de demandes, patientez.".into()));
    }
    let found: Option<(i64,)> = sqlx::query_as("SELECT id FROM users WHERE username = ?")
        .bind(&username)
        .fetch_optional(&state.db)
        .await?;
    let (target,) = found.ok_or_else(no_such_member)?;
    // Blocked either way looks exactly like a missing username.
    if is_blocked(&state, me.id, target).await? {
        return Err(no_such_member());
    }
    let user = user_by_id(&state, target).await?.ok_or_else(no_such_member)?;
    if are_friends(&state, me.id, target).await? {
        return Err(Error(StatusCode::CONFLICT, "Vous êtes déjà amis.".into()));
    }
    let asked_me = sqlx::query("SELECT 1 FROM friend_requests WHERE from_id = ? AND to_id = ?")
        .bind(target)
        .bind(me.id)
        .fetch_optional(&state.db)
        .await?
        .is_some();
    if asked_me {
        befriend(&state, me.id, target).await?;
        let notice = FriendNotice { kind: FriendNoticeKind::Accepted, user: me.clone() };
        state.send(&[target], &ServerEvent::Friends { notice: Some(notice) });
        reload(&state, &[me.id]);
        announce_direct(&state, me.id, target).await?;
        return Ok(Json(FriendRequestOutcome { status: FriendRequestStatus::Friends, user }));
    }
    let pending = sqlx::query("SELECT 1 FROM friend_requests WHERE from_id = ? AND to_id = ?")
        .bind(me.id)
        .bind(target)
        .fetch_optional(&state.db)
        .await?
        .is_some();
    if pending {
        return Ok(Json(FriendRequestOutcome { status: FriendRequestStatus::Sent, user }));
    }
    let (outgoing,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM friend_requests WHERE from_id = ?")
        .bind(me.id)
        .fetch_one(&state.db)
        .await?;
    if outgoing as usize >= limits::PENDING_REQUESTS {
        return Err(bad("Trop de demandes en attente."));
    }
    sqlx::query("INSERT OR IGNORE INTO friend_requests (from_id, to_id, created_at) VALUES (?, ?, ?)")
        .bind(me.id)
        .bind(target)
        .bind(now_ms())
        .execute(&state.db)
        .await?;
    let notice = FriendNotice { kind: FriendNoticeKind::Request, user: me.clone() };
    state.send(&[target], &ServerEvent::Friends { notice: Some(notice) });
    reload(&state, &[me.id]);
    Ok(Json(FriendRequestOutcome { status: FriendRequestStatus::Sent, user }))
}

pub async fn accept(
    State(state): State<AppState>,
    Auth(me): Auth,
    Path(from): Path<i64>,
) -> Result<Json<Friend>> {
    let missing = || Error(StatusCode::NOT_FOUND, "Demande introuvable.".into());
    sqlx::query("SELECT 1 FROM friend_requests WHERE from_id = ? AND to_id = ?")
        .bind(from)
        .bind(me.id)
        .fetch_optional(&state.db)
        .await?
        .ok_or_else(missing)?;
    let user = user_by_id(&state, from).await?.ok_or_else(missing)?;
    befriend(&state, me.id, from).await?;
    let notice = FriendNotice { kind: FriendNoticeKind::Accepted, user: me.clone() };
    state.send(&[from], &ServerEvent::Friends { notice: Some(notice) });
    reload(&state, &[me.id]);
    announce_direct(&state, me.id, from).await?;
    let since = friends_since(&state, me.id, from).await?.unwrap_or_else(now_ms);
    Ok(Json(friend_of(&state, me.id, user, since).await?))
}

/// Deletes one request; both users reload when there was one.
async fn drop_request(state: &AppState, from: i64, to: i64) -> Result<StatusCode> {
    let removed = sqlx::query("DELETE FROM friend_requests WHERE from_id = ? AND to_id = ?")
        .bind(from)
        .bind(to)
        .execute(&state.db)
        .await?
        .rows_affected();
    if removed > 0 {
        reload(state, &[from, to]);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn decline(
    State(state): State<AppState>,
    Auth(me): Auth,
    Path(from): Path<i64>,
) -> Result<StatusCode> {
    drop_request(&state, from, me.id).await
}

pub async fn cancel(
    State(state): State<AppState>,
    Auth(me): Auth,
    Path(to): Path<i64>,
) -> Result<StatusCode> {
    drop_request(&state, me.id, to).await
}

pub async fn remove(
    State(state): State<AppState>,
    Auth(me): Auth,
    Path(other): Path<i64>,
) -> Result<StatusCode> {
    if user_by_id(&state, other).await?.is_none() {
        return Err(Error(StatusCode::NOT_FOUND, "Utilisateur introuvable.".into()));
    }
    let (a, b) = pair(me.id, other);
    let removed = sqlx::query("DELETE FROM friendships WHERE user_a = ? AND user_b = ?")
        .bind(a)
        .bind(b)
        .execute(&state.db)
        .await?
        .rows_affected();
    if removed > 0 {
        end_calls(&state, me.id, other);
        reload(&state, &[me.id, other]);
        announce_direct(&state, me.id, other).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}
