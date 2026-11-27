//! Types shared by the server and the app.

use serde::{Deserialize, Serialize};

/// The reactions a message accepts.
pub const REACTIONS: [&str; 6] = ["👍", "❤️", "😂", "😮", "😢", "🙏"];

/// The reaction a double tap gives (and takes back).
pub const LIKE: &str = "❤️";

/// A font bundled with the app (SIL OFL, woff2 in `app/fonts/`). See `docs/contract-0.4.0.md`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Font {
    /// Stored in `User::name_font` and in the per-device look; CSS class `font-<id>`.
    pub id: &'static str,
    /// Shown in the picker.
    pub name: &'static str,
    /// Readable enough for message text, not only for names.
    pub body: bool,
}

/// The bundled fonts. An empty id means the system font.
pub const FONTS: [Font; 10] = [
    Font { id: "inter", name: "Inter", body: true },
    Font { id: "nunito", name: "Nunito", body: true },
    Font { id: "atkinson", name: "Atkinson Hyperlegible", body: true },
    Font { id: "lora", name: "Lora", body: true },
    Font { id: "space-mono", name: "Space Mono", body: true },
    Font { id: "comfortaa", name: "Comfortaa", body: true },
    Font { id: "caveat", name: "Caveat", body: true },
    Font { id: "pacifico", name: "Pacifico", body: false },
    Font { id: "press-start", name: "Press Start 2P", body: false },
    Font { id: "bebas", name: "Bebas Neue", body: false },
];

/// Suggested name colours shown as swatches in the picker (any `#rrggbb` is accepted).
/// Empty means the default text colour.
pub const NAME_COLORS: [&str; 12] = [
    "#e5484d", "#f76b15", "#d69e00", "#46a758", "#12a594", "#0d93c9",
    "#3e63dd", "#6e56cf", "#ab4aba", "#e93d82", "#a07553", "#7c7f8a",
];

/// `true` for "" (system font) and for the id of a bundled font.
pub fn valid_font(id: &str) -> bool {
    id.is_empty() || FONTS.iter().any(|f| f.id == id)
}

/// `true` for "" (default colour) and for any `#rrggbb` colour (6 hex digits, either case;
/// the server stores it lowercase).
pub fn valid_name_color(color: &str) -> bool {
    color.is_empty()
        || (color.len() == 7
            && color.starts_with('#')
            && color[1..].bytes().all(|b| b.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_colors() {
        assert!(valid_name_color(""));
        assert!(valid_name_color("#a1b2c3"));
        assert!(valid_name_color("#A1B2C3"));
        assert!(NAME_COLORS.iter().all(|c| valid_name_color(c)));
        for bad in ["a1b2c3", "#a1b2c", "#a1b2c3d", "#g1b2c3", "red", "#fff", " #a1b2c3"] {
            assert!(!valid_name_color(bad), "{bad}");
        }
    }

    #[test]
    fn fonts() {
        assert!(valid_font("") && valid_font("bebas"));
        assert!(!valid_font("comic-sans"));
    }
}

/// Shared limits, checked by the server and pre-checked by the app.
pub mod limits {
    pub const MB: i64 = 1024 * 1024;
    /// Characters of a profile bio.
    pub const BIO_CHARS: usize = 190;
    /// Lines of a profile bio.
    pub const BIO_LINES: usize = 6;
    /// Profile photo, still or animated (GIF, WebP).
    pub const AVATAR_BYTES: i64 = 8 * MB;
    /// Width and height of an avatar or banner image.
    pub const IMAGE_SIDE: u32 = 4096;
    pub const BANNER_BYTES: i64 = 8 * MB;
    /// Side of the still first frame generated for an animated avatar.
    pub const AVATAR_STILL_SIDE: u32 = 256;
    pub const STORY_PHOTO_BYTES: i64 = 15 * MB;
    pub const STORY_VIDEO_BYTES: i64 = 60 * MB;
    pub const STORY_VIDEO_MS: i64 = 30_000;
    /// How long a photo story stays on screen in the viewer.
    pub const STORY_PHOTO_MS: i64 = 5_000;
    /// How long a story stays visible to friends.
    pub const STORY_LIFETIME_MS: i64 = 24 * 3600 * 1000;
    /// Live (or processing) stories per author at once.
    pub const STORIES_LIVE: usize = 30;
    /// Long side of a photo story as uploaded by the app (it re-encodes to JPEG first).
    pub const STORY_PHOTO_SIDE: u32 = 1920;
    /// Mentions stored per message.
    pub const MENTIONS: usize = 20;
    /// Outgoing friend requests waiting at once.
    pub const PENDING_REQUESTS: usize = 50;
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    /// Media id of the profile photo. May be animated (GIF or animated WebP).
    #[serde(default)]
    pub avatar: Option<String>,
    /// Media id of a still PNG of the first frame, only when `avatar` is animated.
    /// Lists show it; profiles (and hover on desktop) show `avatar`.
    #[serde(default)]
    pub avatar_still: Option<String>,
    /// One of [`FONTS`] ids, or empty for the system font.
    #[serde(default)]
    pub name_font: String,
    /// One of [`NAME_COLORS`], or empty for the default colour.
    #[serde(default)]
    pub name_color: String,
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

/// `PUT /api/me`. Every field but `display_name` is optional: none (or false) keeps
/// the current value, so 0.3.0 apps keep working.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Profile {
    pub display_name: String,
    /// A new profile photo, or none to keep the current one.
    #[serde(default)]
    pub avatar_id: Option<String>,
    #[serde(default)]
    pub remove_avatar: bool,
    /// New bio (empty clears it), at most [`limits::BIO_CHARS`] characters and [`limits::BIO_LINES`] lines.
    #[serde(default)]
    pub bio: Option<String>,
    /// A new banner image (JPEG, PNG or WebP), or none to keep the current one.
    #[serde(default)]
    pub banner_id: Option<String>,
    #[serde(default)]
    pub remove_banner: bool,
    /// "" or one of [`FONTS`] ids.
    #[serde(default)]
    pub name_font: Option<String>,
    /// "" or one of [`NAME_COLORS`].
    #[serde(default)]
    pub name_color: Option<String>,
}

/// How the caller relates to another user.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Relation {
    #[default]
    None,
    /// The caller's own profile.
    Me,
    Friend,
    /// The caller sent a friend request that is waiting.
    Outgoing,
    /// This user sent the caller a friend request that is waiting.
    Incoming,
}

/// `GET /api/users/{id}`: the full profile.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct UserProfile {
    pub user: User,
    pub bio: String,
    /// Media id of the banner image.
    pub banner: Option<String>,
    pub relation: Relation,
    /// Unix ms, when `relation` is `Friend`.
    pub friends_since: Option<i64>,
    /// Unix ms of the account creation.
    pub joined_at: i64,
    /// Only reported to friends (and to oneself); false otherwise.
    pub online: bool,
    /// The one-to-one conversation between the caller and this user, if any (it may be locked).
    pub direct_id: Option<i64>,
}

