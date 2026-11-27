//! goofysound (Navidrome, Subsonic API): search, shared songs, streaming, covers
//! and listening activity.

use crate::{
    auth::{now_ms, Auth},
    error::{Error, Result},
    links::{require_music, sign, verify},
    state::AppState,
};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    Json,
};
use md5::{Digest, Md5};
use proto::{Activity, ServerEvent, Track};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashMap, time::Duration};

/// Subsonic token credentials. The password itself is never kept.
#[derive(Clone, sqlx::FromRow)]
pub struct Creds {
    pub username: String,
    pub salt: String,
    pub token: String,
}

impl Creds {
    pub fn from_password(username: String, password: &str, salt: String) -> Self {
        let token = hex(&Md5::digest(format!("{password}{salt}").as_bytes()));
        Creds { username, salt, token }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, PartialEq)]
pub enum SubError {
    /// Wrong credentials (codes 40, 41, 44).
    Auth,
    /// Code 70.
    NotFound,
    /// Any other failure answered by the server.
    Failed,
    /// No answer, or not a Subsonic answer.
    Unreachable,
}

impl From<SubError> for Error {
    fn from(e: SubError) -> Self {
        match e {
            SubError::Auth => Error(
                StatusCode::BAD_REQUEST,
                "Identifiant ou mot de passe goofysound incorrect.".into(),
            ),
            SubError::NotFound => Error(StatusCode::NOT_FOUND, "Morceau introuvable.".into()),
            SubError::Failed => Error(StatusCode::BAD_GATEWAY, "goofysound a refusé la demande.".into()),
            SubError::Unreachable => {
                Error(StatusCode::BAD_GATEWAY, "goofysound ne répond pas, réessayez plus tard.".into())
            }
        }
    }
}

fn not_linked() -> Error {
    Error(StatusCode::FORBIDDEN, "Liez votre compte goofysound dans les réglages.".into())
}

fn bad_key() -> Error {
    Error(StatusCode::FORBIDDEN, "Lien invalide.".into())
}

fn endpoint(state: &AppState, method: &str) -> String {
    format!("{}/rest/{method}.view", state.config.navidrome_url)
}

/// Auth parameters. They hold a token: never log a URL built with them.
fn auth(creds: &Creds) -> [(&'static str, &str); 6] {
    [
        ("u", creds.username.as_str()),
        ("t", creds.token.as_str()),
        ("s", creds.salt.as_str()),
        ("v", "1.16.1"),
        ("c", "GoofyChat"),
        ("f", "json"),
    ]
}

/// Reads at most `limit` bytes of a body.
pub async fn read_limited(mut response: reqwest::Response, limit: usize) -> Option<Vec<u8>> {
    if response.content_length().is_some_and(|n| n > limit as u64) {
        return None;
    }
    let mut out = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if out.len() + chunk.len() > limit {
            return None;
        }
        out.extend_from_slice(&chunk);
    }
    Some(out)
}

/// The `subsonic-response` object of a successful answer.
fn check(body: &[u8]) -> std::result::Result<Value, SubError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| SubError::Unreachable)?;
    let response = value.get("subsonic-response").ok_or(SubError::Unreachable)?;
    if response["status"] == "ok" {
        return Ok(response.clone());
    }
    match response["error"]["code"].as_i64() {
        Some(40 | 41 | 44) => Err(SubError::Auth),
        Some(70) => Err(SubError::NotFound),
        _ => Err(SubError::Failed),
    }
}

async fn call(
    state: &AppState,
    creds: &Creds,
    method: &str,
    extra: &[(&str, &str)],
) -> std::result::Result<Value, SubError> {
    let sent = state.http.get(endpoint(state, method)).query(&auth(creds)).query(extra).send().await;
    let response = match sent {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("navidrome {method}: {}", e.without_url());
            return Err(SubError::Unreachable);
        }
    };
    if !response.status().is_success() {
        tracing::warn!("navidrome {method}: HTTP {}", response.status());
        return Err(if matches!(response.status().as_u16(), 401 | 403) {
            SubError::Auth
        } else {
            SubError::Unreachable
        });
    }
    let body = read_limited(response, 4 << 20).await.ok_or(SubError::Unreachable)?;
    check(&body)
}

pub async fn ping(state: &AppState, creds: &Creds) -> Result<()> {
    call(state, creds, "ping", &[]).await?;
    Ok(())
}

/// Navidrome ids are short and URL-safe; anything else is refused.
pub fn valid_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
}

pub fn track_key(state: &AppState, id: &str) -> String {
    sign(state, &format!("track:{id}"))
}

fn text(song: &Value, field: &str) -> String {
    song[field].as_str().unwrap_or_default().chars().take(200).collect()
}

