//! HTTP-level tests of access control and the main flows.

use crate::{api, state};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use serde_json::{json, Value};
use tower::ServiceExt;

async fn app() -> Router {
    app_with_invite("").await
}

async fn app_with_invite(invite_code: &str) -> Router {
    api::router(app_state(invite_code, "").await)
}

async fn app_state(invite_code: &str, fake: &str) -> state::AppState {
    let data_dir = std::env::temp_dir().join(format!("chat-test-{}", crate::auth::random_hex(8)));
    let config = state::Config {
        data_dir,
        max_upload: 1000,
        user_quota: 1500,
        total_quota: 1_000_000,
        turn_host: "turn.test".into(),
        turn_secret: "secret".into(),
        invite_code: invite_code.into(),
        navidrome_url: fake.into(),
        filebrowser_url: fake.into(),
        photos_folder: state::DEFAULT_PHOTOS_FOLDER.into(),
    };
    state::AppState::new(config).await.unwrap()
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Vec<u8>) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    (status, to_bytes(response.into_body(), 1 << 20).await.unwrap().to_vec())
}

async fn call(app: &Router, method: &str, uri: &str, token: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .body(Body::from(if body.is_null() { String::new() } else { body.to_string() }))
        .unwrap();
    let (status, bytes) = send(app, request).await;
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn account(app: &Router, name: &str) -> (String, i64) {
    let body = json!({"username": name, "display_name": name, "password": "motdepasse"});
    let (status, v) = call(app, "POST", "/api/register", "", body).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    (v["token"].as_str().unwrap().into(), v["user"]["id"].as_i64().unwrap())
}

async fn upload(app: &Router, token: &str, mime: &str, data: &[u8]) -> (StatusCode, Value) {
    let mut body = format!(
        "--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"a\"\r\nContent-Type: {mime}\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(data);
    body.extend_from_slice(b"\r\n--B--\r\n");
    let request = Request::builder()
        .method("POST")
        .uri("/api/media")
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "multipart/form-data; boundary=B")
        .body(Body::from(body))
        .unwrap();
    let (status, bytes) = send(app, request).await;
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn accounts() {
    let app = app().await;
    let (token, _) = account(&app, "alice").await;
    let dup = json!({"username": "ALICE", "display_name": "x", "password": "motdepasse"});
    assert_eq!(call(&app, "POST", "/api/register", "", dup).await.0, StatusCode::CONFLICT);
    let bad = json!({"username": "alice", "password": "mauvais-mdp"});
    assert_eq!(call(&app, "POST", "/api/login", "", bad).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&app, "GET", "/api/me", &token, Value::Null).await.0, StatusCode::OK);
    call(&app, "POST", "/api/logout", &token, Value::Null).await;
    assert_eq!(call(&app, "GET", "/api/me", &token, Value::Null).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn messaging_is_limited_to_members() {
    let app = app().await;
    let (alice, _) = account(&app, "alice").await;
    let (bob, bob_id) = account(&app, "bob").await;
    let (eve, _) = account(&app, "eve").await;

    let new = json!({"member_ids": [bob_id], "name": null});
    let (_, first) = call(&app, "POST", "/api/conversations", &alice, new.clone()).await;
    let (_, again) = call(&app, "POST", "/api/conversations", &alice, new).await;
    assert_eq!(first["id"], again["id"], "a direct conversation is reused");
    let url = format!("/api/conversations/{}/messages", first["id"]);

    let hello = json!({"body": "salut", "media_id": null});
    assert_eq!(call(&app, "POST", &url, &alice, hello.clone()).await.0, StatusCode::OK);
    let (_, list) = call(&app, "GET", "/api/conversations", &bob, Value::Null).await;
    assert_eq!(list[0]["unread"], 1);
    assert_eq!(list[0]["last_message"]["body"], "salut");

    assert_eq!(call(&app, "GET", &url, &eve, Value::Null).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "POST", &url, &eve, hello).await.0, StatusCode::NOT_FOUND);

    let (_, media) = upload(&app, &alice, "image/png", b"data").await;
    let stolen = json!({"body": "", "media_id": media["id"]});
    assert_eq!(call(&app, "POST", &url, &bob, stolen).await.0, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn uploads_are_restricted() {
    let app = app().await;
    let (alice, _) = account(&app, "alice").await;
    assert_eq!(upload(&app, &alice, "text/html", b"<p>").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(upload(&app, &alice, "image/svg+xml", b"<svg>").await.0, StatusCode::BAD_REQUEST);
    assert_eq!(upload(&app, &alice, "image/png", &[0; 1001]).await.0, StatusCode::PAYLOAD_TOO_LARGE);
    let (status, media) = upload(&app, &alice, "audio/webm;codecs=opus", &[7; 900]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(upload(&app, &alice, "image/png", &[0; 900]).await.0, StatusCode::PAYLOAD_TOO_LARGE);

    let request = Request::builder()
        .uri(format!("/api/media/{}", media["id"].as_str().unwrap()))
        .header("range", "bytes=0-9")
        .body(Body::empty())
        .unwrap();
    let (status, bytes) = send(&app, request).await;
    assert_eq!((status, bytes.len()), (StatusCode::PARTIAL_CONTENT, 10));
}

async fn direct(app: &Router, token: &str, other: i64) -> (StatusCode, Value) {
    let new = json!({"member_ids": [other], "name": null});
    call(app, "POST", "/api/conversations", token, new).await
}

async fn say(app: &Router, token: &str, conversation: &Value, body: Value) -> (StatusCode, Value) {
    let url = format!("/api/conversations/{conversation}/messages");
    call(app, "POST", &url, token, body).await
}

async fn history(app: &Router, token: &str, conversation: &Value) -> Value {
    let url = format!("/api/conversations/{conversation}/messages");
    call(app, "GET", &url, token, Value::Null).await.1
}

async fn file_status(app: &Router, id: &Value) -> StatusCode {
    let uri = format!("/api/media/{}", id.as_str().unwrap());
    send(app, Request::builder().uri(uri).body(Body::empty()).unwrap()).await.0
}

#[tokio::test]
async fn replies_reactions_deletion_and_reads() {
    let app = app().await;
    let (alice, alice_id) = account(&app, "alice").await;
    let (bob, bob_id) = account(&app, "bob").await;
    let (eve, eve_id) = account(&app, "eve").await;
    let chat = direct(&app, &alice, bob_id).await.1["id"].clone();
    let other = direct(&app, &alice, eve_id).await.1["id"].clone();

    let (_, media) = upload(&app, &alice, "image/png", b"data").await;
    let (_, first) = say(&app, &alice, &chat, json!({"body": "photo", "media_id": media["id"]})).await;
    let answer = json!({"body": "jolie", "media_id": null, "reply_to": first["id"]});
    let (status, reply) = say(&app, &bob, &chat, answer.clone()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(reply["reply"]["body"], "photo");
    assert_eq!(say(&app, &alice, &other, answer).await.0, StatusCode::BAD_REQUEST, "no cross quoting");

    let react = format!("/api/messages/{}/reaction", first["id"]);
    let heart = json!({"emoji": "❤️"});
    assert_eq!(call(&app, "PUT", &react, &eve, heart.clone()).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "PUT", &react, &bob, json!({"emoji": "x"})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "PUT", &react, &bob, heart).await.0, StatusCode::NO_CONTENT);
    assert_eq!(history(&app, &alice, &chat).await[0]["reactions"][0]["user_id"], bob_id);

    let read = format!("/api/conversations/{chat}/read");
    assert_eq!(call(&app, "POST", &read, &bob, Value::Null).await.0, StatusCode::NO_CONTENT);
    let (_, list) = call(&app, "GET", "/api/conversations", &alice, Value::Null).await;
    let found = list.as_array().unwrap().iter().find(|c| c["id"] == chat).unwrap();
    let mark = found["reads"].as_array().unwrap().iter().find(|r| r["user_id"] == bob_id).unwrap();
    assert_eq!(mark["message_id"], reply["id"]);
    assert_ne!(alice_id, bob_id);

    let url = format!("/api/messages/{}", first["id"]);
    assert_eq!(call(&app, "DELETE", &url, &bob, Value::Null).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "DELETE", &url, &alice, Value::Null).await.0, StatusCode::NO_CONTENT);
    let list = history(&app, &bob, &chat).await;
    assert_eq!(list[0]["deleted"], true);
    assert_eq!(list[0]["body"], "");
    assert_eq!(list[1]["reply"]["deleted"], true);
    assert_eq!(list[1]["reply"]["body"], "");
    assert_eq!(file_status(&app, &media["id"]).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn blocking() {
    let app = app().await;
    let (alice, alice_id) = account(&app, "alice").await;
    let (bob, bob_id) = account(&app, "bob").await;
    let chat = direct(&app, &alice, bob_id).await.1["id"].clone();
    let hello = json!({"body": "salut", "media_id": null});
    let url = format!("/api/blocks/{bob_id}");
    assert_eq!(call(&app, "PUT", &url, &alice, Value::Null).await.0, StatusCode::NO_CONTENT);

    assert_eq!(say(&app, &bob, &chat, hello.clone()).await.0, StatusCode::FORBIDDEN);
    assert_eq!(say(&app, &alice, &chat, hello.clone()).await.0, StatusCode::FORBIDDEN);
    assert_eq!(direct(&app, &bob, alice_id).await.0, StatusCode::FORBIDDEN);
    let (_, found) = call(&app, "GET", "/api/users?q=ali", &bob, Value::Null).await;
    assert_eq!(found, json!([]));
    let (_, list) = call(&app, "GET", "/api/blocks", &alice, Value::Null).await;
    assert_eq!(list[0]["id"], bob_id);

    assert_eq!(call(&app, "DELETE", &url, &alice, Value::Null).await.0, StatusCode::NO_CONTENT);
    assert_eq!(say(&app, &bob, &chat, hello).await.0, StatusCode::OK);
}

#[tokio::test]
async fn leaving_and_deleting_an_account() {
    let app = app().await;
    let (alice, alice_id) = account(&app, "alice").await;
    let (bob, bob_id) = account(&app, "bob").await;
    let (eve, eve_id) = account(&app, "eve").await;
    let new = json!({"member_ids": [bob_id, eve_id], "name": "groupe"});
    let group = call(&app, "POST", "/api/conversations", &alice, new).await.1["id"].clone();
    let chat = direct(&app, &alice, bob_id).await.1["id"].clone();

    let leave = |id: &Value| format!("/api/conversations/{id}/leave");
    assert_eq!(call(&app, "POST", &leave(&chat), &bob, Value::Null).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "POST", &leave(&group), &eve, Value::Null).await.0, StatusCode::NO_CONTENT);
    let url = format!("/api/conversations/{group}/messages");
    assert_eq!(call(&app, "GET", &url, &eve, Value::Null).await.0, StatusCode::NOT_FOUND);

    let (_, mine) = upload(&app, &alice, "image/png", b"mine").await;
    let (_, theirs) = upload(&app, &bob, "image/png", b"theirs").await;
    say(&app, &alice, &group, json!({"body": "", "media_id": mine["id"]})).await;
    say(&app, &bob, &chat, json!({"body": "", "media_id": theirs["id"]})).await;
    say(&app, &bob, &group, json!({"body": "reste", "media_id": null})).await;

    let wrong = json!({"password": "mauvais-mdp"});
    let right = json!({"password": "motdepasse"});
    let url = "/api/account/delete";
    assert_eq!(call(&app, "POST", url, &alice, wrong).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "POST", url, &alice, right).await.0, StatusCode::NO_CONTENT);
    assert_eq!(call(&app, "GET", "/api/me", &alice, Value::Null).await.0, StatusCode::UNAUTHORIZED);

    let (_, list) = call(&app, "GET", "/api/conversations", &bob, Value::Null).await;
    assert_eq!(list.as_array().unwrap().len(), 1, "the one-to-one conversation is gone");
    assert_eq!(list[0]["id"], group);
    let messages = history(&app, &bob, &group).await;
    assert_eq!(messages.as_array().unwrap().len(), 1);
    assert!(messages.as_array().unwrap().iter().all(|m| m["sender"]["id"] != alice_id));
    assert_eq!(file_status(&app, &mine["id"]).await, StatusCode::NOT_FOUND);
    assert_eq!(file_status(&app, &theirs["id"]).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn profile_photo() {
    let app = app().await;
    let (alice, _) = account(&app, "alice").await;
    let (bob, _) = account(&app, "bob").await;
    let (_, photo) = upload(&app, &alice, "image/png", b"face").await;
    let (_, sound) = upload(&app, &alice, "audio/ogg", b"la").await;
    let profile = |id: &Value| json!({"display_name": "Alice", "avatar_id": id});

    assert_eq!(call(&app, "PUT", "/api/me", &bob, profile(&photo["id"])).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "PUT", "/api/me", &alice, profile(&sound["id"])).await.0, StatusCode::BAD_REQUEST);
    let (status, user) = call(&app, "PUT", "/api/me", &alice, profile(&photo["id"])).await;
    assert_eq!((status, &user["avatar"]), (StatusCode::OK, &photo["id"]));

    let remove = json!({"display_name": "Alice", "remove_avatar": true});
    assert_eq!(call(&app, "PUT", "/api/me", &alice, remove).await.1["avatar"], Value::Null);
    assert_eq!(file_status(&app, &photo["id"]).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn directory_lists_everyone_else() {
    let app = app().await;
    let (alice, _) = account(&app, "alice").await;
    let (bob, bob_id) = account(&app, "bob").await;
    account(&app, "carole").await;
    let (_, all) = call(&app, "GET", "/api/users", &alice, Value::Null).await;
    let names: Vec<&str> = all.as_array().unwrap().iter().map(|u| u["username"].as_str().unwrap()).collect();
    assert_eq!(names, ["bob", "carole"]);
    let (_, found) = call(&app, "GET", "/api/users?q=aro", &alice, Value::Null).await;
    assert_eq!(found.as_array().unwrap().len(), 1);
    assert_eq!(found[0]["username"], "carole");

    call(&app, "PUT", &format!("/api/blocks/{bob_id}"), &alice, Value::Null).await;
    let (_, all) = call(&app, "GET", "/api/users", &bob, Value::Null).await;
    assert_eq!(all.as_array().unwrap().len(), 1, "a block hides both ways");
}

#[tokio::test]
async fn invite_code() {
    let app = app_with_invite("sesame").await;
    let mut body = json!({"username": "alice", "display_name": "a", "password": "motdepasse"});
    assert_eq!(call(&app, "POST", "/api/register", "", body.clone()).await.0, StatusCode::FORBIDDEN);
    body["invite"] = json!("sesame");
    assert_eq!(call(&app, "POST", "/api/register", "", body).await.0, StatusCode::OK);
}

#[tokio::test]
async fn old_databases_are_migrated() {
    let db = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::raw_sql("CREATE TABLE users (id INTEGER); CREATE TABLE messages (id INTEGER);")
        .execute(&db)
        .await
        .unwrap();
    state::migrate(&db).await.unwrap();
    state::migrate(&db).await.unwrap();
    sqlx::query("SELECT avatar FROM users").fetch_all(&db).await.unwrap();
    sqlx::query("SELECT reply_to, deleted FROM messages").fetch_all(&db).await.unwrap();
}

/// A fake goofysound (Subsonic) and FileBrowser Quantum on one port.
/// Accounts: Navidrome `alice` / `pw`, FileBrowser `alice` / `pw`.
async fn fake_services() -> String {
    use axum::{extract::Query, http::HeaderMap, response::IntoResponse, routing::{get, post}};
    use md5::{Digest, Md5};
    use std::collections::HashMap;
    type Q = Query<HashMap<String, String>>;

    fn sub(Query(q): Q) -> Result<(), Value> {
        let salt = q.get("s").cloned().unwrap_or_default();
        let token: String = Md5::digest(format!("pw{salt}")).iter().map(|b| format!("{b:02x}")).collect();
        if q.get("u").map(String::as_str) == Some("alice") && q.get("t") == Some(&token) {
            Ok(())
        } else {
            Err(json!({"subsonic-response": {"status": "failed", "error": {"code": 40, "message": "Wrong"}}}))
        }
    }
    fn ok(extra: Value) -> axum::Json<Value> {
        let mut body = json!({"status": "ok", "version": "1.16.1"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        axum::Json(json!({ "subsonic-response": body }))
    }
    let song = json!({"id": "s1", "title": "Song", "artist": "Artist", "album": "Album", "coverArt": "al-1", "duration": 213});
    let fb_auth = |h: &HeaderMap| h.get("authorization").and_then(|v| v.to_str().ok()) == Some("Bearer a.b.c");

    let (s1, s2, s3) = (song.clone(), song.clone(), song.clone());
    let router = axum::Router::new()
        .route("/rest/ping.view", get(|q: Q| async move { sub(q).map(|_| ok(json!({}))).unwrap_or_else(axum::Json) }))
        .route("/rest/search3.view", get(move |q: Q| async move {
            sub(q).map(|_| ok(json!({"searchResult3": {"song": [s1]}}))).unwrap_or_else(axum::Json)
        }))
        .route("/rest/getSong.view", get(move |q: Q| async move {
            let id = q.0.get("id").cloned();
            match sub(q) {
                Err(e) => axum::Json(e),
                Ok(_) if id.as_deref() == Some("s1") => ok(json!({"song": s2})),
                Ok(_) => axum::Json(json!({"subsonic-response": {"status": "failed", "error": {"code": 70}}})),
            }
        }))
        .route("/rest/getNowPlaying.view", get(move |q: Q| async move {
            let mut entry = s3.clone();
            entry["username"] = json!("alice");
            entry["minutesAgo"] = json!(0);
            entry["playerName"] = json!("SelfHost-hub");
            sub(q).map(|_| ok(json!({"nowPlaying": {"entry": [entry]}}))).unwrap_or_else(axum::Json)
        }))
        .route("/rest/stream.view", get(|q: Q| async move {
            match sub(q) {
                Ok(_) => ([("content-type", "audio/mpeg")], b"MP3DATA".to_vec()).into_response(),
                Err(e) => axum::Json(e).into_response(),
            }
        }))
        .route("/rest/getCoverArt.view", get(|q: Q| async move {
            match sub(q) {
                Ok(_) => ([("content-type", "image/png")], b"PNG".to_vec()).into_response(),
                Err(e) => axum::Json(e).into_response(),
            }
        }))
        .route("/api/auth/login", post(|Query(q): Q, h: HeaderMap| async move {
            let user = q.get("username").map(String::as_str) == Some("alice");
            if user && h.get("x-password").and_then(|v| v.to_str().ok()) == Some("pw") {
                ([("set-cookie", "filebrowser_quantum_jwt=a.b.c; Path=/")], "").into_response()
            } else {
                StatusCode::UNAUTHORIZED.into_response()
            }
        }))
        .route("/api/users", get(move |h: HeaderMap| async move {
            if !fb_auth(&h) {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            axum::Json(json!({"username": "alice", "scopes": [{"name": "default"}]})).into_response()
        }))
        .route("/api/resources", get(move |Query(q): Q, h: HeaderMap| async move {
            if !fb_auth(&h) || q.get("source").map(String::as_str) != Some("default") {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            let body = match q.get("path").map(String::as_str).unwrap_or("") {
                "/backups/photos/alice" => json!({"folders": [{"name": "2026"}, {"name": "Corbeille"}], "files": []}),
                "/backups/photos/alice/2026" => json!({"folders": [{"name": "09"}], "files": []}),
                "/backups/photos/alice/2026/09" => json!({"folders": [], "files": [
                    {"name": "IMG_1.jpg", "size": 8, "type": "image/jpeg", "modified": "2026-09-15T10:00:00Z"},
                    {"name": "notes.txt", "size": 3, "type": "text/plain"},
                ]}),
                _ => return StatusCode::NOT_FOUND.into_response(),
            };
            axum::Json(body).into_response()
        }))
        .route("/api/resources/preview", get(move |h: HeaderMap| async move {
            if !fb_auth(&h) {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            ([("content-type", "image/jpeg")], b"THUMB".to_vec()).into_response()
        }))
        .route("/api/resources/download", get(move |Query(q): Q, h: HeaderMap| async move {
            if !fb_auth(&h) {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            if q.get("file").map(String::as_str) != Some("/backups/photos/alice/2026/09/IMG_1.jpg") {
                return StatusCode::NOT_FOUND.into_response();
            }
            ([("content-type", "image/jpeg")], b"JPEGDATA".to_vec()).into_response()
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    url
}

async fn get_raw(app: &Router, uri: &str) -> (StatusCode, Vec<u8>) {
    send(app, Request::builder().uri(uri).body(Body::empty()).unwrap()).await
}

#[tokio::test]
async fn goofysound_links_music_and_activity() {
    let state = app_state("", &fake_services().await).await;
    let app = api::router(state.clone());
    let (alice, alice_id) = account(&app, "alice").await;
    let (bob, bob_id) = account(&app, "bob").await;
    let (eve, _) = account(&app, "eve").await;

    let link = "/api/links/navidrome";
    let wrong = json!({"username": "alice", "password": "nope"});
    assert_eq!(call(&app, "POST", link, &alice, wrong).await.0, StatusCode::BAD_REQUEST);
    let right = json!({"username": "alice", "password": "pw"});
    let (status, links) = call(&app, "POST", link, &alice, right.clone()).await;
    assert_eq!((status, &links["navidrome"]), (StatusCode::OK, &json!("alice")));
    assert_eq!(call(&app, "POST", link, &bob, right).await.0, StatusCode::CONFLICT, "one owner per account");
    let row: (String, String) = sqlx::query_as("SELECT salt, token FROM navidrome_links").fetch_one(&state.db).await.unwrap();
    assert!(!row.1.contains("pw") && row.1.len() == 32, "only a token is stored");

    assert_eq!(call(&app, "GET", "/api/music/search?q=song", &bob, Value::Null).await.0, StatusCode::FORBIDDEN);
    let (status, found) = call(&app, "GET", "/api/music/search?q=song", &alice, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(found[0]["title"], "Song");
    let key = found[0]["key"].as_str().unwrap().to_string();

    let chat = direct(&app, &alice, bob_id).await.1["id"].clone();
    let (status, sent) = say(&app, &alice, &chat, json!({"body": "", "media_id": null, "track_id": "s1"})).await;
    assert_eq!((status, &sent["track"]["artist"]), (StatusCode::OK, &json!("Artist")));
    let missing = json!({"body": "", "media_id": null, "track_id": "zz"});
    assert_eq!(say(&app, &alice, &chat, missing).await.0, StatusCode::NOT_FOUND);
    assert_eq!(history(&app, &bob, &chat).await[0]["track"]["key"], json!(key));

    let (status, bytes) = get_raw(&app, &format!("/api/music/s1/stream?k={key}")).await;
    assert_eq!((status, bytes.as_slice()), (StatusCode::OK, &b"MP3DATA"[..]));
    let forged = "0".repeat(64);
    assert_eq!(get_raw(&app, &format!("/api/music/s1/stream?k={forged}")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(get_raw(&app, &format!("/api/music/s2/stream?k={key}")).await.0, StatusCode::FORBIDDEN);
    assert_eq!(get_raw(&app, &format!("/api/music/s1/cover?k={key}")).await.0, StatusCode::OK);

    crate::music::poll_activity(&state).await;
    let (_, seen) = call(&app, "GET", "/api/activity", &bob, Value::Null).await;
    assert_eq!(seen[0]["user_id"], alice_id);
    assert_eq!(seen[0]["track"]["title"], "Song");
    let (_, hidden) = call(&app, "GET", "/api/activity", &eve, Value::Null).await;
    assert_eq!(hidden, json!([]), "only people sharing a conversation see it");

    assert_eq!(call(&app, "DELETE", link, &alice, Value::Null).await.0, StatusCode::NO_CONTENT);
    let (_, seen) = call(&app, "GET", "/api/activity", &bob, Value::Null).await;
    assert_eq!(seen, json!([]));
    // Nobody linked any more: shared songs stop playing.
    assert_eq!(get_raw(&app, &format!("/api/music/s1/stream?k={key}")).await.0, StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn photos_link_list_thumb_and_import() {
    let state = app_state("", &fake_services().await).await;
    let app = api::router(state.clone());
    let (alice, _) = account(&app, "alice").await;
    let (bob, bob_id) = account(&app, "bob").await;

    assert_eq!(call(&app, "GET", "/api/photos", &alice, Value::Null).await.0, StatusCode::FORBIDDEN);
    let link = "/api/links/photos";
    let wrong = json!({"username": "alice", "password": "nope"});
    assert_eq!(call(&app, "POST", link, &alice, wrong).await.0, StatusCode::BAD_REQUEST);
    let right = json!({"username": "alice", "password": "pw"});
    assert_eq!(call(&app, "POST", link, &alice, right).await.1["photos"], "alice");
    let (sealed,): (String,) = sqlx::query_as("SELECT password FROM photo_links").fetch_one(&state.db).await.unwrap();
    assert_ne!(sealed, "pw", "the password is encrypted");

    let (status, page) = call(&app, "GET", "/api/photos?from=2026-01-01&to=2026-12-31", &alice, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    let photos = page["photos"].as_array().unwrap();
    assert_eq!(photos.len(), 1, "only photos and videos");
    let path = photos[0]["path"].as_str().unwrap().to_string();
    assert_eq!(path, "/backups/photos/alice/2026/09/IMG_1.jpg");
    let (_, empty) = call(&app, "GET", "/api/photos?from=2027-01-01", &alice, Value::Null).await;
    assert_eq!(empty["photos"], json!([]));

    // Forget the session: the stored, encrypted password logs in again.
    crate::links::forget_photos(&state, 1);
    let key = photos[0]["key"].as_str().unwrap();
    let thumb = format!("/api/photos/thumb?path={}&k={key}", path.replace('/', "%2F"));
    assert_eq!(get_raw(&app, &thumb).await, (StatusCode::OK, b"THUMB".to_vec()));
    let other = format!("/api/photos/thumb?path=%2Fbackups%2Fphotos%2Fbob%2Fa.jpg&k={key}");
    assert_eq!(get_raw(&app, &other).await.0, StatusCode::FORBIDDEN);

    let escape = json!({"path": "/backups/photos/alice/../bob/2026/09/IMG_1.jpg"});
    assert_eq!(call(&app, "POST", "/api/photos/import", &alice, escape).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "POST", "/api/photos/import", &bob, json!({"path": path})).await.0, StatusCode::FORBIDDEN);
    let (status, media) = call(&app, "POST", "/api/photos/import", &alice, json!({"path": path})).await;
    assert_eq!((status, &media["size"]), (StatusCode::OK, &json!(8)));
    let chat = direct(&app, &alice, bob_id).await.1["id"].clone();
    assert_eq!(say(&app, &alice, &chat, json!({"body": "", "media_id": media["id"]})).await.0, StatusCode::OK);
    let (_, bytes) = get_raw(&app, &format!("/api/media/{}", media["id"].as_str().unwrap())).await;
    assert_eq!(bytes, b"JPEGDATA");

    let right = json!({"password": "motdepasse"});
    assert_eq!(call(&app, "POST", "/api/account/delete", &alice, right).await.0, StatusCode::NO_CONTENT);
    let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM photo_links").fetch_one(&state.db).await.unwrap();
    assert_eq!(count, 0, "links go with the account");
    let _ = bob;
}

#[tokio::test]
async fn registration_is_rate_limited() {
    let app = app().await;
    for i in 0..5 {
        account(&app, &format!("user{i}")).await;
    }
    let body = json!({"username": "user9", "display_name": "x", "password": "motdepasse"});
    assert_eq!(call(&app, "POST", "/api/register", "", body).await.0, StatusCode::TOO_MANY_REQUESTS);
}