/// An entry of `GET /api/friends`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Friend {
    pub user: User,
    /// Unix ms when the friendship started.
    pub since: i64,
    pub online: bool,
    /// The one-to-one conversation with this friend, if it exists.
    pub direct_id: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FriendRequest {
    /// The other person: the sender for incoming requests, the recipient for outgoing ones.
    pub user: User,
    pub created_at: i64,
}

/// `GET /api/friends/requests`, newest first in each list.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct FriendRequests {
    pub incoming: Vec<FriendRequest>,
    pub outgoing: Vec<FriendRequest>,
}

/// `POST /api/friends/requests`: by exact username (case-insensitive, a leading "@" is ignored).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NewFriendRequest {
    pub username: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FriendRequestStatus {
    /// The request is waiting for the other person.
    Sent,
    /// The other person had already asked: you are now friends.
    Friends,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FriendRequestOutcome {
    pub status: FriendRequestStatus,
    pub user: User,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FriendNoticeKind {
    /// `user` sent you a friend request.
    Request,
    /// `user` accepted your friend request.
    Accepted,
}

/// Why a [`ServerEvent::Friends`] was sent, when the app should tell the user.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FriendNotice {
    pub kind: FriendNoticeKind,
    pub user: User,
}

/// A photo or video exchanged in the one-to-one conversation of two users.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SharedMedia {
    pub message_id: i64,
    pub sender_id: i64,
    pub media: Media,
    pub created_at: i64,
}

/// `GET /api/users/{id}/media?before=`, newest first.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct SharedMediaPage {
    pub items: Vec<SharedMedia>,
    /// Pass as `before` for the next page; none at the end.
    pub next: Option<i64>,
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
    /// Unix ms of the last edit by the sender; none if never edited.
    #[serde(default)]
    pub edited_at: Option<i64>,
    /// Members mentioned with "@username" in the body (parsed by the server, sender excluded).
    #[serde(default)]
    pub mentions: Vec<i64>,
}

/// `PUT /api/messages/{id}`: new text of one of the caller's messages.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EditMessage {
    pub body: String,
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
    /// One-to-one conversation whose two members are not friends (or blocked either
    /// way): readable, but sending, editing, reacting and calling are refused.
    #[serde(default)]
    pub locked: bool,
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
    /// Also used mid-call to renegotiate (screen sharing).
    Offer,
    Answer,
    Ice,
    Hangup,
    /// Screen sharing started or stopped. Payload: [`ScreenShare`].
    Screen,
}

/// Payload of [`CallAction::Screen`], opaque to the server.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ScreenShare {
    pub on: bool,
    /// `MediaStream.id` of the shared screen, so the receiver can tell it from the camera.
    #[serde(default)]
    pub stream: String,
}