fn track_of(state: &AppState, song: &Value) -> Option<Track> {
    let id = match &song["id"] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return None,
    };
    if !valid_id(&id) {
        return None;
    }
    let title = Some(text(song, "title")).filter(|t| !t.is_empty()).unwrap_or_else(|| "?".into());
    Some(Track {
        key: track_key(state, &id),
        title,
        artist: text(song, "artist"),
        album: text(song, "album"),
        duration: song["duration"].as_i64().unwrap_or(0).max(0),
        has_cover: song["coverArt"].as_str().is_some_and(|c| !c.is_empty()),
        id,
    })
}

pub async fn creds_of(state: &AppState, user: i64) -> Result<Option<Creds>> {
    Ok(sqlx::query_as("SELECT username, salt, token FROM navidrome_links WHERE user_id = ?")
        .bind(user)
        .fetch_optional(&state.db)
        .await?)
}

/// Looks a song up for a message sent by `user`.
pub async fn lookup(state: &AppState, user: i64, id: &str) -> Result<Track> {
    require_music(state)?;
    let creds = creds_of(state, user).await?.ok_or_else(not_linked)?;
    if !valid_id(id) {
        return Err(SubError::NotFound.into());
    }
    let response = call(state, &creds, "getSong", &[("id", id)]).await.map_err(|e| match e {
        // Stale credentials: the user must link again.
        SubError::Auth => not_linked(),
        e => e.into(),
    })?;
    track_of(state, &response["song"]).ok_or_else(|| SubError::NotFound.into())
}

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    q: String,
}

pub async fn search(
    State(state): State<AppState>,
    Auth(user): Auth,
    Query(query): Query<SearchQuery>,
) -> Result<Json<Vec<Track>>> {
    require_music(&state)?;
    let creds = creds_of(&state, user.id).await?.ok_or_else(not_linked)?;
    let q = query.q.trim();
    if q.is_empty() || q.chars().count() > 100 {
        return Ok(Json(vec![]));
    }
    if !state.allow(user.id.to_string(), "music-search", 60, Duration::from_secs(60)) {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "Doucement ! Patientez un instant.".into()));
    }
    let params = [("query", q), ("songCount", "30"), ("artistCount", "0"), ("albumCount", "0")];
    let response = call(&state, &creds, "search3", &params).await.map_err(|e| match e {
        SubError::Auth => not_linked(),
        e => e.into(),
    })?;
    let songs = match &response["searchResult3"]["song"] {
        Value::Array(list) => list.iter().filter_map(|s| track_of(&state, s)).take(30).collect(),
        song @ Value::Object(_) => track_of(&state, song).into_iter().collect(),
        _ => vec![],
    };
    Ok(Json(songs))
}

/// Credentials to fetch a song with: the latest sharer's first, then any linked account.
async fn candidates(state: &AppState, id: &str) -> Result<Vec<Creds>> {
    let mut list: Vec<Creds> = sqlx::query_as(
        "SELECT l.username, l.salt, l.token FROM messages m
         JOIN navidrome_links l ON l.user_id = m.sender_id
         WHERE m.track_id = ? AND m.deleted = 0 ORDER BY m.id DESC LIMIT 1",
    )
    .bind(id)
    .fetch_all(&state.db)
    .await?;
    let others: Vec<Creds> =
        sqlx::query_as("SELECT username, salt, token FROM navidrome_links ORDER BY created_at DESC LIMIT 4")
            .fetch_all(&state.db)
            .await?;
    for creds in others {
        if !list.iter().any(|c| c.username == creds.username) {
            list.push(creds);
        }
    }
    list.truncate(3);
    if list.is_empty() {
        return Err(Error(StatusCode::SERVICE_UNAVAILABLE, "Aucun compte goofysound n'est lié.".into()));
    }
    Ok(list)
}

#[derive(Deserialize)]
pub struct Signed {
    #[serde(default)]
    k: String,
    size: Option<i64>,
}

fn content_type(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase()
}

/// A Subsonic error sent instead of the expected bytes.
async fn failure(response: reqwest::Response) -> SubError {
    match read_limited(response, 64 * 1024).await {
        Some(body) => check(&body).err().unwrap_or(SubError::Failed),
        None => SubError::Unreachable,
    }
}

fn is_error_body(mime: &str) -> bool {
    mime.contains("json") || mime.contains("xml") || mime.starts_with("text/")
}

