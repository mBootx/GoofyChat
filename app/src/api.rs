//! REST client.

use gloo_net::http::{Request, RequestBuilder, Response};
use proto::*;
use serde::{de::DeserializeOwned, Serialize};

/// Override at build time with CHAT_SERVER=https://example.org
pub const SERVER: &str = match option_env!("CHAT_SERVER") {
    Some(s) => s,
    None => "https://goofychat.duckdns.org",
};

#[derive(Debug, Clone)]
pub struct Failure {
    pub status: u16,
    pub message: String,
}

fn offline<E>(_: E) -> Failure {
    Failure { status: 0, message: "Serveur injoignable. Vérifiez votre connexion.".into() }
}

async fn check(response: Response) -> Result<Response, Failure> {
    if response.ok() {
        return Ok(response);
    }
    let status = response.status();
    let message = response
        .json::<ApiError>()
        .await
        .map(|e| e.error)
        .unwrap_or_else(|_| format!("Erreur {status}"));
    Err(Failure { status, message })
}

fn auth(builder: RequestBuilder, token: &str) -> RequestBuilder {
    builder.header("Authorization", &format!("Bearer {token}"))
}

pub async fn get<T: DeserializeOwned>(token: &str, path: &str) -> Result<T, Failure> {
    let response = auth(Request::get(&format!("{SERVER}{path}")), token)
        .send()
        .await
        .map_err(offline)?;
    check(response).await?.json().await.map_err(offline)
}

async fn post_raw<B: Serialize>(token: &str, path: &str, body: &B) -> Result<Response, Failure> {
    let response = auth(Request::post(&format!("{SERVER}{path}")), token)
        .json(body)
        .map_err(offline)?
        .send()
        .await
        .map_err(offline)?;
    check(response).await
}

pub async fn post<B: Serialize, T: DeserializeOwned>(token: &str, path: &str, body: &B) -> Result<T, Failure> {
    post_raw(token, path, body).await?.json().await.map_err(offline)
}

/// For endpoints that answer with no content.
pub async fn post_empty(token: &str, path: &str) -> Result<(), Failure> {
    post_raw(token, path, &()).await.map(|_| ())
}

pub fn media_url(id: &str) -> String {
    format!("{SERVER}/api/media/{id}")
}
