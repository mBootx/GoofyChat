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
    app_state_with(invite_code, fake, "").await
}

async fn app_state_with(invite_code: &str, fake: &str, worker_token: &str) -> state::AppState {
    let data_dir = std::env::temp_dir().join(format!("chat-test-{}", crate::auth::random_hex(8)));
    let config = state::Config {
        worker_token: worker_token.into(),
        worker_bind: "127.0.0.1:0".into(),
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
    let (alice, alice_id) = account(&app, "alice").await;
    let (bob, bob_id) = account(&app, "bob").await;
    let (eve, _) = account(&app, "eve").await;
    befriend(&app, &alice, alice_id, &bob, "bob").await;

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

/// `from` asks `to_name` (`to`) to be friends and they accept.
async fn befriend(app: &Router, from: &str, from_id: i64, to: &str, to_name: &str) {
    let (status, v) = call(app, "POST", "/api/friends/requests", from, json!({"username": to_name})).await;
    assert_eq!(status, StatusCode::OK, "{v}");
    if v["status"] == "sent" {
        let url = format!("/api/friends/requests/{from_id}/accept");
        let (status, v) = call(app, "POST", &url, to, Value::Null).await;
        assert_eq!(status, StatusCode::OK, "{v}");
    }
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
    befriend(&app, &alice, alice_id, &bob, "bob").await;
    befriend(&app, &alice, alice_id, &eve, "eve").await;
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
    befriend(&app, &alice, alice_id, &bob, "bob").await;
    let chat = direct(&app, &alice, bob_id).await.1["id"].clone();
    let hello = json!({"body": "salut", "media_id": null});
    let url = format!("/api/blocks/{bob_id}");
    assert_eq!(call(&app, "PUT", &url, &alice, Value::Null).await.0, StatusCode::NO_CONTENT);

    let (status, refused) = say(&app, &bob, &chat, hello.clone()).await;
    assert_eq!((status, refused["error"].as_str()), (StatusCode::FORBIDDEN, Some("Vous ne pouvez pas écrire à cette personne.")));
    assert_eq!(say(&app, &alice, &chat, hello.clone()).await.0, StatusCode::FORBIDDEN);
    assert_eq!(direct(&app, &bob, alice_id).await.0, StatusCode::FORBIDDEN);
    let (_, found) = call(&app, "GET", "/api/users?q=ali", &bob, Value::Null).await;
    assert_eq!(found, json!([]));
    let (_, list) = call(&app, "GET", "/api/blocks", &alice, Value::Null).await;
    assert_eq!(list[0]["id"], bob_id);
    let (_, friends) = call(&app, "GET", "/api/friends", &bob, Value::Null).await;
    assert_eq!(friends, json!([]), "blocking ends the friendship");
    assert_eq!(history(&app, &bob, &chat).await.as_array().unwrap().len(), 0, "still readable");

    // Unblocking restores nothing: the conversation stays locked until they are friends again.
    assert_eq!(call(&app, "DELETE", &url, &alice, Value::Null).await.0, StatusCode::NO_CONTENT);
    let (status, refused) = say(&app, &bob, &chat, hello.clone()).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(refused["error"].as_str().unwrap().starts_with("Vous n'êtes plus amis."));
    befriend(&app, &bob, bob_id, &alice, "alice").await;
    assert_eq!(say(&app, &bob, &chat, hello).await.0, StatusCode::OK);
}

#[tokio::test]
async fn leaving_and_deleting_an_account() {
    let app = app().await;
    let (alice, alice_id) = account(&app, "alice").await;
    let (bob, bob_id) = account(&app, "bob").await;
    let (eve, eve_id) = account(&app, "eve").await;
    befriend(&app, &alice, alice_id, &bob, "bob").await;
    befriend(&app, &alice, alice_id, &eve, "eve").await;
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

fn encode(image: image::RgbaImage, format: image::ImageFormat) -> Vec<u8> {
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(image).write_to(&mut std::io::Cursor::new(&mut out), format).unwrap();
    out
}

fn png(w: u32, h: u32) -> Vec<u8> {
    encode(image::RgbaImage::from_pixel(w, h, image::Rgba([40, 90, 200, 255])), image::ImageFormat::Png)
}

/// Two frames: red, then blue.
fn animated_gif() -> Vec<u8> {
    use image::{codecs::gif::GifEncoder, Frame, Rgba, RgbaImage};
    let mut out = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut out);
        let red = Frame::new(RgbaImage::from_pixel(40, 20, Rgba([255, 0, 0, 255])));
        let blue = Frame::new(RgbaImage::from_pixel(40, 20, Rgba([0, 0, 255, 255])));
        encoder.encode_frames(vec![red, blue]).unwrap();
    }
    out
}

#[tokio::test]
async fn profile_photo() {
    let app = app().await;
    let (alice, _) = account(&app, "alice").await;
    let (bob, _) = account(&app, "bob").await;
    let (_, photo) = upload(&app, &alice, "image/png", &png(8, 8)).await;
    let (_, sound) = upload(&app, &alice, "audio/ogg", b"la").await;
    let (_, broken) = upload(&app, &alice, "image/png", b"face").await;
    let (_, heic) = upload(&app, &alice, "image/heic", b"heic").await;
    let profile = |id: &Value| json!({"display_name": "Alice", "avatar_id": id});

    assert_eq!(call(&app, "PUT", "/api/me", &bob, profile(&photo["id"])).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "PUT", "/api/me", &alice, profile(&sound["id"])).await.0, StatusCode::BAD_REQUEST);
    let (status, refused) = call(&app, "PUT", "/api/me", &alice, profile(&broken["id"])).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "not decodable");
    assert_eq!(refused["error"], "Choisissez une image JPEG, PNG, WebP ou GIF de 8 Mo au plus.");
    assert_eq!(call(&app, "PUT", "/api/me", &alice, profile(&heic["id"])).await.0, StatusCode::BAD_REQUEST);
    let (status, user) = call(&app, "PUT", "/api/me", &alice, profile(&photo["id"])).await;
    assert_eq!((status, &user["avatar"], &user["avatar_still"]), (StatusCode::OK, &photo["id"], &Value::Null));

    let remove = json!({"display_name": "Alice", "remove_avatar": true});
    assert_eq!(call(&app, "PUT", "/api/me", &alice, remove).await.1["avatar"], Value::Null);
    assert_eq!(file_status(&app, &photo["id"]).await, StatusCode::NOT_FOUND);
}

