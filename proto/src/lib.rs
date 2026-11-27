//! Types shared by the server and the app.

use serde::{Deserialize, Serialize};

/// The reactions a message accepts.
pub const REACTIONS: [&str; 6] = ["👍", "❤️", "😂", "😮", "😢", "🙏"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    /// Media id of the profile photo.
    #[serde(default)]
    pub avatar: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Register {
    pub username: String,
    pub display_name: String,
    pub password: String,
    /// Only checked when the server requires one.
    #[serde(default)]
    pub invite: String,
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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Profile {
    pub display_name: String,
    /// A new profile photo, or none to keep the current one.
    #[serde(default)]
    pub avatar_id: Option<String>,
    #[serde(default)]
    pub remove_avatar: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DeleteAccount {
    pub password: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Media {
    /// Random, unguessable identifier. Knowing it grants read access.
    pub id: String,
    pub mime: String,
    pub name: String,
    pub size: i64,
}

/// A song of the goofysound (Navidrome) library.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Track {
    /// Navidrome song id.
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Seconds.
    pub duration: i64,
    pub has_cover: bool,
    /// Signed by the server: grants `GET /api/music/{id}/stream?k=` and `/cover?k=`
    /// without a bearer header, so `<audio>` and `<img>` can use them.
    pub key: String,
}

/// What a member is listening to right now.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Activity {
    pub user_id: i64,
    pub track: Track,
    /// Player name reported by Navidrome, e.g. "SelfHost-hub".
    pub player: String,
    /// Unix time in milliseconds when this track was first seen playing.
    pub since: i64,
}

/// The SelfHost-hub services a Goofy Chat account is linked to.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Links {
    /// goofysound (Navidrome) username, when linked.
    pub navidrome: Option<String>,
    /// FileBrowser username, when linked.
    pub photos: Option<String>,
}

/// `POST /api/links/navidrome` and `POST /api/links/photos`. Verified against the
/// service before anything is stored.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LinkAccount {
    pub username: String,
    pub password: String,
}

/// A photo or video backed up by SelfHost-hub, from `GET /api/photos`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Photo {
    /// Path inside the user's FileBrowser space.
    pub path: String,
    pub name: String,
    pub is_video: bool,
    pub size: i64,
    /// Unix time in milliseconds: taken date when known, else the year/month folder or modified time.
    pub taken: i64,
    /// Signed by the server: grants `GET /api/photos/thumb?path=&k=` without a bearer header.
    pub key: String,
}

/// A page of `GET /api/photos`, newest first.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct PhotoPage {
    pub photos: Vec<Photo>,
    /// Opaque cursor for the next page, none at the end.
    pub next: Option<String>,
}

/// `POST /api/photos/import`: copies a backed-up photo into Goofy Chat and returns its [`Media`].
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImportPhoto {
    pub path: String,
}

/// The message another one answers.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Quote {
    pub id: i64,
    /// Empty when the original no longer exists.
    pub sender_name: String,
    pub body: String,
    pub mime: Option<String>,
    pub deleted: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Reaction {
    pub user_id: i64,
    pub emoji: String,
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
    #[serde(default)]
    pub reply: Option<Quote>,
    #[serde(default)]
    pub reactions: Vec<Reaction>,
    /// Deleted by its sender: body and media are empty.
    #[serde(default)]
    pub deleted: bool,
    /// A shared song.
    #[serde(default)]
    pub track: Option<Track>,
}

/// How far a member has read a conversation.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ReadMark {
    pub user_id: i64,
    pub message_id: i64,
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
    #[serde(default)]
    pub reads: Vec<ReadMark>,
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
    #[serde(default)]
    pub reply_to: Option<i64>,
    /// Navidrome song id to share; needs a linked goofysound account.
    #[serde(default)]
    pub track_id: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct React {
    /// One of [`REACTIONS`], or none to remove the reaction.
    pub emoji: Option<String>,
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
    Typing { conversation_id: i64 },
    Ping,
}

/// Sent by the server over the WebSocket.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Ready,
    Message { message: Message },
    /// An existing message changed: reaction or deletion.
    Update { message: Message },
    /// A conversation appeared, changed or disappeared.
    Conversation { id: i64 },
    Read { conversation_id: i64, mark: ReadMark },
    Typing { conversation_id: i64, user_id: i64 },
    /// A member started or stopped listening (none). Sent to everyone who shares a
    /// conversation with them, unless blocked either way.
    Activity { user_id: i64, activity: Option<Activity> },
    Call { from: User, signal: CallSignal },
    Pong,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ApiError {
    pub error: String,
}