pub async fn stream(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<Signed>,
    headers: HeaderMap,
) -> Result<Response> {
    if !valid_id(&id) || !verify(&state, &format!("track:{id}"), &query.k) {
        return Err(bad_key());
    }
    require_music(&state)?;
    let range = headers.get(header::RANGE).filter(|r| r.len() < 100).cloned();
    let mut last = SubError::Auth;
    for creds in candidates(&state, &id).await? {
        let mut request = state
            .streaming
            .get(endpoint(&state, "stream"))
            .query(&auth(&creds))
            .query(&[("id", id.as_str()), ("maxBitRate", "192"), ("format", "mp3")]);
        if let Some(range) = &range {
            request = request.header(header::RANGE, range);
        }
        let response = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("navidrome stream: {}", e.without_url());
                return Err(SubError::Unreachable.into());
            }
        };
        let status = response.status();
        let mime = content_type(&response);
        if status.is_success() && is_error_body(&mime) {
            match failure(response).await {
                SubError::Auth => {
                    last = SubError::Auth;
                    continue;
                }
                e => return Err(e.into()),
            }
        }
        if !matches!(status.as_u16(), 200 | 206 | 416) {
            return Err(if status == StatusCode::NOT_FOUND { SubError::NotFound } else { SubError::Failed }.into());
        }
        let mut builder = Response::builder().status(status.as_u16());
        let out = builder.headers_mut().expect("fresh builder");
        let mime = if mime.starts_with("audio/") { mime.as_str() } else { "audio/mpeg" };
        out.insert(header::CONTENT_TYPE, HeaderValue::from_str(mime).unwrap_or(HeaderValue::from_static("audio/mpeg")));
        for name in [header::CONTENT_LENGTH, header::CONTENT_RANGE, header::ACCEPT_RANGES] {
            if let Some(v) = response.headers().get(&name) {
                out.insert(name, v.clone());
            }
        }
        out.insert(header::CACHE_CONTROL, HeaderValue::from_static("private, max-age=86400"));
        out.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        return builder.body(Body::from_stream(response.bytes_stream())).map_err(crate::error::internal);
    }
    Err(match last {
        SubError::Auth => Error(StatusCode::SERVICE_UNAVAILABLE, "Aucun compte goofysound lié ne permet de lire ce morceau.".into()),
        e => e.into(),
    })
}

/// Only image types a browser renders passively.
pub fn safe_image(mime: &str) -> Option<&'static str> {
    match mime {
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/png" => Some("image/png"),
        "image/webp" => Some("image/webp"),
        "image/gif" => Some("image/gif"),
        _ => None,
    }
}

pub fn unsupported_image() -> Error {
    Error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "Image non prise en charge.".into())
}

/// Answers an image with the headers shared by covers and thumbnails.
pub fn image_response(mime: &'static str, bytes: Vec<u8>) -> Result<Response> {
    Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "private, max-age=86400")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .body(Body::from(bytes))
        .map_err(crate::error::internal)
}

pub async fn cover(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<Signed>,
) -> Result<Response> {
    if !valid_id(&id) || !verify(&state, &format!("track:{id}"), &query.k) {
        return Err(bad_key());
    }
    require_music(&state)?;
    let size = query.size.unwrap_or(320).clamp(64, 640).to_string();
    for creds in candidates(&state, &id).await? {
        let sent = state
            .http
            .get(endpoint(&state, "getCoverArt"))
            .query(&auth(&creds))
            .query(&[("id", id.as_str()), ("size", size.as_str())])
            .send()
            .await;
        let response = match sent {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("navidrome cover: {}", e.without_url());
                return Err(SubError::Unreachable.into());
            }
        };
        if !response.status().is_success() {
            return Err(crate::error::not_found());
        }
        let mime = content_type(&response);
        if is_error_body(&mime) {
            match failure(response).await {
                SubError::Auth => continue,
                SubError::Unreachable => return Err(SubError::Unreachable.into()),
                _ => return Err(crate::error::not_found()),
            }
        }
        let mime = safe_image(&mime).ok_or_else(unsupported_image)?;
        let bytes = read_limited(response, 5 << 20).await.ok_or(SubError::Unreachable)?;
        return image_response(mime, bytes);
    }
    Err(Error(StatusCode::SERVICE_UNAVAILABLE, "Aucun compte goofysound lié ne permet d'afficher cette pochette.".into()))
}

/// The user, their friends and everyone sharing a conversation with them, minus blocks
/// either way. Also the audience of profile changes.
pub async fn audience(state: &AppState, user: i64) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT other FROM (
             SELECT b.user_id AS other FROM members a
             JOIN members b ON b.conversation_id = a.conversation_id
             WHERE a.user_id = ?1
             UNION
             SELECT CASE WHEN user_a = ?1 THEN user_b ELSE user_a END FROM friendships
             WHERE user_a = ?1 OR user_b = ?1)
         WHERE other != ?1
           AND NOT EXISTS (SELECT 1 FROM blocks
             WHERE (user_id = ?1 AND blocked_id = other) OR (user_id = other AND blocked_id = ?1))",
    )
    .bind(user)
    .fetch_all(&state.db)
    .await?;
    let mut ids: Vec<i64> = rows.into_iter().map(|r| r.0).collect();
    ids.push(user);
    Ok(ids)
}