/// Subscribes a fake WebSocket receiver for a user so tests can assert server events.
fn listen(state: &state::AppState, user: i64) -> tokio::sync::mpsc::Receiver<proto::ServerEvent> {
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    state.sockets.lock().unwrap().entry(user).or_default().push(tx);
    rx
}

fn drain(rx: &mut tokio::sync::mpsc::Receiver<proto::ServerEvent>) -> Vec<proto::ServerEvent> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        out.push(ev);
    }
    out
}

#[tokio::test]
async fn animated_avatar_gets_a_still() {
    let mut state = app_state("", "").await;
    std::sync::Arc::make_mut(&mut state.config).user_quota = 100_000;
    let app = api::router(state.clone());
    let (alice, _) = account(&app, "alice").await;
    let gif = animated_gif();
    assert!(gif.len() < 1000, "fits the test upload limit");
    // Declared as PNG on purpose: the server goes by the content.
    let (status, media) = upload(&app, &alice, "image/png", &gif).await;
    assert_eq!(status, StatusCode::OK, "{media}");
    let (status, user) = call(&app, "PUT", "/api/me", &alice, json!({"display_name": "A", "avatar_id": media["id"]})).await;
    assert_eq!(status, StatusCode::OK, "{user}");
    assert_eq!(user["avatar"], media["id"]);
    let still = user["avatar_still"].clone();
    assert!(still.is_string());

    let request = Request::builder().uri(format!("/api/media/{}", still.as_str().unwrap())).body(Body::empty()).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.headers()["content-type"], "image/png");
    let bytes = to_bytes(response.into_body(), 1 << 20).await.unwrap();
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).unwrap().to_rgba8();
    assert_eq!(image.dimensions(), (256, 256));
    assert_eq!(image.get_pixel(128, 128).0, [255, 0, 0, 255], "the first frame");
    let (_, avatar) = get_raw(&app, &format!("/api/media/{}", media["id"].as_str().unwrap())).await;
    assert_eq!(avatar, gif, "the animation itself is kept as is");
    let (mime,): (String,) = sqlx::query_as("SELECT mime FROM media WHERE id = ?")
        .bind(media["id"].as_str().unwrap())
        .fetch_one(&state.db)
        .await
        .unwrap();
    assert_eq!(mime, "image/gif");

    // A still photo replaces it: the old still goes.
    let (status, photo) = upload(&app, &alice, "image/png", &png(4, 4)).await;
    assert_eq!(status, StatusCode::OK, "{photo}");
    let (status, user) = call(&app, "PUT", "/api/me", &alice, json!({"display_name": "A", "avatar_id": photo["id"]})).await;
    assert_eq!(status, StatusCode::OK, "{user}");
    assert_eq!(user["avatar_still"], Value::Null);
    assert_eq!(file_status(&app, &still).await, StatusCode::NOT_FOUND);
    assert_eq!(file_status(&app, &media["id"]).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn profile_fields_and_banner() {
    let state = app_state("", "").await;
    let app = api::router(state.clone());
    let (alice, alice_id) = account(&app, "alice").await;
    let (bob, bob_id) = account(&app, "bob").await;
    let (eve, _) = account(&app, "eve").await;
    befriend(&app, &alice, alice_id, &bob, "bob").await;
    let mut bob_events = listen(&state, bob_id);
    let put = |body: Value| call(&app, "PUT", "/api/me", &alice, body);

    let long = "é".repeat(191);
    assert_eq!(put(json!({"display_name": "A", "bio": long})).await.1["error"], "Bio : 190 caractères et 6 lignes au plus.");
    assert_eq!(put(json!({"display_name": "A", "bio": "1\n2\n3\n4\n5\n6\n7"})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(put(json!({"display_name": "A", "name_font": "comic"})).await.1["error"], "Police inconnue.");
    assert_eq!(put(json!({"display_name": "A", "name_color": "red"})).await.1["error"], "Couleur inconnue.");
    assert_eq!(put(json!({"display_name": "A", "name_color": "#12345"})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(put(json!({"display_name": ""})).await.0, StatusCode::BAD_REQUEST);

    let bio = format!("  {}\r\nligne 2  ", "x".repeat(150));
    let (status, user) = put(json!({"display_name": "Alice", "bio": bio, "name_font": "bebas", "name_color": "#A1B2C3"})).await;
    assert_eq!(status, StatusCode::OK, "{user}");
    assert_eq!((user["name_font"].as_str(), user["name_color"].as_str()), (Some("bebas"), Some("#a1b2c3")));
    // Fields left out keep their value.
    let (_, user) = put(json!({"display_name": "Alice B"})).await;
    assert_eq!(user["name_font"], "bebas");

    let changed = drain(&mut bob_events);
    assert!(changed.iter().any(|e| matches!(e, proto::ServerEvent::UserChanged { user } if user.id == alice_id)));

    let (_, banner) = upload(&app, &alice, "image/png", &png(30, 10)).await;
    let (_, gif) = upload(&app, &alice, "image/gif", &animated_gif()).await;
    let (_, theirs) = upload(&app, &bob, "image/png", &png(3, 1)).await;
    let refused = put(json!({"display_name": "Alice", "banner_id": gif["id"]})).await;
    assert_eq!(refused.1["error"], "Choisissez une bannière JPEG, PNG ou WebP de 8 Mo au plus.");
    assert_eq!(put(json!({"display_name": "Alice", "banner_id": theirs["id"]})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(put(json!({"display_name": "Alice", "banner_id": banner["id"]})).await.0, StatusCode::OK);

    let url = format!("/api/users/{alice_id}");
    let (status, profile) = call(&app, "GET", &url, &bob, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{profile}");
    assert_eq!(profile["bio"], format!("{}\nligne 2", "x".repeat(150)));
    assert_eq!(profile["banner"], banner["id"]);
    assert_eq!(profile["relation"], "friend");
    assert!(profile["friends_since"].as_i64().unwrap() > 0 && profile["joined_at"].as_i64().unwrap() > 0);
    assert_eq!(profile["online"], false);
    assert_eq!(profile["direct_id"], Value::Null);
    assert_eq!(call(&app, "GET", &url, &eve, Value::Null).await.0, StatusCode::NOT_FOUND, "a stranger");
    assert_eq!(call(&app, "GET", &url, &alice, Value::Null).await.1["relation"], "me");
    call(&app, "POST", "/api/friends/requests", &eve, json!({"username": "alice"})).await;
    let (status, profile) = call(&app, "GET", &url, &eve, Value::Null).await;
    assert_eq!((status, profile["relation"].as_str()), (StatusCode::OK, Some("outgoing")), "a pending request shows it");
    assert_eq!(call(&app, "GET", &format!("/api/users/{}", 9999), &bob, Value::Null).await.0, StatusCode::NOT_FOUND);

    assert_eq!(put(json!({"display_name": "Alice", "remove_banner": true})).await.1["display_name"], "Alice");
    assert_eq!(file_status(&app, &banner["id"]).await, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn directory_lists_friends_only() {
    let app = app().await;
    let (alice, alice_id) = account(&app, "alice").await;
    let (bob, bob_id) = account(&app, "bob").await;
    let (carole, _) = account(&app, "carole").await;
    account(&app, "dan").await;
    befriend(&app, &alice, alice_id, &bob, "bob").await;
    befriend(&app, &alice, alice_id, &carole, "carole").await;
    let (_, all) = call(&app, "GET", "/api/users", &alice, Value::Null).await;
    let names: Vec<&str> = all.as_array().unwrap().iter().map(|u| u["username"].as_str().unwrap()).collect();
    assert_eq!(names, ["bob", "carole"], "not dan, a stranger");
    let (_, found) = call(&app, "GET", "/api/users?q=aro", &alice, Value::Null).await;
    assert_eq!(found.as_array().unwrap().len(), 1);
    assert_eq!(found[0]["username"], "carole");

    call(&app, "PUT", &format!("/api/blocks/{bob_id}"), &alice, Value::Null).await;
    let (_, all) = call(&app, "GET", "/api/users", &bob, Value::Null).await;
    assert_eq!(all, json!([]), "a block ends the friendship");
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
    // Pre-0.4.0 tables (lacking new columns and 0.4.0 tables).
    sqlx::raw_sql(
        "CREATE TABLE users (id INTEGER PRIMARY KEY, username TEXT NOT NULL, display_name TEXT NOT NULL,
             password_hash TEXT NOT NULL, created_at INTEGER NOT NULL);
         CREATE TABLE conversations (id INTEGER PRIMARY KEY, name TEXT NOT NULL DEFAULT '',
             is_group INTEGER NOT NULL DEFAULT 0, direct_key TEXT UNIQUE, created_at INTEGER NOT NULL);
         CREATE TABLE members (conversation_id INTEGER NOT NULL, user_id INTEGER NOT NULL,
             last_read INTEGER NOT NULL DEFAULT 0, PRIMARY KEY (conversation_id, user_id));
         CREATE TABLE messages (id INTEGER PRIMARY KEY, conversation_id INTEGER NOT NULL,
             sender_id INTEGER NOT NULL, body TEXT NOT NULL DEFAULT '', media_id TEXT, created_at INTEGER NOT NULL);
         CREATE TABLE blocks (user_id INTEGER NOT NULL, blocked_id INTEGER NOT NULL,
             created_at INTEGER NOT NULL, PRIMARY KEY (user_id, blocked_id));
         INSERT INTO users VALUES (1, 'alice', 'Alice', 'h', 100), (2, 'bob', 'Bob', 'h', 100),
             (3, 'carole', 'Carole', 'h', 100), (4, 'dan', 'Dan', 'h', 100);
         INSERT INTO conversations VALUES (10, '', 0, '1:2', 500), (11, '', 0, '1:3', 600), (12, 'G', 1, NULL, 700);
         INSERT INTO members VALUES (10, 1, 0), (10, 2, 0), (11, 1, 0), (11, 3, 0), (12, 1, 0), (12, 4, 0);
         INSERT INTO blocks VALUES (1, 3, 650);",
    )
    .execute(&db)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!("../schema.sql")).execute(&db).await.unwrap();
    state::migrate(&db).await.unwrap();
    state::migrate(&db).await.unwrap();
    sqlx::query("SELECT avatar, bio, banner, avatar_still, name_font, name_color FROM users")
        .fetch_all(&db)
        .await
        .unwrap();
    sqlx::query("SELECT reply_to, deleted, edited_at FROM messages")
        .fetch_all(&db)
        .await
        .unwrap();
    let pairs: Vec<(i64, i64, i64)> =
        sqlx::query_as("SELECT user_a, user_b, created_at FROM friendships ORDER BY user_a, user_b")
            .fetch_all(&db)
            .await
            .unwrap();
    assert_eq!(pairs, [(1, 2, 500)], "only unblocked 1:1 pair becomes friends, not blocked or group-only");
    // Deleting the friendship and re-running migrate must not recreate it.
    sqlx::query("DELETE FROM friendships").execute(&db).await.unwrap();
    state::migrate(&db).await.unwrap();
    let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM friendships").fetch_one(&db).await.unwrap();
    assert_eq!(count.0, 0);
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
    befriend(&app, &alice, alice_id, &bob, "bob").await;

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
    let (alice, alice_id) = account(&app, "alice").await;
    let (bob, bob_id) = account(&app, "bob").await;
    befriend(&app, &alice, alice_id, &bob, "bob").await;

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

#[tokio::test]
async fn friends_flow_groups_and_locked_chats() {
    let state = app_state("", "").await;
    let app = api::router(state.clone());
    let (alice, alice_id) = account(&app, "alice").await;
    let (bob, bob_id) = account(&app, "bob").await;
    let (eve, eve_id) = account(&app, "eve").await;

    // Strangers cannot start direct chats or add non-friends to groups.
    assert_eq!(direct(&app, &alice, bob_id).await.0, StatusCode::FORBIDDEN);
    let group = json!({"member_ids": [bob_id, eve_id], "name": "Club"});
    assert_eq!(call(&app, "POST", "/api/conversations", &alice, group.clone()).await.0, StatusCode::FORBIDDEN);

    // Validation & self-request & unknown user.
    assert_eq!(call(&app, "POST", "/api/friends/requests", &alice, json!({"username": "a"})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "POST", "/api/friends/requests", &alice, json!({"username": "@ALICE"})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "POST", "/api/friends/requests", &alice, json!({"username": "nobody"})).await.0, StatusCode::NOT_FOUND);

    // Send, idempotency, cancel, resend, decline.
    let (status, out) = call(&app, "POST", "/api/friends/requests", &alice, json!({"username": " @Bob "})).await;
    assert_eq!((status, out["status"].as_str()), (StatusCode::OK, Some("sent")));
    let (status, out) = call(&app, "POST", "/api/friends/requests", &alice, json!({"username": "bob"})).await;
    assert_eq!((status, out["status"].as_str()), (StatusCode::OK, Some("sent")));
    let (_, reqs) = call(&app, "GET", "/api/friends/requests", &bob, Value::Null).await;
    assert_eq!(reqs["incoming"].as_array().unwrap().len(), 1);
    assert_eq!(call(&app, "DELETE", &format!("/api/friends/requests/{bob_id}"), &alice, Value::Null).await.0, StatusCode::NO_CONTENT);
    let (_, reqs) = call(&app, "GET", "/api/friends/requests", &bob, Value::Null).await;
    assert_eq!(reqs["incoming"].as_array().unwrap().len(), 0);

    call(&app, "POST", "/api/friends/requests", &alice, json!({"username": "bob"})).await;
    assert_eq!(call(&app, "POST", &format!("/api/friends/requests/{alice_id}/decline"), &bob, Value::Null).await.0, StatusCode::NO_CONTENT);

    // Mutual requests auto-accept into friendship.
    call(&app, "POST", "/api/friends/requests", &alice, json!({"username": "bob"})).await;
    let (status, out) = call(&app, "POST", "/api/friends/requests", &bob, json!({"username": "alice"})).await;
    assert_eq!((status, out["status"].as_str()), (StatusCode::OK, Some("friends")));
    assert_eq!(call(&app, "POST", "/api/friends/requests", &alice, json!({"username": "bob"})).await.0, StatusCode::CONFLICT);

    // Direct chat works once friends; group requires all members to be friends.
    let (status, conv) = direct(&app, &alice, bob_id).await;
    assert_eq!(status, StatusCode::OK);
    let chat = conv["id"].clone();
    assert_eq!(call(&app, "POST", "/api/conversations", &alice, group.clone()).await.0, StatusCode::FORBIDDEN);
    befriend(&app, &alice, alice_id, &eve, "eve").await;
    assert_eq!(call(&app, "POST", "/api/conversations", &alice, group).await.0, StatusCode::OK);

    // Removing a friend locks the 1:1 chat for send, edit, react — history stays readable.
    let (_, msg) = say(&app, &alice, &chat, json!({"body": "coucou", "media_id": null})).await;
    assert_eq!(call(&app, "DELETE", &format!("/api/friends/{bob_id}"), &alice, Value::Null).await.0, StatusCode::NO_CONTENT);
    let (_, list) = call(&app, "GET", "/api/conversations", &alice, Value::Null).await;
    let direct_conv = list.as_array().unwrap().iter().find(|c| c["id"] == chat).unwrap();
    assert_eq!(direct_conv["locked"], true);
    assert_eq!(say(&app, &alice, &chat, json!({"body": "encore", "media_id": null})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "PUT", &format!("/api/messages/{}", msg["id"]), &alice, json!({"body": "modif"})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "PUT", &format!("/api/messages/{}/reaction", msg["id"]), &bob, json!({"emoji": "❤️"})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(history(&app, &bob, &chat).await.as_array().unwrap().len(), 1);

    // Blocked pair looks like a missing username on friend request.
    call(&app, "PUT", &format!("/api/blocks/{bob_id}"), &alice, Value::Null).await;
    let (status, err) = call(&app, "POST", "/api/friends/requests", &bob, json!({"username": "alice"})).await;
    assert_eq!((status, err["error"].as_str()), (StatusCode::NOT_FOUND, Some("Aucun membre avec cet identifiant.")));
}

#[tokio::test]
async fn edit_mentions_and_shared_media() {
    let app = app().await;
    let (alice, alice_id) = account(&app, "alice").await;
    let (bob, bob_id) = account(&app, "bob").await;
    let (eve, _) = account(&app, "eve").await;
    befriend(&app, &alice, alice_id, &bob, "bob").await;
    let chat = direct(&app, &alice, bob_id).await.1["id"].clone();

    let (_, msg) = say(&app, &alice, &chat, json!({"body": "salut @bob et @alice", "media_id": null})).await;
    assert_eq!(msg["mentions"], json!([bob_id]), "self-mention excluded");
    assert_eq!(msg["edited_at"], Value::Null);

    // Only sender can edit; empty text without media refused.
    let edit_url = format!("/api/messages/{}", msg["id"]);
    assert_eq!(call(&app, "PUT", &edit_url, &bob, json!({"body": "nope"})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&app, "PUT", &edit_url, &alice, json!({"body": "   "})).await.0, StatusCode::BAD_REQUEST);

    // Edit updates text, sets edited_at, recomputes mentions.
    let (status, edited) = call(&app, "PUT", &edit_url, &alice, json!({"body": "sans mention"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(edited["body"], "sans mention");
    assert!(edited["edited_at"].as_i64().is_some());
    assert_eq!(edited["mentions"], json!([]));

    // Shared media endpoint returns images/videos newest first, ignores audio/deleted, 404 to strangers.
    let (_, pic1) = upload(&app, &alice, "image/png", &png(4, 4)).await;
    let (_, audio) = upload(&app, &alice, "audio/ogg", b"ogg").await;
    let (_, pic2) = upload(&app, &bob, "image/png", &png(4, 4)).await;
    let (_, m1) = say(&app, &alice, &chat, json!({"body": "", "media_id": pic1["id"]})).await;
    say(&app, &alice, &chat, json!({"body": "", "media_id": audio["id"]})).await;
    let (_, m2) = say(&app, &bob, &chat, json!({"body": "", "media_id": pic2["id"]})).await;

    let (status, page) = call(&app, "GET", &format!("/api/users/{bob_id}/media"), &alice, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    let ids: Vec<i64> = page["items"].as_array().unwrap().iter().map(|i| i["message_id"].as_i64().unwrap()).collect();
    assert_eq!(ids, [m2["id"].as_i64().unwrap(), m1["id"].as_i64().unwrap()]);
    assert_eq!(call(&app, "GET", &format!("/api/users/{bob_id}/media"), &eve, Value::Null).await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn presence_grace_and_events() {
    let mut state = app_state("", "").await;
    state.offline_grace = std::time::Duration::from_millis(30);
    let app = api::router(state.clone());
    let (alice, alice_id) = account(&app, "alice").await;
    let (_bob, bob_id) = account(&app, "bob").await;
    befriend(&app, &alice, alice_id, &_bob, "bob").await;
    let mut bob_events = listen(&state, bob_id);

    crate::presence::activate(&state, alice_id).await;
    assert!(crate::presence::is_online(&state, alice_id));
    assert!(drain(&mut bob_events).iter().any(|e| matches!(e, proto::ServerEvent::Presence { user_id, online: true } if *user_id == alice_id)));

    crate::presence::deactivate(&state, alice_id);
    assert!(crate::presence::is_online(&state, alice_id), "grace window holds");
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    assert!(!crate::presence::is_online(&state, alice_id));
    assert!(drain(&mut bob_events).iter().any(|e| matches!(e, proto::ServerEvent::Presence { user_id, online: false } if *user_id == alice_id)));
}

#[tokio::test]
async fn stories_lifecycle_and_worker_api() {
    // Without worker token: photo stories work, video stories return 503.
    let no_worker = app_state("", "").await;
    let app_nw = api::router(no_worker);
    let (alice_nw, _) = account(&app_nw, "alice").await;
    let (_, vid_nw) = upload(&app_nw, &alice_nw, "video/mp4", b"fake-mp4").await;
    assert_eq!(
        call(&app_nw, "POST", "/api/stories", &alice_nw, json!({"media_id": vid_nw["id"]})).await.0,
        StatusCode::SERVICE_UNAVAILABLE
    );

    // With worker token: full lifecycle across public + internal routers.
    let state = app_state_with("", "", "wsecret").await;
    let app = api::router(state.clone());
    let worker = api::internal_router(state.clone());
    let (alice, alice_id) = account(&app, "alice").await;
    let (bob, _bob_id) = account(&app, "bob").await;
    let (eve, _) = account(&app, "eve").await;
    befriend(&app, &alice, alice_id, &bob, "bob").await;

    // Internal API rejects missing/wrong bearer token.
    assert_eq!(call(&worker, "POST", "/internal/jobs/claim", "wrong", Value::Null).await.0, StatusCode::UNAUTHORIZED);
    assert_eq!(call(&worker, "POST", "/internal/jobs/claim", "wsecret", Value::Null).await.0, StatusCode::NO_CONTENT);

    // Photo story goes live immediately.
    let (_, pic) = upload(&app, &alice, "image/png", &png(20, 30)).await;
    let (status, photo_story) = call(&app, "POST", "/api/stories", &alice, json!({"media_id": pic["id"]})).await;
    assert_eq!(status, StatusCode::OK, "{photo_story}");
    assert_eq!(photo_story["state"], "live");
    assert_eq!((&photo_story["width"], &photo_story["height"]), (&json!(20), &json!(30)));
    let photo_id = photo_story["id"].as_i64().unwrap();
    let photo_live_key = photo_story["key"].as_str().unwrap().to_string();

    // Friend sees it, stranger does not; mark seen works for friend only.
    let (_, bob_reels) = call(&app, "GET", "/api/stories", &bob, Value::Null).await;
    assert_eq!(bob_reels.as_array().unwrap().len(), 1);
    assert_eq!(bob_reels[0]["unseen"], true);
    let (_, eve_reels) = call(&app, "GET", "/api/stories", &eve, Value::Null).await;
    assert_eq!(eve_reels, json!([]));
    assert_eq!(call(&app, "POST", &format!("/api/stories/{photo_id}/seen"), &eve, Value::Null).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "POST", &format!("/api/stories/{photo_id}/seen"), &bob, Value::Null).await.0, StatusCode::NO_CONTENT);
    let (_, bob_reels) = call(&app, "GET", "/api/stories", &bob, Value::Null).await;
    assert_eq!(bob_reels[0]["unseen"], false);

    // Live media served with live key, rejected with bad key.
    assert_eq!(get_raw(&app, &format!("/api/stories/{photo_id}/media?k={photo_live_key}")).await.0, StatusCode::OK);
    assert_eq!(get_raw(&app, &format!("/api/stories/{photo_id}/media?k=bad")).await.0, StatusCode::FORBIDDEN);

    // Expire photo story with a forged clock -> moves to archive + queues archive job.
    let after_24h = photo_story["expires_at"].as_i64().unwrap() + 1;
    crate::stories::expire(&state, after_24h).await.unwrap();
    // Live key now returns 403; archive returns archive key that works.
    assert_eq!(get_raw(&app, &format!("/api/stories/{photo_id}/media?k={photo_live_key}")).await.0, StatusCode::FORBIDDEN);
    let (_, arch) = call(&app, "GET", "/api/stories/archive", &alice, Value::Null).await;
    assert_eq!(arch["stories"].as_array().unwrap().len(), 1);
    let arch_key = arch["stories"][0]["key"].as_str().unwrap().to_string();
    assert_eq!(get_raw(&app, &format!("/api/stories/{photo_id}/media?k={arch_key}")).await.0, StatusCode::OK);

    // Queue a video story (prepare job) while the archive job is already queued:
    // prepare MUST be claimed before archive, and archive=false skips archive jobs.
    let (_, vid) = upload(&app, &alice, "video/mp4", b"raw-video-bytes").await;
    let (status, vid_story) = call(&app, "POST", "/api/stories", &alice, json!({"media_id": vid["id"]})).await;
    assert_eq!((status, vid_story["state"].as_str()), (StatusCode::OK, Some("processing")));
    let vid_id = vid_story["id"].as_i64().unwrap();

    let (status, job) = call(&worker, "POST", "/internal/jobs/claim?archive=false", "wsecret", Value::Null).await;
    assert_eq!((status, job["kind"].as_str()), (StatusCode::OK, Some("prepare")), "prepare claimed first");
    let job_id = job["id"].as_i64().unwrap();

    // Outside archive hours (archive=false), no more jobs should be claimed while archive waits.
    assert_eq!(call(&worker, "POST", "/internal/jobs/claim?archive=false", "wsecret", Value::Null).await.0, StatusCode::NO_CONTENT);

    // Worker fetches input, uploads main + poster, reports done -> video goes live!
    let req = Request::builder()
        .uri(format!("/internal/jobs/{job_id}/input"))
        .header("authorization", "Bearer wsecret")
        .body(Body::empty())
        .unwrap();
    let (status, bytes) = send(&worker, req).await;
    assert_eq!((status, bytes.as_slice()), (StatusCode::OK, &b"raw-video-bytes"[..]));

    for (slot, mime, payload) in [("main", "video/mp4", &b"MP4OUT"[..]), ("poster", "image/jpeg", &b"JPGOUT"[..])] {
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/internal/jobs/{job_id}/output/{slot}"))
            .header("authorization", "Bearer wsecret")
            .header("content-type", mime)
            .body(Body::from(payload.to_vec()))
            .unwrap();
        assert_eq!(send(&worker, req).await.0, StatusCode::NO_CONTENT);
    }
    assert_eq!(
        call(&worker, "POST", &format!("/internal/jobs/{job_id}/done"), "wsecret", json!({"width": 720, "height": 1280, "duration_ms": 5200})).await.0,
        StatusCode::NO_CONTENT
    );

    let (_, bob_reels) = call(&app, "GET", "/api/stories", &bob, Value::Null).await;
    assert_eq!(bob_reels[0]["stories"][0]["id"], vid_id);
    assert_eq!(bob_reels[0]["stories"][0]["state"], "live");
    assert_eq!(bob_reels[0]["stories"][0]["has_poster"], true);
    let vid_key = bob_reels[0]["stories"][0]["key"].as_str().unwrap();
    assert_eq!(get_raw(&app, &format!("/api/stories/{vid_id}/media?k={vid_key}")).await, (StatusCode::OK, b"MP4OUT".to_vec()));
    assert_eq!(get_raw(&app, &format!("/api/stories/{vid_id}/poster?k={vid_key}")).await, (StatusCode::OK, b"JPGOUT".to_vec()));

    // Inside archive hours (archive=true), the waiting archive job is claimed.
    let (status, ajob) = call(&worker, "POST", "/internal/jobs/claim?archive=true", "wsecret", Value::Null).await;
    assert_eq!((status, ajob["kind"].as_str()), (StatusCode::OK, Some("archive")));

    // Reshare from archive goes live again, then delete removes both original and reshare.
    let (status, reshared) = call(&app, "POST", &format!("/api/stories/{photo_id}/reshare"), &alice, Value::Null).await;
    assert_eq!((status, reshared["state"].as_str(), reshared["reshared_from"].as_i64()), (StatusCode::OK, Some("live"), Some(photo_id)));
    assert_eq!(call(&app, "DELETE", &format!("/api/stories/{photo_id}"), &bob, Value::Null).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "DELETE", &format!("/api/stories/{photo_id}"), &alice, Value::Null).await.0, StatusCode::NO_CONTENT);
    let (_, arch) = call(&app, "GET", "/api/stories/archive", &alice, Value::Null).await;
    assert_eq!(arch["stories"], json!([]));
}
