//! SelfHost-hub camera backups on FileBrowser Quantum: listing, thumbnails and import.

use crate::{
    auth::{now_ms, random_hex, Auth},
    error::{bad, internal, Error, Result},
    links::{decrypt, require_photos, sign, verify},
    media::{self, Busy, Unfinished},
    music::{image_response, read_limited, safe_image, unsupported_image},
    state::{AppState, FileSession},
};
use axum::{
    extract::{Query, State},
    http::{header, StatusCode},
    response::Response,
    Json,
};
use proto::{ImportPhoto, Media, Photo, PhotoPage};
use serde::Deserialize;
use serde_json::Value;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;

const PAGE: usize = 120;
const TRASH: &str = "Corbeille";

/// A `YYYY/MM` folder of the backup.
#[derive(Clone, Debug)]
pub struct Month {
    /// year * 100 + month
    pub ym: u32,
    pub path: String,
}

fn not_linked() -> Error {
    Error(StatusCode::FORBIDDEN, "Liez vos photos dans les réglages.".into())
}

fn unreachable() -> Error {
    Error(StatusCode::BAD_GATEWAY, "Le serveur de photos ne répond pas, réessayez plus tard.".into())
}

fn bad_path() -> Error {
    bad("Chemin invalide.")
}

/// encodeURIComponent, as SelfHost-hub sends the password header.
fn encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The account name as a single folder name.
fn sanitize_username(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let clean = clean.trim().trim_matches('.').trim().to_string();
    if clean.is_empty() { "user".into() } else { clean }
}

/// Whether a FileBrowser path is absolute and free of traversal tricks.
fn clean_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 1024
        && !path.contains('\\')
        && !path.contains("//")
        && !path.chars().any(char::is_control)
        && path[1..].split('/').all(|s| !s.is_empty() && s != "." && s != "..")
}

/// A file path inside the user's photos root.
pub fn check_path(root: &str, path: &str) -> Result<()> {
    let inside = path.strip_prefix(root).is_some_and(|rest| rest.starts_with('/') && rest.len() > 1);
    if !clean_path(path) || !inside {
        return Err(bad_path());
    }
    Ok(())
}

/// Photos and videos Goofy Chat accepts, by extension.
pub fn mime_of(name: &str) -> Option<&'static str> {
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "heif" => "image/heif",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "webm" => "video/webm",
        "3gp" => "video/3gpp",
        "mkv" => "video/x-matroska",
        _ => return None,
    })
}

fn base(state: &AppState) -> &str {
    &state.config.filebrowser_url
}

fn jwt_from(response: &reqwest::Response) -> Option<String> {
    response.headers().get_all(header::SET_COOKIE).iter().find_map(|v| {
        let v = v.to_str().ok()?;
        let rest = v.split(';').next()?.trim().strip_prefix("filebrowser_quantum_jwt=")?;
        (!rest.is_empty()).then(|| rest.to_string())
    })
}

fn looks_like_jwt(s: &str) -> bool {
    s.split('.').count() == 3
        && s.len() < 8192
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'='))
}

async fn get_json(state: &AppState, url: String, jwt: &str, query: &[(&str, &str)]) -> Option<Value> {
    let response = state.http.get(url).bearer_auth(jwt).query(query).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    serde_json::from_slice(&read_limited(response, 1 << 20).await?).ok()
}

fn name_of(item: &Value) -> Option<String> {
    match item {
        Value::String(s) => Some(s.clone()),
        Value::Object(_) => item["name"].as_str().map(str::to_string),
        _ => None,
    }
    .filter(|s| !s.is_empty())
}

