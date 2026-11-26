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
    let data_dir = std::env::temp_dir().join(format!("chat-test-{}", crate::auth::random_hex(8)));
    let config = state::Config {
        data_dir,
        max_upload: 1000,
        user_quota: 1500,
        total_quota: 1_000_000,
        turn_host: "turn.test".into(),
        turn_secret: "secret".into(),
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

#[tokio::test]
async fn registration_is_rate_limited() {
    let app = app().await;
    for i in 0..5 {
        account(&app, &format!("user{i}")).await;
    }
    let body = json!({"username": "user9", "display_name": "x", "password": "motdepasse"});
    assert_eq!(call(&app, "POST", "/api/register", "", body).await.0, StatusCode::TOO_MANY_REQUESTS);
}
