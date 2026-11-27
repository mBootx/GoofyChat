//! Profiles: editing one's own (photo, banner, bio, name font and colour), viewing
//! someone's, the photos shared with them, and the friends search.

use crate::{
    account::{announce, conversations_of},
    auth::{user_by_id, Auth, UserRow, USER_COLS},
    error::{bad, Error, Result},
    friends, imaging, media, music, presence,
    state::AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use proto::*;
use serde::Deserialize;
use std::time::Duration;

/// Trimmed, with Windows line ends folded; at most 190 characters and 6 lines.
pub fn normalize_bio(input: &str) -> Option<String> {
    let bio = input.replace("\r\n", "\n").replace('\r', "\n");
    let bio = bio.trim().to_string();
    let ok = bio.chars().count() <= limits::BIO_CHARS && bio.split('\n').count() <= limits::BIO_LINES;
    ok.then_some(bio)
}

/// Mime and size of a media row owned by `user`.
async fn owned(state: &AppState, user: i64, id: &str) -> Result<Option<(String, i64)>> {
    Ok(sqlx::query_as("SELECT mime, size FROM media WHERE id = ? AND owner_id = ?")
        .bind(id)
        .bind(user)
        .fetch_optional(&state.db)
        .await?)
}

/// Records the format found by decoding, when the uploader declared another one.
async fn fix_mime(state: &AppState, id: &str, declared: &str, info: &imaging::Info) -> Result<()> {
    if declared != info.mime() {
        sqlx::query("UPDATE media SET mime = ? WHERE id = ?").bind(info.mime()).bind(id).execute(&state.db).await?;
    }
    Ok(())
}

/// Checks a new profile photo; returns the PNG still when it is animated.
async fn check_avatar(state: &AppState, user: i64, id: &str) -> Result<Option<Vec<u8>>> {
    let refused = || bad("Choisissez une image JPEG, PNG, WebP ou GIF de 8 Mo au plus.");
    let (mime, size) = owned(state, user, id).await?.ok_or_else(refused)?;
    if !matches!(mime.as_str(), "image/jpeg" | "image/png" | "image/webp" | "image/gif")
        || size > limits::AVATAR_BYTES
    {
        return Err(refused());
    }
    let bytes = media::read(state, id).await?;
    let checked = imaging::run(state, move || imaging::avatar(&bytes)).await?.ok_or_else(refused)?;
    fix_mime(state, id, &mime, &checked.info).await?;
    Ok(checked.still)
}

async fn check_banner(state: &AppState, user: i64, id: &str) -> Result<()> {
    let refused = || bad("Choisissez une bannière JPEG, PNG ou WebP de 8 Mo au plus.");
    let (mime, size) = owned(state, user, id).await?.ok_or_else(refused)?;
    if !matches!(mime.as_str(), "image/jpeg" | "image/png" | "image/webp") || size > limits::BANNER_BYTES {
        return Err(refused());
    }
    let bytes = media::read(state, id).await?;
    let info = imaging::run(state, move || {
        imaging::probe(&bytes).filter(|i| i.format != image::ImageFormat::WebP || !imaging::webp_animated(&bytes))
    })
    .await?
    .filter(|i| {
        i.format != image::ImageFormat::Gif && i.width <= limits::IMAGE_SIDE && i.height <= limits::IMAGE_SIDE
    })
    .ok_or_else(refused)?;
    fix_mime(state, id, &mime, &info).await
}

pub async fn update_profile(
    State(state): State<AppState>,
    Auth(user): Auth,
    Json(input): Json<Profile>,
) -> Result<Json<User>> {
    let display_name = input.display_name.trim().to_string();
    if !(1..=40).contains(&display_name.chars().count()) {
        return Err(bad("Nom affiché : 1 à 40 caractères."));
    }
    let bio = match &input.bio {
        Some(b) => Some(normalize_bio(b).ok_or_else(|| bad("Bio : 190 caractères et 6 lignes au plus."))?),
        None => None,
    };
    if input.name_font.as_deref().is_some_and(|f| !valid_font(f)) {
        return Err(bad("Police inconnue."));
    }
    let name_color = input.name_color.map(|c| c.to_ascii_lowercase());
    if name_color.as_deref().is_some_and(|c| !valid_name_color(c)) {
        return Err(bad("Couleur inconnue."));
    }
    if !state.allow(user.id.to_string(), "profile", 20, Duration::from_secs(3600)) {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "Trop de changements, patientez.".into()));
    }

    let (old_still, old_banner): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT avatar_still, banner FROM users WHERE id = ?")
            .bind(user.id)
            .fetch_one(&state.db)
            .await?;
    let old_avatar = user.avatar.clone();
    let (mut avatar, mut still) = (old_avatar.clone(), old_still.clone());
    if input.remove_avatar {
        (avatar, still) = (None, None);
    } else if let Some(id) = input.avatar_id.filter(|id| old_avatar.as_ref() != Some(id)) {
        still = match check_avatar(&state, user.id, &id).await? {
            Some(png) => Some(media::store(&state, user.id, "image/png", "avatar-still.png", &png).await?),
            None => None,
        };
        avatar = Some(id);
    }
    let mut banner = old_banner.clone();
    if input.remove_banner {
        banner = None;
    } else if let Some(id) = input.banner_id.filter(|id| old_banner.as_ref() != Some(id)) {
        check_banner(&state, user.id, &id).await?;
        banner = Some(id);
    }

    sqlx::query(
        "UPDATE users SET display_name = ?, avatar = ?, avatar_still = ?, banner = ?,
           bio = COALESCE(?, bio), name_font = COALESCE(?, name_font), name_color = COALESCE(?, name_color)
         WHERE id = ?",
    )
    .bind(&display_name)
    .bind(&avatar)
    .bind(&still)
    .bind(&banner)
    .bind(&bio)
    .bind(&input.name_font)
    .bind(&name_color)
    .bind(user.id)
    .execute(&state.db)
    .await?;
    for (old, new) in [(old_avatar, &avatar), (old_still, &still), (old_banner, &banner)] {
        if let Some(old) = old.filter(|old| new.as_ref() != Some(old)) {
            media::discard(&state, &old).await?;
        }
    }
    let user = user_by_id(&state, user.id).await?.ok_or_else(crate::error::unauthorized)?;
    let audience = music::audience(&state, user.id).await?;
    state.send(&audience, &ServerEvent::UserChanged { user: user.clone() });
    announce(&state, &conversations_of(&state, user.id).await?).await?;
    Ok(Json(user))
}