/// Logs into FileBrowser and resolves the source and photos root.
pub async fn login(state: &AppState, username: &str, password: &str) -> Result<FileSession> {
    let sent = state
        .http
        .post(format!("{}/api/auth/login", base(state)))
        .query(&[("username", username), ("recaptcha", "")])
        .header("X-Password", encode_component(password))
        .header("X-Secret", "")
        .send()
        .await;
    let response = sent.map_err(|e| {
        tracing::warn!("filebrowser login: {}", e.without_url());
        unreachable()
    })?;
    if matches!(response.status().as_u16(), 401 | 403) {
        return Err(bad("Identifiant ou mot de passe des photos incorrect."));
    }
    if !response.status().is_success() {
        tracing::warn!("filebrowser login: HTTP {}", response.status());
        return Err(unreachable());
    }
    let jwt = match jwt_from(&response) {
        Some(jwt) => jwt,
        // Older versions answer the token in the body.
        None => {
            let body = read_limited(response, 16 * 1024).await.unwrap_or_default();
            let body = String::from_utf8_lossy(&body).trim().trim_matches('"').to_string();
            if !looks_like_jwt(&body) {
                return Err(bad("La double authentification n'est pas prise en charge."));
            }
            body
        }
    };
    let me = get_json(state, format!("{}/api/users", base(state)), &jwt, &[("id", "self")]).await;
    let account = me
        .as_ref()
        .and_then(|m| m["username"].as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(username)
        .to_string();
    let mut source = me.as_ref().and_then(|m| m["scopes"].get(0)).and_then(name_of);
    if source.is_none() {
        let sources = get_json(state, format!("{}/api/sources", base(state)), &jwt, &[]).await;
        source = sources.as_ref().and_then(|s| s.get(0)).and_then(name_of);
    }
    let root = state.config.photos_folder.replace("{user}", &sanitize_username(&account));
    let root = root.trim_end_matches('/').to_string();
    if !clean_path(&root) {
        tracing::error!("PHOTOS_FOLDER gives an invalid path");
        return Err(internal("invalid photos folder"));
    }
    Ok(FileSession { jwt, source: source.unwrap_or_else(|| "default".into()), root })
}

/// The cached login, or a new one with the stored password.
async fn session(state: &AppState, user: i64) -> Result<FileSession> {
    if let Some(found) = state.files.lock().unwrap().get(&user).cloned() {
        return Ok(found);
    }
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT username, password FROM photo_links WHERE user_id = ?")
            .bind(user)
            .fetch_optional(&state.db)
            .await?;
    let (username, sealed) = row.ok_or_else(not_linked)?;
    let password = decrypt(state, user, &sealed).ok_or_else(not_linked)?;
    let fresh = login(state, &username, &password).await.map_err(|e| {
        if e.0 == StatusCode::BAD_REQUEST {
            Error(StatusCode::FORBIDDEN, "Vos identifiants photos ne sont plus valides, liez-les à nouveau.".into())
        } else {
            e
        }
    })?;
    state.files.lock().unwrap().insert(user, fresh.clone());
    Ok(fresh)
}

/// Sends a request with the user's login, logging in again once when it expired.
async fn send(
    state: &AppState,
    user: i64,
    client: &reqwest::Client,
    build: impl Fn(&reqwest::Client, &FileSession) -> reqwest::RequestBuilder,
) -> Result<(reqwest::Response, FileSession)> {
    for attempt in 0..2 {
        let session = session(state, user).await?;
        let response = build(client, &session).bearer_auth(&session.jwt).send().await.map_err(|e| {
            tracing::warn!("filebrowser: {}", e.without_url());
            unreachable()
        })?;
        if attempt == 0 && matches!(response.status().as_u16(), 401 | 403) {
            state.files.lock().unwrap().remove(&user);
            continue;
        }
        return Ok((response, session));
    }
    Err(unreachable())
}

struct Listing {
    folders: Vec<String>,
    files: Vec<(String, i64, i64)>,
}

fn safe_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.starts_with('.')
        && !name.contains('/')
        && !name.contains('\\')
        && !name.chars().any(char::is_control)
}

