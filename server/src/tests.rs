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
    let data_dir = std::env::temp_dir().join(format!("chat-test-{}", crate::auth::random_hex(8)));
    let config = state::Config {
        data_dir,
        max_upload: 1000,
        user_quota: 1500,
        total_quota: 1_000_000,
        turn_host: "turn.test".into(),
        turn_secret: "secret".into(),
        invite_code: invite_code.into(),
    };
    api::router(state::AppState::new(config).await.unwrap())
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

#[tokio::test]
async fn registration_is_rate_limited() {
    let app = app().await;
    for i in 0..5 {
        account(&app, &format!("user{i}")).await;
    }
    let body = json!({"username": "user9", "display_name": "x", "password": "motdepasse"});
    assert_eq!(call(&app, "POST", "/api/register", "", body).await.0, StatusCode::TOO_MANY_REQUESTS);
}