fn no_user() -> Error {
    Error(StatusCode::NOT_FOUND, "Utilisateur introuvable.".into())
}

pub async fn profile(
    State(state): State<AppState>,
    Auth(me): Auth,
    Path(id): Path<i64>,
) -> Result<Json<UserProfile>> {
    if !friends::visible(&state, me.id, id).await? {
        return Err(no_user());
    }
    let user = user_by_id(&state, id).await?.ok_or_else(no_user)?;
    let (bio, banner, joined_at): (String, Option<String>, i64) =
        sqlx::query_as("SELECT bio, banner, created_at FROM users WHERE id = ?")
            .bind(id)
            .fetch_one(&state.db)
            .await?;
    let relation = friends::relation(&state, me.id, id).await?;
    let friends_since = match relation {
        Relation::Friend => friends::friends_since(&state, me.id, id).await?,
        _ => None,
    };
    let online = matches!(relation, Relation::Friend | Relation::Me) && presence::is_online(&state, id);
    Ok(Json(UserProfile {
        user,
        bio,
        banner,
        relation,
        friends_since,
        joined_at,
        online,
        direct_id: friends::direct_id(&state, me.id, id).await?,
    }))
}

#[derive(Deserialize)]
pub struct Before {
    before: Option<i64>,
}

const MEDIA_PAGE: usize = 60;

/// Photos and videos of the one-to-one conversation with someone, newest first.
pub async fn shared_media(
    State(state): State<AppState>,
    Auth(me): Auth,
    Path(id): Path<i64>,
    Query(page): Query<Before>,
) -> Result<Json<SharedMediaPage>> {
    if !friends::visible(&state, me.id, id).await? {
        return Err(no_user());
    }
    let Some(conversation) = friends::direct_id(&state, me.id, id).await? else {
        return Ok(Json(SharedMediaPage::default()));
    };
    let rows: Vec<(i64, i64, String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT m.id, m.sender_id, f.id, f.mime, f.name, f.size, m.created_at
         FROM messages m JOIN media f ON f.id = m.media_id
         WHERE m.conversation_id = ? AND m.deleted = 0 AND m.id < ?
           AND (f.mime LIKE 'image/%' OR f.mime LIKE 'video/%')
         ORDER BY m.id DESC LIMIT ?",
    )
    .bind(conversation)
    .bind(page.before.unwrap_or(i64::MAX))
    .bind(MEDIA_PAGE as i64)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<SharedMedia> = rows
        .into_iter()
        .map(|(message_id, sender_id, id, mime, name, size, created_at)| SharedMedia {
            message_id,
            sender_id,
            media: Media { id, mime, name, size },
            created_at,
        })
        .collect();
    let next = (items.len() == MEDIA_PAGE).then(|| items.last().map(|i| i.message_id)).flatten();
    Ok(Json(SharedMediaPage { items, next }))
}

#[derive(Deserialize)]
pub struct Search {
    /// Empty lists every friend.
    #[serde(default)]
    q: String,
}

/// The caller's friends, optionally filtered by part of the username or display name.
pub async fn search_users(
    State(state): State<AppState>,
    Auth(user): Auth,
    Query(search): Query<Search>,
) -> Result<Json<Vec<User>>> {
    let q = search.q.trim().to_lowercase();
    if q.chars().count() > 40 {
        return Ok(Json(vec![]));
    }
    let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    let rows: Vec<UserRow> = sqlx::query_as(&format!(
        "SELECT {USER_COLS} FROM friendships f
         JOIN users u ON u.id = CASE WHEN f.user_a = ?1 THEN f.user_b ELSE f.user_a END
         WHERE (f.user_a = ?1 OR f.user_b = ?1)
           AND (u.username LIKE ?2 ESCAPE '\\' OR u.display_name LIKE ?2 ESCAPE '\\')
         ORDER BY u.display_name COLLATE NOCASE, u.username LIMIT 100"
    ))
    .bind(user.id)
    .bind(format!("%{escaped}%"))
    .fetch_all(&state.db)
    .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}