fn parse_listing(value: &Value) -> Listing {
    let mut listing = Listing { folders: vec![], files: vec![] };
    let mut add = |item: &Value, dir: bool| {
        let Some(name) = item["name"].as_str().filter(|n| safe_name(n)) else { return };
        if dir {
            listing.folders.push(name.to_string());
        } else {
            let modified = match item.get("modified").or_else(|| item.get("mtime")) {
                Some(Value::String(s)) => parse_time(s).unwrap_or(0),
                Some(Value::Number(n)) => n.as_i64().map(|t| if t < 100_000_000_000 { t * 1000 } else { t }).unwrap_or(0),
                _ => 0,
            };
            listing.files.push((name.to_string(), item["size"].as_i64().unwrap_or(0), modified));
        }
    };
    for item in value["folders"].as_array().into_iter().flatten() {
        add(item, true);
    }
    for item in value["files"].as_array().into_iter().flatten() {
        add(item, item["isDir"].as_bool().unwrap_or(false) || item["type"] == "directory");
    }
    // Legacy FileBrowser.
    for item in value["items"].as_array().into_iter().flatten() {
        add(item, item["isDir"].as_bool().unwrap_or(false));
    }
    listing
}

async fn list(state: &AppState, user: i64, path: &str) -> Result<Option<Listing>> {
    let url = format!("{}/api/resources", base(state));
    let (response, _) = send(state, user, &state.http, |client, s| {
        client.get(&url).query(&[("path", path), ("source", s.source.as_str())])
    })
    .await?;
    if response.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !response.status().is_success() {
        tracing::warn!("filebrowser list: HTTP {}", response.status());
        return Err(unreachable());
    }
    let body = read_limited(response, 16 << 20).await.ok_or_else(unreachable)?;
    let value: Value = serde_json::from_slice(&body).map_err(|_| unreachable())?;
    Ok(Some(parse_listing(&value)))
}

fn year_of(name: &str) -> Option<u32> {
    (name.len() == 4 && name.bytes().all(|b| b.is_ascii_digit())).then(|| name.parse().ok()).flatten()
}

fn month_of(name: &str) -> Option<u32> {
    (name.len() == 2 && name.bytes().all(|b| b.is_ascii_digit()))
        .then(|| name.parse().ok())
        .flatten()
        .filter(|m| (1..=12).contains(m))
}

/// Every `[Album/]YYYY/MM` folder, newest first. Cached for a minute between pages.
async fn months(state: &AppState, user: i64, root: &str) -> Result<Arc<Vec<Month>>> {
    if let Some((at, found)) = state.months.lock().unwrap().get(&user) {
        if at.elapsed() < Duration::from_secs(60) {
            return Ok(found.clone());
        }
    }
    let mut found = Vec::new();
    let mut budget = 500usize;
    let Some(top) = list(state, user, root).await? else {
        return Ok(Arc::new(found));
    };
    let mut parents: Vec<String> = Vec::new();
    for name in &top.folders {
        if year_of(name).is_some() {
            parents.push(root.to_string());
            break;
        }
    }
    for name in top.folders.iter().filter(|n| year_of(n).is_none() && *n != TRASH) {
        parents.push(format!("{root}/{name}"));
    }
    for parent in parents {
        let listing = if parent == root {
            Some(Listing { folders: top.folders.clone(), files: vec![] })
        } else {
            budget = budget.saturating_sub(1);
            if budget == 0 {
                break;
            }
            list(state, user, &parent).await?
        };
        let Some(listing) = listing else { continue };
        for year_name in listing.folders.iter().filter(|n| year_of(n).is_some()) {
            budget = budget.saturating_sub(1);
            if budget == 0 {
                break;
            }
            let year = year_of(year_name).unwrap_or(0);
            let year_path = format!("{parent}/{year_name}");
            let Some(inner) = list(state, user, &year_path).await? else { continue };
            for month_name in &inner.folders {
                if let Some(month) = month_of(month_name) {
                    found.push(Month { ym: year * 100 + month, path: format!("{year_path}/{month_name}") });
                }
            }
        }
    }
    found.sort_by(|a, b| b.ym.cmp(&a.ym).then_with(|| a.path.cmp(&b.path)));
    let found = Arc::new(found);
    state.months.lock().unwrap().insert(user, (Instant::now(), found.clone()));
    Ok(found)
}