// ---- stories ----

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum StoryState {
    /// A video being checked and transcoded by the worker. Only its author sees it.
    #[default]
    Processing,
    /// Visible to friends until `expires_at`.
    Live,
    /// Past its 24 hours: in the author's private archive.
    Archived,
    /// The worker refused or could not process it. Only its author sees it.
    Failed,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Story {
    pub id: i64,
    /// The author.
    pub user_id: i64,
    pub is_video: bool,
    pub state: StoryState,
    pub created_at: i64,
    /// Unix ms; meaningful once live.
    pub expires_at: i64,
    /// Video length; [`limits::STORY_PHOTO_MS`] for photos.
    pub duration_ms: i64,
    pub width: i64,
    pub height: i64,
    /// Videos have a JPEG poster (`/api/stories/{id}/poster?k=`).
    pub has_poster: bool,
    /// Signs `GET /api/stories/{id}/media?k=` and `/poster?k=`: a live key (works only
    /// while the story is live) for friends, an archive key in the author's archive.
    pub key: String,
    /// The caller has viewed it.
    pub seen: bool,
    /// The archived copy has been compressed by the worker.
    pub compressed: bool,
    /// For a story shared again from the archive: the archived original.
    pub reshared_from: Option<i64>,
}

/// One author's live stories, oldest first.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct StoryReel {
    pub user: User,
    pub stories: Vec<Story>,
    /// At least one story the caller has not viewed.
    pub unseen: bool,
}

/// `POST /api/stories`: publishes an uploaded file (`POST /api/media` first).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct NewStory {
    pub media_id: String,
}

/// `GET /api/stories/archive?before=`, newest first.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct StoryPage {
    pub stories: Vec<Story>,
    pub next: Option<i64>,
}

/// Types of the internal API between the chat server and the stories worker
/// (`/internal/...` on `WORKER_BIND`, bearer `WORKER_TOKEN`). Never public.
pub mod worker {
    use serde::{Deserialize, Serialize};

    #[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum JobKind {
        /// A new video story: probe, refuse if too long, transcode for playback, make a poster.
        Prepare,
        /// An expired story: compress heavily for the archive (photo or video).
        Archive,
    }

    /// Encoder settings chosen by the server; the worker applies them as given.
    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    pub struct EncodeProfile {
        /// Longest side of the output, in pixels (never upscaled).
        pub max_side: u32,
        /// Frame rate cap (video).
        pub fps: u32,
        /// x264 CRF (video).
        pub crf: u32,
        /// x264 maxrate in kbit/s, bufsize twice that (video).
        pub max_kbps: u32,
        /// AAC bitrate in kbit/s (video).
        pub audio_kbps: u32,
        /// 1 or 2 (video).
        pub audio_channels: u32,
        /// ffmpeg `-q:v` for JPEG output (photo, poster): 2 best .. 31 worst.
        pub jpeg_q: u32,
        /// Longest side of the poster (video).
        pub poster_side: u32,
        /// Inputs longer than this (plus 500 ms tolerance) are refused (video).
        pub max_duration_ms: i64,
    }

    /// `POST /internal/jobs/claim` answer (204 when there is nothing to do).
    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    pub struct WorkerJob {
        pub id: i64,
        pub kind: JobKind,
        pub is_video: bool,
        /// Mime type of `GET /internal/jobs/{id}/input`.
        pub input_mime: String,
        pub profile: EncodeProfile,
    }

    /// `POST /internal/jobs/{id}/done`, after the outputs were uploaded.
    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    pub struct WorkerResult {
        pub width: i64,
        pub height: i64,
        /// 0 for photos.
        pub duration_ms: i64,
    }

    /// `POST /internal/jobs/{id}/fail`.
    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    pub struct WorkerFailure {
        /// For the server log only (English).
        pub error: String,
        /// A transient failure worth retrying (at most 3 attempts in total).
        pub retry: bool,
    }
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
    /// The interface is in use on this device (sent after `Ready` and on every change).
    /// Sockets that never send it (the Android background service, 0.3.0 apps) do not
    /// count as online.
    Active { active: bool },
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
    /// A member started or stopped listening (none). Sent to their friends and to
    /// everyone who shares a conversation with them, unless blocked either way.
    Activity { user_id: i64, activity: Option<Activity> },
    Call { from: User, signal: CallSignal },
    Pong,
    /// The friends list or the friend requests of the recipient changed: reload
    /// `/api/friends` and `/api/friends/requests` (and the stories).
    Friends {
        #[serde(default)]
        notice: Option<FriendNotice>,
    },
    /// A friend came online or went offline.
    Presence { user_id: i64, online: bool },
    /// Someone visible to the recipient changed their name, photo, font or colour.
    UserChanged { user: User },
    /// A story appeared or changed state. Friends receive live stories; the author's
    /// devices also receive processing, failed and archived ones.
    Story { story: Story, user: User },
    /// A story was deleted or left the live list (expired).
    StoryGone { id: i64, user_id: i64 },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ApiError {
    pub error: String,
}
