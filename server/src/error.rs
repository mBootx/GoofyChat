use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

/// An error with a user-facing French message.
#[derive(Debug)]
pub struct Error(pub StatusCode, pub String);

pub type Result<T> = std::result::Result<T, Error>;

pub fn bad(msg: &str) -> Error {
    Error(StatusCode::BAD_REQUEST, msg.into())
}

pub fn unauthorized() -> Error {
    Error(StatusCode::UNAUTHORIZED, "Session expirée, reconnectez-vous.".into())
}

pub fn not_found() -> Error {
    Error(StatusCode::NOT_FOUND, "Introuvable.".into())
}

pub fn internal(e: impl std::fmt::Display) -> Error {
    tracing::error!("{e}");
    Error(StatusCode::INTERNAL_SERVER_ERROR, "Erreur du serveur.".into())
}

impl From<sqlx::Error> for Error {
    fn from(e: sqlx::Error) -> Self {
        internal(e)
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        internal(e)
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        (self.0, Json(proto::ApiError { error: self.1 })).into_response()
    }
}