/// Days from 1970-01-01 (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn num(s: &str) -> Option<i64> {
    (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())).then(|| s.parse().ok()).flatten()
}

/// RFC 3339 / ISO 8601 to Unix milliseconds.
pub fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.len() < 19 || !s.is_char_boundary(19) {
        return None;
    }
    let (y, mo, d) = (num(s.get(0..4)?)?, num(s.get(5..7)?)?, num(s.get(8..10)?)?);
    let (h, mi, sec) = (num(s.get(11..13)?)?, num(s.get(14..16)?)?, num(s.get(17..19)?)?);
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let mut rest = &s[19..];
    let mut millis = 0;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        let ms: String = frac[..digits].chars().chain("000".chars()).take(3).collect();
        millis = num(&ms)?;
        rest = &frac[digits..];
    }
    let offset = match rest {
        "" | "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes()[0] {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let tz = rest[1..].replace(':', "");
            if tz.len() != 4 {
                return None;
            }
            sign * (num(&tz[0..2])? * 60 + num(&tz[2..4])?)
        }
    };
    let days = days_from_civil(y, mo, d);
    Some(((days * 24 + h) * 60 + mi - offset) * 60_000 + sec * 1000 + millis)
}

fn month_start(ym: u32) -> i64 {
    days_from_civil((ym / 100) as i64, (ym % 100) as i64, 1) * 86_400_000
}

/// The folder month, refined by the modified time when it agrees with it.
fn taken(ym: u32, modified: i64) -> i64 {
    let next = if ym % 100 == 12 { (ym / 100 + 1) * 100 + 1 } else { ym + 1 };
    let (start, end) = (month_start(ym), month_start(next));
    let slack = 86_400_000; // the folder uses the phone's time zone
    if modified >= start - slack && modified < end + slack {
        modified
    } else {
        start + 12 * 3_600_000
    }
}

pub fn photo_key(state: &AppState, user: i64, path: &str) -> String {
    format!("{user}.{}", sign(state, &format!("photo:{user}:{path}")))
}

/// The user a valid photo key was made for.
fn key_user(state: &AppState, path: &str, key: &str) -> Option<i64> {
    let (user, mac) = key.split_once('.')?;
    let user: i64 = num(user)?;
    verify(state, &format!("photo:{user}:{path}"), mac).then_some(user)
}

fn ym_of_date(date: &str) -> Option<u32> {
    let bytes = date.as_bytes();
    if date.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let (y, m) = (num(&date[0..4])?, num(&date[5..7])?);
    num(&date[8..10])?;
    (1..=12).contains(&m).then_some((y * 100 + m) as u32)
}

#[derive(Deserialize)]
pub struct PhotoQuery {
    from: Option<String>,
    to: Option<String>,
    cursor: Option<String>,
}