pub async fn activity(State(state): State<AppState>, Auth(user): Auth) -> Result<Json<Vec<Activity>>> {
    // Off means nobody is listening, not an error: clients load this at start.
    if state.config.navidrome_url.is_empty() {
        return Ok(Json(vec![]));
    }
    let visible = audience(&state, user.id).await?;
    let snapshot = state.activity.lock().unwrap();
    let mut list: Vec<Activity> =
        visible.iter().filter_map(|id| snapshot.get(id).cloned()).collect();
    list.sort_by_key(|a| a.user_id);
    Ok(Json(list))
}

async fn announce(state: &AppState, user: i64, activity: Option<Activity>) {
    match audience(state, user).await {
        Ok(to) => state.send(&to, &ServerEvent::Activity { user_id: user, activity }),
        Err(e) => tracing::warn!("activity audience: {}", e.1),
    }
}

/// Called when a user unlinks: they stop showing as listening.
pub async fn forget_activity(state: &AppState, user: i64) {
    let removed = state.activity.lock().unwrap().remove(&user);
    if removed.is_some() {
        announce(state, user, None).await;
    }
}

pub async fn activity_loop(state: AppState) {
    let mut every = tokio::time::interval(Duration::from_secs(10));
    every.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        every.tick().await;
        poll_activity(&state).await;
    }
}

#[derive(sqlx::FromRow)]
struct LinkRow {
    user_id: i64,
    username: String,
    salt: String,
    token: String,
}

/// One `getNowPlaying` round: announces who started, changed or stopped.
pub async fn poll_activity(state: &AppState) {
    if state.config.navidrome_url.is_empty() {
        return;
    }
    let links: Vec<LinkRow> = match sqlx::query_as(
        "SELECT user_id, username, salt, token FROM navidrome_links ORDER BY user_id",
    )
    .fetch_all(&state.db)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("activity: {e}");
            return;
        }
    };
    let mut current: HashMap<i64, (i64, Track, String)> = HashMap::new();
    if !links.is_empty() {
        let n = links.len();
        let start = *state.poller.lock().unwrap() % n;
        let mut found = None;
        for step in 0..n.min(3) {
            let link = &links[(start + step) % n];
            let creds = Creds { username: link.username.clone(), salt: link.salt.clone(), token: link.token.clone() };
            match call(state, &creds, "getNowPlaying", &[]).await {
                Ok(response) => {
                    found = Some(response);
                    break;
                }
                // Try another account next time too.
                Err(SubError::Auth) => *state.poller.lock().unwrap() = (start + step + 1) % n,
                // Keep the last state rather than flicker while Navidrome is away.
                Err(_) => return,
            }
        }
        let Some(response) = found else { return };
        let users: HashMap<String, i64> =
            links.iter().map(|l| (l.username.to_lowercase(), l.user_id)).collect();
        let entries = match &response["nowPlaying"]["entry"] {
            Value::Array(list) => list.clone(),
            entry @ Value::Object(_) => vec![entry.clone()],
            _ => vec![],
        };
        for entry in entries {
            let minutes = entry["minutesAgo"].as_i64().unwrap_or(0);
            if minutes > 1 {
                continue;
            }
            let Some(name) = entry["username"].as_str() else { continue };
            let Some(&user) = users.get(&name.to_lowercase()) else { continue };
            let Some(track) = track_of(state, &entry) else { continue };
            let player: String = entry["playerName"].as_str().unwrap_or_default().chars().take(60).collect();
            // Several players: the most recent one wins.
            if current.get(&user).is_none_or(|(m, _, _)| minutes < *m) {
                current.insert(user, (minutes, track, player));
            }
        }
    }
    let now = now_ms();
    let mut changes = Vec::new();
    {
        let mut snapshot = state.activity.lock().unwrap();
        snapshot.retain(|user, _| {
            let keep = current.contains_key(user);
            if !keep {
                changes.push((*user, None));
            }
            keep
        });
        for (user, (_, track, player)) in current {
            let same = snapshot.get(&user).is_some_and(|a| a.track.id == track.id && a.player == player);
            if !same {
                let activity = Activity { user_id: user, track, player, since: now };
                snapshot.insert(user, activity.clone());
                changes.push((user, Some(activity)));
            }
        }
    }
    for (user, activity) in changes {
        announce(state, user, activity).await;
    }
}
