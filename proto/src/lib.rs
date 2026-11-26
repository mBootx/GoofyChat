//! Types shared by the server and the app.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub display_name: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Register {
    pub username: String,
    pub display_name: String,
    pub password: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Login {
    pub username: String,
    pub password: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Session {
    pub token: String,
    pub user: User,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Media {
    /// Random, unguessable identifier. Knowing it grants read access.
    pub id: String,
    pub mime: String,
    pub name: String,
    pub size: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Message {
    pub id: i64,
    pub conversation_id: i64,
    pub sender: User,
    pub body: String,
    pub media: Option<Media>,
    /// Unix time in milliseconds.
    pub created_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Conversation {
    pub id: i64,
    /// Empty for a one-to-one conversation.
    pub name: String,
    pub is_group: bool,
    pub members: Vec<User>,
    pub last_message: Option<Message>,
    pub unread: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NewConversation {
    pub member_ids: Vec<i64>,
    /// A name makes it a group.
    pub name: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NewMessage {
    pub body: String,
    pub media_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CallAction {
    Invite,
    Accept,
    Reject,
    Offer,
    Answer,
    Ice,
    Hangup,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct CallSignal {
    pub call_id: String,
    pub conversation_id: i64,
    pub action: CallAction,
    #[serde(default)]
    pub video: bool,
    /// SDP or ICE candidate, opaque to the server.
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// Sent by a client over the WebSocket.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientEvent {
    /// Must be the first frame.
    Auth { token: String },
    Call { signal: CallSignal },
    Ping,
}

/// Sent by the server over the WebSocket.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Ready,
    Message { message: Message },
    Conversation { id: i64 },
    Call { from: User, signal: CallSignal },
    Pong,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ApiError {
    pub error: String,
}