pub async fn list_photos(
    State(state): State<AppState>,
    Auth(user): Auth,
    Query(query): Query<PhotoQuery>,
) -> Result<Json<PhotoPage>> {
    require_photos(&state)?;
    let date = |d: &Option<String>, default: u32| match d.as_deref().map(str::trim) {
        None | Some("") => Ok(default),
        Some(d) => ym_of_date(d).ok_or_else(|| bad("Date invalide.")),
    };
    let (from, to) = (date(&query.from, 0)?, date(&query.to, u32::MAX)?);
    let cursor = match query.cursor.as_deref().filter(|c| !c.is_empty()) {
        None => None,
        Some(c) => {
            let (ym, offset) = c.split_once('.').ok_or_else(|| bad("Curseur invalide."))?;
            let parsed = num(ym).zip(num(offset)).filter(|(_, o)| *o < 1_000_000);
            let (ym, offset) = parsed.ok_or_else(|| bad("Curseur invalide."))?;
            Some((ym as u32, offset as usize))
        }
    };
    if !state.allow(user.id.to_string(), "photos", 120, Duration::from_secs(60)) {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "Doucement ! Patientez un instant.".into()));
    }
    let root = session(&state, user.id).await?.root;
    let all = months(&state, user.id, &root).await?;
    let upper = cursor.map_or(to, |(ym, _)| ym.min(to));
    let mut groups: Vec<(u32, Vec<&Month>)> = Vec::new();
    for month in all.iter().filter(|m| m.ym >= from && m.ym <= upper) {
        match groups.last_mut() {
            Some((ym, list)) if *ym == month.ym => list.push(month),
            _ => groups.push((month.ym, vec![month])),
        }
    }
    let mut photos = Vec::new();
    let mut next = None;
    for (index, (ym, folders)) in groups.iter().enumerate() {
        let mut items = Vec::new();
        for folder in folders {
            let Some(listing) = list(&state, user.id, &folder.path).await? else { continue };
            for (name, size, modified) in listing.files {
                let Some(mime) = mime_of(&name) else { continue };
                let path = format!("{}/{name}", folder.path);
                items.push((modified, Photo {
                    key: photo_key(&state, user.id, &path),
                    is_video: mime.starts_with("video/"),
                    taken: taken(*ym, modified),
                    path,
                    name,
                    size,
                }));
            }
        }
        items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)).then_with(|| a.1.path.cmp(&b.1.path)));
        let skip = match cursor {
            Some((c, offset)) if c == *ym => offset,
            _ => 0,
        };
        let room = PAGE - photos.len();
        let available = items.len().saturating_sub(skip);
        photos.extend(items.into_iter().skip(skip).take(room).map(|(_, p)| p));
        if available > room {
            next = Some(format!("{ym}.{}", skip + room));
            break;
        }
        if photos.len() == PAGE {
            next = groups.get(index + 1).map(|(ym, _)| format!("{ym}.0"));
            break;
        }
    }
    Ok(Json(PhotoPage { photos, next }))
}

#[derive(Deserialize)]
pub struct ThumbQuery {
    #[serde(default)]
    path: String,
    #[serde(default)]
    k: String,
}

pub async fn thumb(State(state): State<AppState>, Query(query): Query<ThumbQuery>) -> Result<Response> {
    let user = key_user(&state, &query.path, &query.k)
        .ok_or_else(|| Error(StatusCode::FORBIDDEN, "Lien invalide.".into()))?;
    require_photos(&state)?;
    if !state.allow(user.to_string(), "photo-thumb", 600, Duration::from_secs(60)) {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "Doucement ! Patientez un instant.".into()));
    }
    let root = session(&state, user).await?.root;
    check_path(&root, &query.path)?;
    let path = query.path.as_str();
    let mut response = None;
    for route in ["/api/resources/preview", "/api/preview"] {
        let url = format!("{}{route}", base(&state));
        let (found, _) = send(&state, user, &state.http, |client, s| {
            client.get(&url).query(&[
                ("source", s.source.as_str()),
                ("path", path),
                ("size", "small"),
                ("inline", "true"),
            ])
        })
        .await?;
        if found.status() != StatusCode::NOT_FOUND {
            response = Some(found);
            break;
        }
    }
    let response = response.filter(|r| r.status().is_success()).ok_or_else(crate::error::not_found)?;
    let mime = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase();
    let mime = safe_image(&mime).ok_or_else(unsupported_image)?;
    let bytes = read_limited(response, 5 << 20).await.ok_or_else(unreachable)?;
    image_response(mime, bytes)
}

pub async fn import(
    State(state): State<AppState>,
    Auth(user): Auth,
    Json(input): Json<ImportPhoto>,
) -> Result<Json<Media>> {
    require_photos(&state)?;
    // Same budget as uploads.
    if !state.allow(user.id.to_string(), "upload", 60, Duration::from_secs(600)) {
        return Err(Error(StatusCode::TOO_MANY_REQUESTS, "Trop d'envois, patientez.".into()));
    }
    let _busy = Busy::claim(&state, user.id)?;
    let root = session(&state, user.id).await?.root;
    check_path(&root, &input.path)?;
    let name: String = input.path.rsplit('/').next().unwrap_or("photo").chars().take(120).collect();
    let mime = mime_of(&name).ok_or_else(|| bad("Seuls les photos, vidéos et fichiers audio sont acceptés."))?;
    if !media::allowed_mime(mime) {
        return Err(bad("Seuls les photos, vidéos et fichiers audio sont acceptés."));
    }
    let remaining = media::remaining(&state, user.id).await?;
    let url = format!("{}/api/resources/download", base(&state));
    let path = input.path.as_str();
    let (mut response, _) = send(&state, user.id, &state.streaming, |client, s| {
        client.get(&url).query(&[("source", s.source.as_str()), ("file", path)])
    })
    .await?;
    if response.status() == StatusCode::NOT_FOUND {
        return Err(crate::error::not_found());
    }
    if !response.status().is_success() {
        tracing::warn!("filebrowser download: HTTP {}", response.status());
        return Err(unreachable());
    }
    if let Some(length) = response.content_length() {
        if length > state.config.max_upload {
            return Err(media::too_large("Fichier trop volumineux."));
        }
        if length > remaining {
            return Err(media::too_large("Espace de stockage épuisé."));
        }
    }

    let id = random_hex(24);
    let file_path = state.config.data_dir.join("media").join(&id);
    let mut guard = Unfinished(Some(file_path.clone()));
    let mut file = tokio::fs::File::create(&file_path).await?;
    let mut size = 0u64;
    let deadline = Instant::now() + Duration::from_secs(600);
    while let Some(chunk) = response.chunk().await.map_err(|e| {
        tracing::warn!("filebrowser download: {}", e.without_url());
        unreachable()
    })? {
        size += chunk.len() as u64;
        if size > state.config.max_upload {
            return Err(media::too_large("Fichier trop volumineux."));
        }
        if size > remaining {
            return Err(media::too_large("Espace de stockage épuisé."));
        }
        if Instant::now() > deadline {
            return Err(unreachable());
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
        .bind(mime)
        .bind(&name)
        .bind(size as i64)
        .bind(now_ms())
        .execute(&state.db)
        .await?;
    guard.0 = None;
    Ok(Json(Media { id, mime: mime.into(), name, size: size as i64 }))
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn paths_stay_inside_the_root() {
        let root = "/backups/photos/alice";
        assert!(check_path(root, "/backups/photos/alice/2026/09/IMG.jpg").is_ok());
        for bad in [
            "/backups/photos/alice/../bob/2026/09/IMG.jpg",
            "/backups/photos/alice/2026/./IMG.jpg",
            "/backups/photos/alicex/IMG.jpg",
            "/backups/photos/alice",
            "/backups/photos/alice/",
            "/backups/photos/bob/IMG.jpg",
            "backups/photos/alice/IMG.jpg",
            "/backups/photos/alice//etc/passwd",
            "/backups/photos/alice/2026\\..\\..\\bob.jpg",
            "/backups/photos/alice/a\nb.jpg",
        ] {
            assert!(check_path(root, bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn times_and_months() {
        assert_eq!(parse_time("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_time("2026-09-15T10:00:00Z"), Some(1_789_466_400_000));
        assert_eq!(parse_time("2026-09-15T12:00:00.5+02:00"), Some(1_789_466_400_500));
        assert_eq!(parse_time("nope"), None);
        let modified = parse_time("2026-09-15T10:00:00Z").unwrap();
        assert_eq!(taken(202609, modified), modified);
        assert_eq!(taken(202601, modified), month_start(202601) + 12 * 3_600_000);
        assert_eq!(ym_of_date("2026-09-01"), Some(202609));
        assert_eq!(ym_of_date("2026-13-01"), None);
        assert_eq!(mime_of("IMG_1.HEIC"), Some("image/heic"));
        assert_eq!(mime_of("notes.txt"), None);
        assert_eq!(encode_component("a b/é"), "a%20b%2F%C3%A9");
    }
}
