# Goofy Chat 0.4.0: implementation contract

Status: agreed design, written 2026-11-27 from `docs/plan-0.4.0.md` (authoritative for the
feature list). This file is authoritative for *how*: data, API, events, media, worker, app
structure and the work split. A package that needs to deviate updates this file first, in a
section it owns, and says so in its report.

Conventions (unchanged from 0.3.0):
- Shared types live in `proto/src/lib.rs` (already written for 0.4.0, see §11). JSON field names
  are the Rust field names (snake_case); enums are snake_case strings.
- Errors: `{"error": "<French message>"}`. 401 only for a bad session (the app signs out on it).
- Bearer auth on every `/api/*` route except `/api/register`, `/api/login`, `/health`, the
  signed `k=` URLs and `/api/media/{id}` (capability URL, as today).
- UI text French; code comments and docs English.
- Times are Unix milliseconds (`i64`).

---------------------------------------------------------------------------------------------

## 1. Decisions taken here (beyond the plan)

| Topic | Decision |
|---|---|
| Migration of existing pairs | Every pair of users who share a **one-to-one** conversation becomes friends (friendship `created_at` = the conversation's `created_at`), unless one blocked the other. Nothing breaks for current contacts. Pairs who only share a group do not become friends. |
| Removing a friend / blocked pair | Their one-to-one conversation stays readable but is **locked**: no new messages, edits, reactions or calls until they are friends again (`Conversation.locked`). |
| Groups | Creating a group requires every other member to be a friend of the creator. Once in a group, members talk to each other whether friends or not (unchanged). |
| Blocking | Blocking deletes the friendship and any pending request, both ways. Unblocking restores nothing. |
| Friend request to someone who blocked you (or whom you blocked) | 404 "Aucun membre avec cet identifiant.", same as a missing username. |
| Mutual requests | A request to someone who already asked you makes you friends at once. |
| "Personnes" directory | Removed from the app. `GET /api/users?q=` now returns only the caller's friends (kept for the group picker and for 0.3.0 apps). |
| Name colour | Free colour picker, `#rrggbb` (see section 13; `NAME_COLORS` are only suggested swatches). |
| Fonts | 10 bundled SIL OFL fonts (`proto::FONTS`, §8.7); names may use all 10, message text only the 7 with `body: true`. |
| Mentions | The server parses `@username` in the body against the conversation's members (sender excluded). A "ping" from the long-press menu is a reply to that message whose text starts with `@username `. |
| Edits | Text of one's own non-deleted message, any time, unlimited; "modifié" label; edits never notify. |
| In-chat camera | Photo only (tap the shutter), then a preview with a single "Envoyer" tap (or "Reprendre"). The photo is a normal media message: kept forever, like every message photo. |
| Stories | Photo or video ≤ 30 s. Photos go live at once; videos go live after the worker has transcoded them (about 10 to 40 s). After 24 h they move to the author's private archive, compressed by the worker. Reshares from the archive live 24 h again, then disappear (the archive keeps the original). The author can delete any story. No captions, no viewer list, no replies (later). |
| Presence | Online = at least one app interface open and in use: the desktop app counts as soon as it runs (even in the tray, like Discord); a phone counts while the app is on screen. The Android background service never counts. Visible to friends only. |
| Listening activity audience | Friends plus everyone sharing a conversation (was: conversation co-members only), minus blocks. |

---------------------------------------------------------------------------------------------

## 2. Database

Mechanism (as in 0.3.0): `server/schema.sql` runs on every start (`CREATE TABLE/INDEX IF NOT
EXISTS`); columns added to **existing** tables are also listed in `COLUMNS` of
`state::migrate` (`ALTER TABLE ... ADD COLUMN` when `pragma_table_info` lacks them); indexes on
added columns are created in `migrate` after the columns. New for 0.4.0: one-time **data**
migrations, guarded by a row in the new `meta` table and run in a transaction inside `migrate`.

### 2.1 New columns (add to `schema.sql` **and** to `COLUMNS`)

| Table | Column | Type | Meaning |
|---|---|---|---|
| users | bio | `TEXT NOT NULL DEFAULT ''` | ≤ 190 chars, ≤ 6 lines |
| users | banner | `TEXT` | media id (JPEG/PNG/WebP) |
| users | avatar_still | `TEXT` | media id of the PNG first frame when `avatar` is animated, else NULL |
| users | name_font | `TEXT NOT NULL DEFAULT ''` | "" or a `FONTS` id |
| users | name_color | `TEXT NOT NULL DEFAULT ''` | "" or a `NAME_COLORS` value |
| messages | edited_at | `INTEGER` | ms of the last edit |

### 2.2 New tables (in `schema.sql`)

```sql
-- One-time data migrations already applied.
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

-- One row per pair, smaller id first.
CREATE TABLE IF NOT EXISTS friendships (
    user_a     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    user_b     INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (user_a, user_b),
    CHECK (user_a < user_b)
);
CREATE INDEX IF NOT EXISTS idx_friendships_b ON friendships(user_b);

CREATE TABLE IF NOT EXISTS friend_requests (
    from_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    to_id      INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (from_id, to_id)
);
CREATE INDEX IF NOT EXISTS idx_friend_requests_to ON friend_requests(to_id);

CREATE TABLE IF NOT EXISTS message_mentions (
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (message_id, user_id)
);
CREATE INDEX IF NOT EXISTS idx_mentions_user ON message_mentions(user_id);

CREATE TABLE IF NOT EXISTS stories (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    author_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Both NULL for a reshare, which serves the current files of source_id.
    media_id    TEXT REFERENCES media(id),
    poster_id   TEXT REFERENCES media(id),
    source_id   INTEGER REFERENCES stories(id) ON DELETE CASCADE,
    is_video    INTEGER NOT NULL,
    state       TEXT NOT NULL,              -- 'processing' | 'live' | 'archived' | 'failed'
    compressed  INTEGER NOT NULL DEFAULT 0, -- archive copy made by the worker
    width       INTEGER NOT NULL DEFAULT 0,
    height      INTEGER NOT NULL DEFAULT 0,
    duration_ms INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL,
    expires_at  INTEGER NOT NULL            -- went live + 24 h; 0 while processing
);
CREATE INDEX IF NOT EXISTS idx_stories_author ON stories(author_id, id);
CREATE INDEX IF NOT EXISTS idx_stories_state ON stories(state, expires_at);
CREATE INDEX IF NOT EXISTS idx_stories_media ON stories(media_id);
CREATE INDEX IF NOT EXISTS idx_stories_poster ON stories(poster_id);

CREATE TABLE IF NOT EXISTS story_views (
    story_id INTEGER NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
    user_id  INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    seen_at  INTEGER NOT NULL,
    PRIMARY KEY (story_id, user_id)
);

CREATE TABLE IF NOT EXISTS story_jobs (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    story_id   INTEGER NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,                  -- 'prepare' | 'archive'
    state      TEXT NOT NULL DEFAULT 'queued', -- 'queued' | 'running' | 'failed'
    attempts   INTEGER NOT NULL DEFAULT 0,
    claimed_at INTEGER,
    error      TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_story_jobs_state ON story_jobs(state, id);
```

Indexes on `users.banner`/`users.avatar_still` are not needed (the reference checks below run
hourly or per deletion on small tables).

### 2.3 Data migration `friends_from_direct` (in `migrate`, once)

```sql
-- only if NOT EXISTS (SELECT 1 FROM meta WHERE key = 'friends_from_direct'); one transaction
INSERT OR IGNORE INTO friendships (user_a, user_b, created_at)
SELECT a.user_id, b.user_id, c.created_at
FROM conversations c
JOIN members a ON a.conversation_id = c.id
JOIN members b ON b.conversation_id = c.id AND b.user_id > a.user_id
WHERE c.is_group = 0
  AND NOT EXISTS (SELECT 1 FROM blocks
                  WHERE (user_id = a.user_id AND blocked_id = b.user_id)
                     OR (user_id = b.user_id AND blocked_id = a.user_id));
INSERT INTO meta (key, value) VALUES ('friends_from_direct', <now_ms>);
```
A new database runs it on an empty table and records the marker, so it never runs again.

### 2.4 Media references (cleanup and discard)

A media row is "in use" when referenced by any of: `messages.media_id`, `users.avatar`,
`users.avatar_still`, `users.banner`, `stories.media_id`, `stories.poster_id`. Both
`media::cleanup` (unattached uploads older than a day) and `media::discard` must check all six.
Files in `DATA_DIR/media` whose name is not a media id (e.g. `job-12-main.part`) are removed by
the existing stray-file sweep after an hour.

Account deletion: stories, views, jobs, friendships, requests and mentions cascade from
`users`; the own-media file removal already covers story files (owner = author).

---------------------------------------------------------------------------------------------

## 3. HTTP API

Unchanged routes keep their behaviour unless listed. "Visible" below means: not blocked
either way, and either the caller themselves, a friend, someone with a pending request either
way, or someone sharing a conversation with the caller.

### 3.1 Profiles

**`GET /api/me`** → `User` (now with `avatar_still`, `name_font`, `name_color`).

**`PUT /api/me`** body `Profile` → `User`. All new fields optional (none keeps the value).
- `display_name`: 1..=40 chars, required (as today) → 400 "Nom affiché : 1 à 40 caractères."
- `bio`: trimmed; ≤ 190 chars and ≤ 6 lines → 400 "Bio : 190 caractères et 6 lignes au plus."
- `avatar_id`: media owned by the caller, mime `image/jpeg|png|webp|gif`, ≤ 8 MB, width and
  height ≤ 4096 → else 400 "Choisissez une image JPEG, PNG, WebP ou GIF de 8 Mo au plus."
  The server checks the format by decoding (§5.2); an animated GIF/WebP gets a still (§5.2).
  Old avatar and old still are discarded.
- `remove_avatar`: clears `avatar` and `avatar_still` (both discarded).
- `banner_id`: media owned by the caller, mime `image/jpeg|png|webp`, ≤ 8 MB, ≤ 4096 px →
  else 400 "Choisissez une bannière JPEG, PNG ou WebP de 8 Mo au plus." Old banner discarded.
- `remove_banner`.
- `name_font`: `proto::valid_font` → else 400 "Police inconnue."
- `name_color`: `proto::valid_name_color` → else 400 "Couleur inconnue."
- Rate limit `profile` 20/h (as today) → 429.
- Events: `UserChanged { user }` to the caller's devices and to everyone the user is visible to
  (friends + conversation co-members, minus blocks); plus, as today, `Conversation { id }` for
  each of their conversations (0.3.0 apps rely on it).

**`GET /api/users/{id}`** → `UserProfile`. 404 "Utilisateur introuvable." unless visible.
`relation` from the caller's point of view; `online` only for friends and self (else false);
`direct_id` = the one-to-one conversation of the pair, if any (also when locked);
`friends_since` when friends; `joined_at` = `users.created_at`.

**`GET /api/users/{id}/media?before=<message_id>`** → `SharedMediaPage`. Messages of the
one-to-one conversation between the caller and `{id}`, not deleted, whose media mime starts
with `image/` or `video/`, newest first, 60 per page; `next` = the last item's `message_id` when
the page is full. Empty page when there is no such conversation. 404 when the user is not
visible. Works on locked conversations (the history stays readable).

**`GET /api/users?q=`** (changed) → `Vec<User>`: the caller's **friends** whose username or
display name contains `q` (case-insensitive), ordered by display name, max 100. Empty `q` lists
all friends.

### 3.2 Friends

**`GET /api/friends`** → `Vec<Friend>`, ordered by display name (NOCASE), with `online` and
`direct_id`.

**`DELETE /api/friends/{user_id}`** → 204 (idempotent; 404 if the user does not exist).
Events: `Friends { notice: None }` to both users; `Conversation { id }` of their direct
conversation (now locked) to both.

**`GET /api/friends/requests`** → `FriendRequests` (`incoming`: others → caller, `outgoing`:
caller → others, newest first; blocked pairs never appear).

**`POST /api/friends/requests`** body `NewFriendRequest` → `FriendRequestOutcome`.
- Username normalised: trim, strip one leading `@`, lowercase. Not 3..=24 chars of
  `[a-z0-9_.]` → 400 "Identifiant invalide."
- Own username → 400 "C'est votre propre identifiant."
- Missing, or blocked either way → 404 "Aucun membre avec cet identifiant."
- Already friends → 409 "Vous êtes déjà amis."
- The target already asked the caller → friendship created, both requests deleted →
  `status: "friends"`. Events: `Friends { notice: Accepted(caller) }` to the target,
  `Friends { notice: None }` to the caller; `Conversation { id }` of their direct conversation
  (unlocked) to both when it exists.
- Existing outgoing request → 200 `status: "sent"` (idempotent, no event).
- More than 50 outgoing pending → 400 "Trop de demandes en attente."
- Rate limit `friend-request` 30/h per user → 429 "Trop de demandes, patientez."
- Otherwise insert → `status: "sent"`. Events: `Friends { notice: Request(caller) }` to the
  target, `Friends { notice: None }` to the caller.

**`POST /api/friends/requests/{user_id}/accept`** → `Friend`. 404 "Demande introuvable."
without an incoming request from `user_id`. Creates the friendship, deletes requests both
ways. Events: `Friends { notice: Accepted(caller) }` to `user_id`, `Friends { notice: None }`
to the caller, `Conversation { id }` of their direct conversation to both when it exists.

**`POST /api/friends/requests/{user_id}/decline`** → 204 (idempotent). Events:
`Friends { notice: None }` to both.

**`DELETE /api/friends/requests/{user_id}`** (cancel an outgoing request) → 204 (idempotent).
Events: `Friends { notice: None }` to both.

**`PUT /api/blocks/{id}`** (changed): also deletes the friendship and requests both ways; when
something was deleted, `Friends { notice: None }` to both and `Conversation { id }` of the
direct conversation to both.

### 3.3 Conversations and messages

**`GET /api/conversations`**: each `Conversation` has `locked` = one-to-one and (not friends
or blocked either way). Groups are never locked.

**`POST /api/conversations`** (changed):
- One-to-one: the pair must be friends → 403 "Vous devez être amis pour discuter." If friends
  and the conversation exists, its id (as today).
- Group: every other member must be a friend of the caller → 403 "Vous ne pouvez ajouter que
  vos amis." Other checks unchanged.

**Writing in a locked conversation** (`POST .../messages`, `PUT /api/messages/{id}`,
`PUT /api/messages/{id}/reaction`) → 403 "Vous n'êtes plus amis. Envoyez une demande d'ami
pour reprendre la discussion." (replaces the block-only check `require_open`, which keeps its
block message for blocked pairs).

**`POST /api/conversations/{id}/messages`** (changed): after insert, parse mentions (§3.3.1),
store them in `message_mentions`; `Message.mentions` filled everywhere messages are returned.

**`PUT /api/messages/{id}`** body `EditMessage` → `Message`.
- 404 if missing, deleted or not visible; 403 "Vous ne pouvez modifier que vos messages." if
  not the sender; 403 locked as above.
- Body trimmed, ≤ 4000 chars → 400 "Message trop long."; empty allowed only when the message
  has media → else 400 "Message vide."; a message with a track cannot be edited → 400
  "Ce message ne peut pas être modifié."
- Same text → returns the message unchanged (no event).
- Sets `body`, `edited_at = now`, recomputes mentions (no notification).
- Rate limit `edit` 30/min → 429 "Doucement ! Patientez un instant."
- Event: `Update { message }` to the members.

**`DELETE /api/messages/{id}`**: unchanged, also deletes its mentions.

Every query building `Message` returns `edited_at` and `mentions`, and the sender `User` with
`avatar_still`, `name_font`, `name_color` (same for every `User` the server returns:
conversation members, friends, blocks, sessions).

#### 3.3.1 Mention parsing (server)

Scan the body for `@` at the start or after a character that is not `[A-Za-z0-9_.@]`, followed
by 3 to 24 chars of `[A-Za-z0-9_.]`; strip trailing dots; lowercase. Keep the ones equal to the
username of a member of the conversation other than the sender; dedupe; at most 20, in order of
appearance. No regex crate: a small hand-written scanner with unit tests.

### 3.4 Presence and activity

- Presence is WebSocket only (§4). `Friend.online` and `UserProfile.online` give the current
  state.
- **`GET /api/activity`** and `ServerEvent::Activity`: audience becomes friends ∪ conversation
  co-members, minus blocks (`music::audience`).

### 3.5 Stories

Limits (`proto::limits`): photo ≤ 15 MB, video ≤ 60 MB and ≤ 30 s, 30 live-or-processing
stories per author, rate limit `story` 30 per 24 h.

**`POST /api/stories`** body `NewStory` → `Story`.
- The media must be owned by the caller and not used anywhere else (§2.4) → 400 "Fichier
  introuvable." / "Fichier déjà utilisé."
- Mime `image/jpeg|png|webp` → photo; `video/mp4|webm|quicktime` → video; else 400 "Une story
  est une photo ou une vidéo."
- Too big → 413 "Story trop volumineuse."; too many live → 400 "Trop de stories en ligne.";
  rate → 429 "Trop de stories aujourd'hui."
- Photo: dimensions read from the header (`image` crate, §5.2; not decodable → 400 "Image
  illisible."); `state = live`, `expires_at = now + 24 h`, `duration_ms = 5000`. Events:
  `Story { story, user }` to the author's devices and friends.
- Video: no worker configured (`WORKER_TOKEN` empty) → 503 "Les stories vidéo ne sont pas
  disponibles sur ce serveur." Else `state = processing`, `expires_at = 0`, a `prepare` job is
  queued. Event: `Story` to the author's devices only.

**`GET /api/stories`** → `Vec<StoryReel>`: the caller's own reel first when it has processing,
live or failed stories; then each friend with at least one live story, unseen reels first, then
by latest story `created_at` descending. Stories within a reel oldest first. Friends' reels
contain live stories only. `seen` from `story_views` (own stories: always true).

**`POST /api/stories/{id}/seen`** → 204. The story must be live and its author a friend (404
otherwise). Idempotent (`INSERT OR IGNORE`). No event.

**`DELETE /api/stories/{id}`** → 204. Author only (404 otherwise, never revealing others').
Deletes the row (and reshares of it, by cascade); an original's files are discarded. Events:
`StoryGone { id, user_id }` to the author's devices and friends (also for each cascaded
reshare that was live).

**`GET /api/stories/archive?before=<id>`** → `StoryPage`: the caller's archived originals
(`state = archived`, `source_id IS NULL`), newest first, 60 per page, with archive keys.

**`POST /api/stories/{id}/reshare`** → `Story`. Author only, the story must be an archived
original (404 / 400 "Seule une story archivée peut être partagée à nouveau."). Same limits as
a new story. Creates a row with `source_id = id`, NULL files, `is_video`/size/duration copied,
`state = live`, `expires_at = now + 24 h`. Events: `Story` to the author's devices and friends.

**`GET /api/stories/{id}/media?k=`** and **`GET /api/stories/{id}/poster?k=`** (no bearer):
- `k` = hex HMAC-SHA256(server secret, `"story:{id}"`) (live key) or
  (`"story-archive:{id}"`) (archive key). Wrong key → 403 "Lien invalide."
- Live key: allowed only while the story is live and `expires_at > now`, else 403 "Story
  expirée." Archive key: allowed while the row exists and is archived.
- A reshare serves the current files of its source. No file (photo poster, processing) → 404.
- Range support (ServeFile), `X-Content-Type-Options: nosniff`, correct mime,
  `Cache-Control: private, max-age=86400`.
- `Story.key` in JSON: live key in reels and events (author included while live); archive key
  in `/api/stories/archive` and in the author's `Story` event for the archived state.

**Expiry** (server task every 30 s, `stories::expire_loop`, spawned in `main.rs`):
- Live reshares past `expires_at` → row deleted; `StoryGone` to author + friends.
- Live originals past `expires_at` → `state = archived`; queue an `archive` job when a worker is
  configured (else the original file stays as the archive, `compressed = 0`); `StoryGone` to
  friends; `Story` (archived, archive key) to the author's devices.
- Processing for more than 30 minutes → `failed` (job removed); `Story` (failed) to the author.
- Failed stories older than 24 h are deleted with their files by the hourly `cleanup`.

### 3.6 Internal worker API (not public)

Served by a **second listener** on `WORKER_BIND` (default `0.0.0.0:8081`), started only when
`WORKER_TOKEN` is not empty. Never published by compose; the stories worker reaches it as
`http://server:8081`. Every route requires `Authorization: Bearer <WORKER_TOKEN>` (constant-time
comparison) → 401 otherwise. Router: `api::internal_router(state)` so tests can call it.

| Route | Behaviour |
|---|---|
| `POST /internal/jobs/claim` | First claimable job (`queued`, or `running` with `claimed_at` older than 15 min), `prepare` before `archive`, then by id. Sets `running`, `claimed_at = now`, `attempts += 1` → 200 `worker::WorkerJob`, or 204. A job reaching a 4th attempt is failed instead (see `fail`, permanent). |
| `GET /internal/jobs/{id}/input` | The story's current main file (original upload for `prepare`, live file for `archive`), with its mime. 404 unless the job is running. |
| `PUT /internal/jobs/{id}/output/{slot}` | Raw body, `slot` = `main` (≤ 40 MB) or `poster` (≤ 2 MB). `Content-Type` must be `video/mp4` (video main), `image/jpeg` (photo main, poster) → 400 otherwise. Streamed to `DATA_DIR/media/job-{id}-{slot}.part`, replacing a previous upload. 204. |
| `POST /internal/jobs/{id}/done` | Body `worker::WorkerResult`. Requires `main` (and `poster` for videos). Moves each part to a new random media id, inserts the media rows (owner = author, name `story-{story}.mp4/.jpg`, real size; quota not checked), updates the story (below), discards the replaced files, deletes the job. 204. |
| `POST /internal/jobs/{id}/fail` | Body `worker::WorkerFailure`. `retry` and attempts < 3 → back to `queued`. Otherwise the job is `failed` (kept for the log until the story goes): a `prepare` failure makes the story `failed` (event to the author); an `archive` failure leaves the original as the uncompressed archive. Logged with `error`. 204. |

On `done`: `prepare` → `media_id`, `poster_id`, `width`, `height`, `duration_ms` set,
`state = live`, `expires_at = now + 24 h`, original upload discarded, `Story` event to the
author's devices and friends. `archive` → files replaced, `compressed = 1`, `Story` event to the
author's devices.

Encode profiles (server constants, sent in each job):

| Profile | max_side | fps | crf | max_kbps | audio | jpeg_q | poster_side | max_duration_ms |
|---|---|---|---|---|---|---|---|---|
| prepare video | 1280 | 30 | 26 | 2500 | 128 kbit/s stereo | 4 | 720 | 30000 |
| archive video | 854 | 24 | 32 | 800 | 64 kbit/s mono | 8 | 480 | 0 (no check) |
| archive photo | 1080 | – | – | – | – | 8 | – | 0 |

A 30 s live video is at most ~10 MB; its archive at most ~3.2 MB; an archived photo ~100 to
250 kB.

### 3.7 Configuration (compose `server` service, env)

- `WORKER_TOKEN` (`${WORKER_TOKEN:-}`): shared secret with the worker; empty disables video
  stories and archive compression (photo stories still work, archives stay uncompressed).
- `WORKER_BIND` (default `0.0.0.0:8081`).
- No other new variable. `deploy/.env` gains `WORKER_TOKEN=<long random string>`.

---------------------------------------------------------------------------------------------

## 4. WebSocket

### 4.1 Client → server (`ClientEvent`)

- `{"type":"active","active":true|false}` (new). The app sends it right after `ready` and on
  every change. Desktop shell: always `true` while the app runs. Phone/browser:
  `!document.hidden`. A socket is active from `active: true` until `active: false` or close.

### 4.2 Server → client (`ServerEvent`, new variants)

| Event | Sent to | App reaction |
|---|---|---|
| `friends { notice }` | see §3.2 | reload `/api/friends`, `/api/friends/requests`, `/api/stories`; toast for a notice ("X vous a envoyé une demande d'ami.", "X a accepté votre demande d'ami.") |
| `presence { user_id, online }` | friends (not blocked) | update the online set |
| `user_changed { user }` | §3.1 | replace that user in friends, conversations, open messages' senders, session (if self) |
| `story { story, user }` | §3.5 | insert or replace in `Ctx::stories` (remove from reels when no longer live/processing/failed; archive screen refreshes) |
| `story_gone { id, user_id }` | §3.5 | remove from reels |

Presence rules (server, in memory): user online ⇔ ≥ 1 active socket. Going online →
`presence(true)` at once to friends. Going offline → wait 15 s; still offline → `presence(false)`.
Friendship created → each side gets the other's state via the `friends` reload.

`message` / `update` carry `edited_at` and `mentions`. Call signals may use the new action
`screen` (§9); the server relays it like the others (one-to-one conversations whose members
are friends; locked conversations relay nothing).

### 4.3 Android background service

`app/android/ChatService.kt` (package C) also handles:
- `message` whose `mentions` contains the user id: notification on a new channel `mentions`
  (importance high), title "{sender} vous a mentionné", text = body; same suppression while
  `MainActivity.visible`.
- `friends` with `notice.kind == "request"`: notification on channel `messages`, title
  "Demande d'ami", text "{display_name} veut devenir votre ami.".
Unknown events are already ignored by 0.3.0 services.

---------------------------------------------------------------------------------------------

## 5. Media rules

### 5.1 Authorisation

| Media | Served by | Who learns the URL |
|---|---|---|
| Message files, avatars, avatar stills, banners | `GET /api/media/{id}` (id = 24 random bytes, capability URL, as today) | conversation members; anyone the user is visible to (avatar, still, banner) |
| Story files and posters | `GET /api/stories/{id}/media|poster?k=` (HMAC key, live key expires with the story) | friends while live (live key), the author forever (archive key) |

Story media ids never leave the server (the uploader knows only the original upload id, which is
discarded when a video is prepared; for a photo the author can of course re-fetch their own
upload through `/api/media/{id}` until it is archived and replaced).

### 5.2 Formats, limits, processing

| Kind | Accepted | Max | Server processing |
|---|---|---|---|
| Avatar | JPEG, PNG, WebP, GIF (animated GIF/WebP allowed) | 8 MB, 4096×4096 | Decoded with the `image` crate to validate. Animated (≥ 2 frames) → still of the first frame: centre square crop, resized to 256×256 (`FilterType::Triangle`), PNG, stored as a media row (owner = user, name `avatar-still.png`) in `users.avatar_still`. |
| Banner | JPEG, PNG, WebP (still) | 8 MB, 4096×4096 | Header decode only (dimensions). Displayed 3:1, `object-fit: cover`. |
| Story photo | JPEG, PNG, WebP | 15 MB | Dimensions from the header. The app always uploads a JPEG it re-encoded (§8.6), long side ≤ 1920, EXIF dropped. |
| Story video | MP4, WebM, QuickTime | 60 MB, 30 s | Worker `prepare` (H.264/AAC MP4 + JPEG poster). |
| Message media | unchanged | `MAX_UPLOAD_MB` | none ("kept forever" as today) |

Server crate: `image = { version = "0.25", default-features = false, features = ["gif",
"webp", "png", "jpeg"] }`. Decoding runs in `spawn_blocking` behind a 1-permit semaphore
(`AppState::imaging`), with `image::Limits` (`max_image_width/height = 4096`,
`max_alloc = 64 MB`). Only the first two frames of an animation are decoded (enough to know it
is animated and to make the still). The worker is not involved in avatars.

Retention: message media forever (until the message, conversation or account is deleted);
live story files until archived (then replaced by the compressed copy); archives forever until
the author deletes them or the account; all count against the user quota (`USER_QUOTA_MB`).

---------------------------------------------------------------------------------------------

## 6. Stories worker

### 6.1 Shape

- A second binary of the server crate: `server/src/bin/story-worker.rs` (compiled by
  `cargo test`, so `build/check.sh` covers it). Uses `tokio`, `reqwest`, `serde_json`, `chrono`,
  `proto` (already dependencies). No database, no data volume: everything goes through §3.6.
- Loop (one job at a time): `POST /internal/jobs/claim?archive={true|false}` (`archive=true`
  only when local time is inside `ARCHIVE_HOURS`, so expired-archive compression runs at night
  while live-story `prepare` jobs run at once). The server claims atomically, FIFO (`id ASC`),
  `prepare` always before `archive`. 204 → sleep 5 s; job → download input to `/tmp/job-{id}/in`,
  run ffprobe/ffmpeg (`tokio::process::Command`, ffmpeg under `nice -n 19` when available, kill
  after 5 min), upload outputs, `done`; on any error `fail` with `retry = true` for
  I/O/HTTP/timeouts, `retry = false` for refused inputs (too long, no video stream, undecodable).
  HTTP failure talking to the server → sleep 10 s. Always delete `/tmp/job-{id}`.
- Env: `SERVER_URL` (default `http://server:8081`), `WORKER_TOKEN` (required: exit with an
  error when empty), `FFMPEG_THREADS` (default `1`), `ARCHIVE_HOURS` (default `2-6`, local time
  from `/etc/localtime`; empty = any time).

### 6.2 Commands (profile values from the job; `S` = max_side, `P` = poster_side)

Scale filter keeping the aspect ratio, never upscaling, even sizes:
`scale='if(gte(iw,ih),trunc(min(S,iw)/2)*2,-2)':'if(gte(iw,ih),-2,trunc(min(S,ih)/2)*2)'`

- Probe (video): `ffprobe -v error -print_format json -show_format -show_streams in` → no video
  stream → refuse; `format.duration` > (max_duration_ms + 500)/1000 → refuse.
- Video remux fast-path (`prepare` only): when the upload is already H.264 (`yuv420p`/`yuvj420p`)
  + AAC (or no audio), max side ≤ 1280 and short side ≤ 720 (landscape or portrait, rotation
  applied), fps ≤ 30, size ≤ 40 MB and bitrate within budget, skip re-encoding and remux:
  `ffmpeg -nostdin -y -i in -t 30 -map 0:v:0 -map 0:a:0? -c copy -movflags +faststart
  -map_metadata -1 out.mp4`.
- Video main (otherwise): `ffmpeg -nostdin -y -i in -t 30 -map 0:v:0 -map 0:a:0? -vf "<scale>,fps=F"
  -c:v libx264 -preset veryfast -crf C -maxrate Mk -bufsize 2Mk -pix_fmt yuv420p
  -c:a aac -b:a Ak -ac N -movflags +faststart -map_metadata -1 -threads T out.mp4`
  (rotation is applied by ffmpeg's default autorotate; run under `nice -n 19` when available).
- Poster: `ffmpeg -nostdin -y -i out.mp4 -frames:v 1 -vf "<scale with P>" -q:v Q -threads T poster.jpg`.
- Photo archive: `ffmpeg -nostdin -y -i in -frames:v 1 -vf "<scale>" -q:v Q -map_metadata -1
  -threads T out.jpg`.
- `WorkerResult` width/height/duration from `ffprobe` of the output.

### 6.3 Dockerfile `server/Dockerfile.worker` (build context = project root)

```dockerfile
FROM rust:1-bookworm AS build
WORKDIR /src
COPY proto proto
COPY server server
WORKDIR /src/server
RUN cargo build --release --bin story-worker

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ffmpeg \
    && rm -rf /var/lib/apt/lists/* && useradd --system --uid 10002 worker
COPY --from=build /src/server/target/release/story-worker /usr/local/bin/story-worker
USER worker
CMD ["story-worker"]
```

### 6.4 Compose service (`deploy/docker-compose.yml`)

```yaml
  stories:
    build:
      context: ..
      dockerfile: server/Dockerfile.worker
      network: host
    image: goofychat-stories
    restart: unless-stopped
    depends_on: [server]
    environment:
      SERVER_URL: http://server:8081
      WORKER_TOKEN: ${WORKER_TOKEN:-}
      FFMPEG_THREADS: "1"
      ARCHIVE_HOURS: ${ARCHIVE_HOURS-2-6}
    networks: [default]
    volumes:
      - /etc/localtime:/etc/localtime:ro
    read_only: true
    tmpfs:
      - /tmp:size=256m,mode=1777
    security_opt:
      - no-new-privileges:true
    cap_drop: [ALL]
    mem_limit: 512m
    cpus: 1.0
    cpu_shares: 128
```
and in `server.environment`: `WORKER_TOKEN: ${WORKER_TOKEN:-}`, `WORKER_BIND: 0.0.0.0:8081`.
Port 8081 is never in `ports:`.

### 6.5 Data flow

```
app ──POST /api/media──▶ server (original in media/)
app ──POST /api/stories──▶ server: photo → live │ video → processing + job(prepare)
worker ──claim──▶ server ──job──▶ worker ──GET input──▶ ffmpeg ──PUT main/poster──▶ done
server: story live, original discarded ──story event──▶ author + friends
expire_loop (24 h later): archived + job(archive) ─▶ worker ─▶ done: compressed copy replaces it
```

---------------------------------------------------------------------------------------------

## 7. Navigation and app structure

### 7.1 Core types (in `app/src/main.rs`, written by package S)

```rust
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab { Home, Messages, Activity, Me }

#[derive(Clone, PartialEq, Debug)]
pub enum Screen {
    /// Root. Narrow: the current tab (`Ctx::tab`). Wide: the main pane's home.
    Home,
    New,
    Settings,
    Chat(i64),
    /// A user's profile, full screen (own id = own profile as others see it).
    Profile(i64),
    EditProfile,
    Friends,
    StoryArchive,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Overlay {
    /// Story viewer, starting at this author's reel (first unseen story, else the first).
    Stories { user_id: i64 },
    /// Capture or pick, preview, publish.
    StoryComposer,
    /// Snapchat-style camera of a conversation.
    Camera { conversation_id: i64 },
}
```

`Ctx` gains (all `Signal`s unless noted): `tab: Tab`, `back_stack: Vec<Screen>`,
`overlay: Option<Overlay>`, `friends: Vec<proto::Friend>`, `requests: proto::FriendRequests`,
`online: HashSet<i64>`, `stories: Vec<proto::StoryReel>`, `side_open: bool` (desktop right
pane shown; persisted with `music::pref("side.open")`). `people_tab` is removed by package B
(S leaves it in place).

Navigation API (S implements, everyone uses):
- `ctx.open(screen)`: same screen → nothing. Narrow: pushes the current screen on
  `back_stack` and a browser history entry (`js::push_screen`). Wide: pushes a history entry
  only when leaving `Home`; otherwise replaces (back on desktop always returns Home).
  Clears messages/typing and loads a chat as today.
- `ctx.back()`: `js::pop_screen()` (the `popstate` handler does the rest).
- `popstate` handler: overlay open → close it; else narrow: pop `back_stack` (empty → `Home`);
  wide: `Home`. Landing on `Screen::Chat(id)` reloads its messages (`ctx.load(id)`).
- `ctx.go_tab(tab)`: narrow only: sets `tab`, `screen = Home`; tab bar is only shown on `Home`.
- `ctx.open_overlay(o)`: pushes a history entry and sets `overlay`; `ctx.close_overlay()`:
  `js::pop_screen()` when an overlay is open. Escape closes the overlay first (App-level
  listener), then the existing rules.
- Helpers: `ctx.is_friend(id)`, `ctx.friend(id) -> Option<Friend>` (peek), `ctx.is_online(id)`
  (reads, subscribes).

Event dispatch in `Ctx::handle` (S wires the calls; the functions live in the owners' files):

| Event | Call |
|---|---|
| `Ready` | existing + `friends::load(self).await`, `stories::load(self).await` |
| `Message` | existing; desktop notification via `chat::announce(self, &message)` (moved from main.rs) |
| `Friends { notice }` | `friends::on_friends(self, notice).await; stories::load(self).await` |
| `Presence { user_id, online }` | `friends::on_presence(self, user_id, online)` |
| `UserChanged { user }` | `friends::on_user_changed(self, user)` |
| `Story { story, user }` | `stories::on_story(self, story, user)` |
| `StoryGone { id, user_id }` | `stories::on_story_gone(self, id, user_id)` |

### 7.2 Mobile (narrow) layout

- `Screen::Home` renders `nav::MobileHome`: the tab content plus `nav::TabBar` at the bottom.
  - **Accueil** → `home::HomeTab`: header "Accueil" (own avatar left opens Profil tab), the
    stories row (`stories::StoriesRow`: own bubble "Ta story" with a "+" first, then friends'
    reels, unseen ring = accent gradient, seen ring = `--line`), then the posts area
    placeholder: a card "Publications" — "Bientôt ici : les photos et les nouvelles de vos amis."
    with a muted illustration (no API).
  - **Messages** → `screens::Conversations` (the former conversation list): header
    "Messages" with an "Amis" icon button (`userplus`, badge = incoming requests) →
    `Screen::Friends`; rows show the "Vu" status (`chat::row_status`); FAB "+" → `Screen::New`
    (friends only; group name when ≥ 2 chosen).
  - **Activité** → `activity::ActivityPage` (§7.4).
  - **Profil** → `profile::MyProfile`: own banner/avatar (animated)/name/bio, buttons
    "Modifier le profil" (`EditProfile`), "Amis" (badge), "Archives des stories"
    (`StoryArchive`), "Réglages" (`Settings`).
- Tab bar: Accueil (`home`), Messages (`chat`, badge = total unread), Activité (`activity`),
  Profil (own small avatar, badge = incoming requests). Labels in French as written.
- Chat header: tapping the avatar/name of a one-to-one conversation opens
  `Screen::Profile(peer)`; in a group it opens the existing members sheet.
- Overlays cover everything, including the tab bar.

### 7.3 Desktop (wide) layout, three panes

- **Left** `nav::SideNav` (width `--side`): top buttons "Accueil" (main pane `Home`) and
  "Amis" (`Friends`, badge); section title "Messages" with "+" (`New`); the conversation list;
  bottom **user panel** (Discord-like): own still avatar + name (font/colour) + `@username`;
  clicking it opens `Profile(me)`; a gear button opens `Settings`.
- **Main**: `Home` → `home::DesktopHome` (stories row + posts placeholder + the former
  Welcome shortcuts); other screens as on mobile.
- **Right** `activity::SidePane` (≈ 300 px, toggled by `Ctx::side_open`, a header button in the
  chat "Profil"/"Membres" toggles it): one-to-one chat → `profile::ProfilePane { user_id }`
  (banner, animated avatar, name, bio, "Membre depuis", "Amis depuis", activity line, buttons
  Profil complet / Photos), group → members list (online dots), otherwise →
  `activity::ActivityPane` (§7.4). Default: open.
- The call UI (§9) covers the main pane only.

### 7.4 Activity

`activity::ActivityPage` (mobile tab) and `activity::ActivityPane` (desktop right pane) show,
for friends only: "En ligne — n" (avatars with green dots; tap → profile), "En écoute" (the
existing listening cards), "Stories récentes" (`stories::RecentStories`: friends' reels with
live stories, newest first, tap → viewer). Empty state: "C'est calme ici — quand vos amis sont
en ligne, écoutent de la musique ou publient une story, ça s'affiche ici."

### 7.5 Profiles (package B)

- `profile::ProfileScreen { user_id }`: banner (3:1, fallback accent-tinted), large animated
  avatar overlapping it, name (font + colour), `@username`, presence dot, bio, "Membre depuis",
  activity line; actions by `relation`: Friend → "Message" (opens `direct_id` or creates it),
  "Appeler", menu "Retirer des amis" / "Bloquer"; Incoming → "Accepter" / "Refuser";
  Outgoing → "Demande envoyée" + "Annuler"; None → "Ajouter en ami"; Me → "Modifier le
  profil". Below: "Photos" grid (§3.1 media endpoint, infinite scroll, 3 columns, squares) with
  a full-screen viewer (swipe/arrow keys left-right, Escape/back closes, videos play inline).
- `profile::EditProfile`: avatar (upload; GIF/animated WebP accepted, ≤ 8 MB pre-checked),
  banner, display name, bio with counter (190), name font picker (10 fonts, each previewed
  with the user's name), colour palette (12 + "Par défaut"), live preview card; "Enregistrer".
- `settings::Settings` keeps: Personnes bloquées, Apparence (theme), Sons, SelfHost-hub,
  Application (desktop), Compte. The profile block moves to `EditProfile`.
- `ui::Avatar` fix (oval/glitch): `aspect-ratio: 1`, fixed width = height, `flex: none`,
  `overflow: hidden`, `img { width: 100%; height: 100%; object-fit: cover; display: block }`.
  Props (backward compatible): `name`, `photo` (full, maybe animated), `still` (first frame),
  `small`, `live`, `online` (green dot), `animate` (always show `photo`), `size` (px, optional).
  Without `animate`, it shows `still` when present, and on desktop swaps to `photo` on hover.
- `ui::UserAvatar { user, small, live, online, animate }` and `ui::Name { user }` (renders
  `span.name` with class `font-<id>` and inline `color` when set). Every place that shows a
  person uses these.

### 7.6 Messages (package C)

- Composer: on Android/narrow a `textarea` (1 to 5 rows, auto-grow) where Enter inserts a new
  line and a send button (always visible when there is text, mic otherwise); on desktop/wide
  Enter sends, Shift+Enter inserts a new line. A camera button (`camera`) opens
  `Overlay::Camera`. Typing `@` in a group offers members (filter by username/display name).
- Conversation list "Vu": `chat::row_status(c, me) -> Option<String>`: last message is mine and
  not deleted → one-to-one: "Vu" when the other's read mark ≥ it, else "Envoyé"; group: "Vu par
  n" when n ≥ 1. Shown instead of the time on the right of the row, muted; not shown when
  `locked` (row shows a lock icon instead).
- Typing indicator: an incoming-style bubble with three dots (`.typing-dots`, §8.4); several
  people → names above it in small text.
- Double tap (or double click) a message: toggles ❤️ (`proto::LIKE`) with the heart pop.
- Long press (450 ms, < 10 px movement; right click on desktop): vibration 12 ms, the bubble
  lifts, menu at the bubble: reactions row (6), "Répondre", "Mentionner" (others' messages:
  reply + `@username ` prefilled), "Copier", "Modifier" (own text messages), "Supprimer pour
  tout le monde" (own, with confirmation).
- Editing: the composer switches to edit mode ("Modification du message" bar with cancel),
  `PUT /api/messages/{id}`; edited messages show "modifié" next to the time.
- Mentions: `@username` tokens of members are highlighted (`.mention`; mine:
  `.mention.me`); a message mentioning me gets a subtle accent outline; desktop notification
  even when the window is focused unless that conversation is open and active.
- Locked conversation: the composer is replaced by "Vous n'êtes plus amis." + "Ajouter en
  ami" (sends a request by username) or "Demande envoyée"; call buttons hidden.
- Chat header: name via `ui::Name`, tap → profile (§7.2), right-pane toggle on desktop.
- Camera overlay (`camera::Camera`): full-screen live preview (`facingMode: environment`
  first on phones, `user` on desktop), shutter, flip, close; after the shot a preview with
  "Envoyer" (uploads via `GoofyBridge.uploadBlob`, sends a media message, closes) and
  "Reprendre". JPEG, long side ≤ 1920, quality 0.85. Desktop shows the button only when a
  camera exists.

### 7.7 Stories UI (package D)

- `stories::StoriesRow` (home, desktop home), `stories::RecentStories` (activity).
- `stories::StoryViewer { user_id }` overlay: segmented progress bars on top (photo 5 s, video
  its duration; linear), author avatar + name + relative time ("il y a 3 h"), close button;
  tap right half → next, left → previous, press and hold → pause, swipe down (or Escape/back)
  → close; at the end of a reel → next reel in `Ctx::stories` order, after the last → close.
  Marks each displayed friend story seen (`POST .../seen`, then local `seen = true`). Own
  stories: "Supprimer" in a "…" menu; processing → "Publication…" placeholder; failed → "La
  vidéo n'a pas pu être publiée." + "Supprimer". Video: sound on, `playsinline`, poster.
- `stories::StoryComposer` overlay: live camera preview; tap shutter = photo, hold = video
  (ring fills over 30 s, stops at 30 s), flip, gallery (file input `image/*,video/*`: photos
  re-encoded; videos ≤ 60 MB and ≤ 30 s checked via metadata, else toast "Vidéo trop longue
  (30 s max)." / "Vidéo trop lourde (60 Mo max)."); preview step with "Publier" (upload
  progress bar, then `POST /api/stories`) and "Reprendre".
- `stories::StoryArchive` screen: grid (3 columns, by month headers "novembre 2026"), tap →
  viewer in archive mode (no progress auto-advance across reels) with "Partager à nouveau"
  (`reshare`) and "Supprimer". Empty: "Vos stories expirées arrivent ici. Elles ne sont
  visibles que par vous."
- `stories::load(ctx)`, `on_story`, `on_story_gone` keep `Ctx::stories` in sync. URLs:
  `{SERVER}/api/stories/{id}/media?k={key}`, `/poster?k={key}` (helper in `stories.rs`).

### 7.8 New and changed files

| File | Content | Package |
|---|---|---|
| `app/src/nav.rs` | `MobileHome`, `TabBar`, `SideNav`, user panel | S (stub) → B |
| `app/src/profile.rs` | `ProfileScreen`, `ProfilePane`, `EditProfile`, `MyProfile`, photo grid + viewer | S → B |
| `app/src/friends.rs` | `FriendsScreen`, add by username, requests; `load`, `on_friends`, `on_presence`, `on_user_changed` | S → B |
| `app/src/home.rs` | `HomeTab`, `DesktopHome` (former `Welcome`), posts placeholder | S → D |
| `app/src/stories.rs` | state + `StoriesRow`, `RecentStories`, `StoryViewer`, `StoryComposer`, `StoryArchive` | S → D |
| `app/src/camera.rs` | `Camera` overlay | S → C |
| `app/src/call.rs` | `CallView`, `CallOverlay` (moved from main.rs/chat.rs) | S → E |
| `app/src/screens.rs` | `Auth`, `Conversations` (list), `NewChat` (friends only); `People` removed | B (F2: `Auth` look) |
| `app/src/chat.rs` (+ optional `bubble.rs`, `composer.rs` split by C) | conversation | C |
| `app/js/gestures.js` | long press, double tap, vibrate | C |
| `app/js/camera.js` | in-chat camera | C |
| `app/js/stories.js` | story capture, pick, normalise, upload | D |
| `app/js/bridge.js` | + `window.GoofyBridge`, presence; calls section: screen share, speaking | S; E (calls) |
| `app/profile.css`, `app/nav.css` | profiles, friends, activity, nav | B |
| `app/chat.css` | messages features | C |
| `app/stories.css` | home + stories | D |
| `app/call.css` | call UI | E |
| `app/look.css` | customization selectors (`[data-bubble]`, `[data-font]`, `[data-wall]`, `[data-compact]`, moved from style.css) | F |
| `app/motion.css` | motion tokens (§8.4) | S (tokens) → F |
| `app/fonts.css`, `app/fonts/*.woff2`, `app/fonts/LICENSES.txt` | bundled fonts | S (empty) → F |

`app/index.html` (S): CSS links in this order: `motion.css`, `fonts.css`, `style.css`,
`music.css`, `look.css`, `nav.css`, `profile.css`, `chat.css`, `stories.css`, `call.css`; plus
`<link data-trunk rel="copy-dir" href="fonts">`.

JS modules do not import each other; they use globals (as `look.js` does with
`window.GoofyLook`). `bridge.js` exposes:

```js
window.GoofyBridge = {
  // Uploads to /api/media with the session token; resolves to the media id, rejects with a
  // French message. onProgress(fraction 0..1) is optional (XMLHttpRequest upload progress).
  uploadBlob(blob, name, onProgress),
  // { server, token, userId } of the current session.
  session(),
  // Short vibration where supported (Android native bridge first, then navigator.vibrate).
  vibrate(ms),
};
```
Each package declares the Rust bindings of its own JS module in its own Rust file with
`#[wasm_bindgen(module = "/js/<file>.js")]` (not in `js.rs`, which only S and E edit).

### 7.9 Bridge functions added (JS → exported names; Rust bindings in the owner's file)

| Module | Function | Contract |
|---|---|---|
| bridge.js (S) | `GoofyBridge.uploadBlob/session/vibrate` | above |
| bridge.js (S) | presence | sends `{"type":"active","active":...}` after `ready` and on `visibilitychange` (desktop: always true) |
| gestures.js (C) | `attachGestures(containerId, callback) -> handle` | delegated pointer handling on elements with `data-msg="<id>"`: reports `"long:<id>:<x>:<y>"` (after vibrating 12 ms) and `"double:<id>"`; `contextmenu` on desktop = long press. Handle for `js::off_listener`. |
| gestures.js (C) | `vibrate(ms)` | calls `GoofyBridge.vibrate` |
| gestures.js (C) | `autosize(textareaId)` | grows the composer up to 5 lines |
| camera.js (C) | `cameraOpen(videoId, facing) -> Promise<bool>`, `cameraFlip() -> Promise`, `cameraShoot() -> Promise<string>` (object URL of the JPEG preview), `cameraSend(onProgress) -> Promise<string>` (media id), `cameraRetake()`, `cameraClose()`, `cameraAvailable() -> Promise<bool>` | errors are French strings ("Accès à la caméra refusé.") |
| stories.js (D) | `storyCameraOpen(videoId, facing)`, `storyFlip()`, `storyShoot() -> url`, `storyRecordStart(maxMs, onTick)`, `storyRecordStop() -> url`, `storyPick(inputId) -> url`, `storyPublish(onProgress) -> media id`, `storyDiscard()`, `storyCameraClose()` | photo normalisation: `createImageBitmap` (EXIF orientation applied), canvas, JPEG 0.85, long side ≤ 1920; video recording: `MediaRecorder` mp4 if supported else webm, auto-stop at 30 s |
| bridge.js calls (E) | `startScreenShare() -> Promise`, `stopScreenShare()`, `canShareScreen() -> bool`, speaking detection, new `CallView` fields | §9 |

Camera code is duplicated between `camera.js` and `stories.js` on purpose (parallel work); F2
may merge it later only if trivial.

---------------------------------------------------------------------------------------------

## 8. Look and feel

### 8.1 Login page (F2)
`screens::Auth`: solid colours only: logo tile `background: var(--accent)`, no box-shadow
glow; primary button solid `var(--accent)`; page `var(--bg)`; inputs `var(--surface)`. No
`linear-gradient`/`radial-gradient` anywhere on the page.

### 8.2 Customization model (F1: `theme.rs`, `look.js`, `look.css`)
The per-device look (localStorage `goofychat.theme`) gains fields (JSON names):

| Field | Values | Applied as |
|---|---|---|
| `bubble` | `round` (default), `square`, `flat`, `pill`, `tail` | `html[data-bubble]` |
| `mineColor` | `""` (theme gradient) or `#rrggbb` (solid) | `--bubble-mine`, `--mine-text` (luminance > 0.5 → `#0b0b12` else `#fff`) |
| `inColor` | `""` (theme) or `#rrggbb` | `--in`, `--in-text` (same rule) |
| `font` | `""` or a `FONTS` id with `body: true` | `html[data-font]`, `--msg-font` |
| `motion` | `auto` (follow the system), `reduced` | `html[data-motion="reduced"]` |

Existing fields unchanged (`preset`, `mode`, `accent`, `size`, `wall`, `compact`). The
Apparence screen adds: bubble shape segments (5 with a live preview), "Couleur de mes bulles"
(theme + 8 swatches + custom picker), "Couleur des bulles reçues" (theme + 8 + custom), "Police
des messages" (system + 7 previews), "Réduire les animations" switch. The inline script of
`index.html` keeps applying the cached look before first paint (fields included in the cache).

### 8.3 Fonts (F1)
Files `app/fonts/<id>-<weight>.woff2`, latin subset (covers French, including œ), downloaded
from Fontsource via jsDelivr (pin the exact version you download and record it):
`https://cdn.jsdelivr.net/npm/@fontsource/<pkg>@5/files/<pkg>-latin-<weight>-normal.woff2`,
licence text from `https://cdn.jsdelivr.net/npm/@fontsource/<pkg>@5/LICENSE`. Upstream source:
`https://github.com/google/fonts/tree/main/ofl/<dir>`. All are SIL Open Font License 1.1.

| id | Family | Fontsource `<pkg>` | Weights | Body |
|---|---|---|---|---|
| inter | Inter | `inter` | 400, 700 | yes |
| nunito | Nunito | `nunito` | 400, 700 | yes |
| atkinson | Atkinson Hyperlegible | `atkinson-hyperlegible` | 400, 700 | yes |
| lora | Lora | `lora` | 400, 700 | yes |
| space-mono | Space Mono | `space-mono` | 400, 700 | yes |
| comfortaa | Comfortaa | `comfortaa` | 400, 700 | yes |
| caveat | Caveat | `caveat` | 400, 700 | yes |
| pacifico | Pacifico | `pacifico` | 400 | names only |
| press-start | Press Start 2P | `press-start-2p` | 400 | names only |
| bebas | Bebas Neue | `bebas-neue` | 400 | names only |

`app/fonts.css`: one `@font-face` per file, family `"GC <Family>"`, `font-display: swap`;
classes `.font-<id> { font-family: "GC <Family>", system-ui, sans-serif; }`; optical
corrections `.font-press-start { font-size: .72em; }`, `.font-bebas { font-size: 1.15em;
letter-spacing: .02em; }`. `app/fonts/LICENSES.txt`: for each font, name, version, source URL
and the full OFL text with its copyright line (required by the OFL when redistributing).
Total ≈ 0.5 MB, fetched only when a font is used. No runtime font service.

### 8.4 Motion tokens (`app/motion.css`, written by S exactly like this; F2 may tune values)

```css
:root {
  --dur-instant: 90ms;   /* press feedback, toggles */
  --dur-fast: 160ms;     /* hover, fades, leaving menus */
  --dur-base: 240ms;     /* menus, popovers, long-press menu */
  --dur-slow: 340ms;     /* sheets, page changes */
  --dur-slower: 480ms;   /* story viewer, call screen */
  --dur-fade: 140ms;     /* pure opacity changes; kept when motion is reduced */
  --ease-out: cubic-bezier(.22, 1, .36, 1);       /* entering */
  --ease-in: cubic-bezier(.55, 0, 1, .45);        /* leaving */
  --ease-in-out: cubic-bezier(.65, 0, .35, 1);    /* moving across the screen */
  --ease-spring: cubic-bezier(.34, 1.56, .64, 1); /* overshoot: heart, reactions, pops */
  --move: 1;             /* multiply translate distances by this */
  --press-scale: .96;
}
@media (prefers-reduced-motion: reduce) {
  :root:not([data-motion="full"]) { --dur-instant: 1ms; --dur-fast: 1ms; --dur-base: 1ms;
    --dur-slow: 1ms; --dur-slower: 1ms; --move: 0; --press-scale: 1; }
}
:root[data-motion="reduced"] { --dur-instant: 1ms; --dur-fast: 1ms; --dur-base: 1ms;
  --dur-slow: 1ms; --dur-slower: 1ms; --move: 0; --press-scale: 1; }
```
Rules for everyone: every new transition/animation uses these tokens (never raw durations);
translations are written `calc(<distance> * var(--move))`; infinite decorative animations
(typing dots, ringing) switch to an opacity pulse when `--move` is 0 (use
`@media (prefers-reduced-motion: reduce)` + `[data-motion="reduced"]` selectors). Functional
progress (story bars, upload bars) keeps running.

Catalogue (owners implement in their CSS; F2 harmonises): screen push (mobile): enter from
`translateX(24px)` + fade, `--dur-slow --ease-out`; tab change: fade `--dur-fast`; bottom
sheets: `translateY(100%)` → 0 `--dur-slow --ease-out`, backdrop fade `--dur-base`;
long-press menu: scale .92 → 1 + fade from the press point (`transform-origin` from
`--x/--y`), `--dur-base --ease-spring`, pressed bubble scale `--press-scale`; double-tap heart:
scale 0 → 1.15 → 1 `--dur-slow --ease-spring`, then fade `--dur-fast`; typing dots: 3 dots
bouncing `translateY(calc(-3px * var(--move)))`, 1.2 s loop, 0.15 s stagger; toasts: drop
12 px + fade `--dur-base`; story viewer open: scale .96 → 1 + fade `--dur-slower`.

---------------------------------------------------------------------------------------------

## 9. Desktop call UI and screen sharing (package E)

The connection fix is a separate package (X, already planned). E builds on it.

### 9.1 UI (`app/src/call.rs`, `app/call.css`)
- Ringing (incoming/calling): unchanged design (full-screen portrait), restyled with tokens.
- Connected, wide layout ("like Discord"): covers the main pane (left and right panes stay
  usable). Dark stage (`#111214`-ish, theme-independent). **Tiles**: one per participant
  (me, peer), 16:9, rounded 12 px; camera video when on (`object-fit: cover`; own mirrored),
  else the user's animated avatar centred on a tile tinted with their hue. **Speaking
  indicator**: 3 px `--ok` ring + soft glow on the tile while that participant speaks.
  **Screen share**: the shared screen becomes the focus tile (`object-fit: contain`, black
  bars), participant tiles shrink into a strip below (or right when wider than 1400 px);
  double click → window full screen (`requestFullscreen` on the stage). Top-left: peer name
  + timer + "Appel vidéo"/"Appel vocal". **Bottom control bar** (centred pill, blurred
  background): Micro (`mic`/`micoff`, red when muted), Caméra (`video`/`videooff`), Partager
  l'écran (`screen`/`screenoff`, accent when sharing; hidden when `canShare` is false),
  Plein écran (`expand`/`shrink`), Raccrocher (red, `hangup`). Tooltips in French.
- Narrow/mobile: current layout kept; a received screen share replaces the remote video
  (contain) with a small "Écran partagé" label; speaking ring on the avatar.

### 9.2 Bridge (`app/js/bridge.js`, calls section) and `CallView`
- New `CallView` JSON fields (camelCase from JS, as today): `screenLocal`, `screenRemote`,
  `canShare` (`!!navigator.mediaDevices?.getDisplayMedia` and not Android), `speakingLocal`,
  `speakingRemote`, `remoteCamera` (remote camera track live and enabled), `since` (ms).
- Elements (always mounted, like today): `remote-video` (peer camera + audio),
  `local-video`, `remote-screen`, `local-screen`.
- `startScreenShare()`: `getDisplayMedia({ video: { frameRate: 30, width: { max: 1920 } },
  audio: false })`; `contentHint = "detail"`; send `screen {on: true, stream: <stream.id>}`;
  `pc.addTrack(track, stream)`; sender `maxBitrate` 1.5 Mbit/s; renegotiate. Track `ended`
  (browser "Arrêter le partage") → `stopScreenShare()`: `removeTrack`, send `screen {on:
  false}`, renegotiate. Errors → `"Partage d'écran impossible."` (cancel by the user: silent).
- Renegotiation: `negotiationneeded` after the initial connection → offer via the existing
  `offer`/`answer` actions; "perfect negotiation" with the callee as the polite peer
  (rollback on glare). The receiver tells screen from camera by the stream id announced in
  `screen`; before the announcement, an unknown second video stream is treated as screen.
- Speaking: `AnalyserNode` on local and remote audio, RMS threshold 0.04 (time domain),
  300 ms hold; publish only on change.
- Desktop shell `allow_media` (`app/src-tauri/src/desktop.rs`): keep granting camera and
  microphone; on Linux, display capture requests (`UserMediaPermissionRequest` for display)
  are allowed for the app's own page; on Windows WebView2 shows its own picker. If
  `getDisplayMedia` is missing or fails on WebKitGTK, the button is hidden (`canShare`) or the
  error toast shows; that is acceptable for 0.4.0.
- The server needs no change beyond `CallAction::Screen` (relayed as is).

---------------------------------------------------------------------------------------------

## 10. What breaks for 0.3.0 apps (against a 0.4.0 server)

Unavoidable, and only these:
1. **Starting a one-to-one conversation with a non-friend** fails (403 "Vous devez être amis
   pour discuter."). The 0.3.0 "Personnes" page only lists friends now, and a 0.3.0 app cannot
   send friend requests: a 0.3.0 user can only talk to people they were already talking to
   (auto-friended) or who befriended them from a 0.4.0 app (requests cannot be accepted from
   0.3.0 either).
2. **Creating a group** that includes a non-friend fails (403).
3. **Writing in a locked conversation** (friendship removed by the other side, or a block)
   fails (403 with the French message, shown as a toast).
4. **HEIC/HEIF profile photos** are refused (400); JPEG/PNG/WebP/GIF still work.
5. **Presence**: 0.3.0 apps never send `active`, so their users show as offline.

Not broken, just missing: stories, friends, profiles (banner, bio, fonts, colours) are not
shown; edited messages show the new text without "modifié"; mentions are plain text; a peer's
screen share appears in place of their camera; new events are ignored (0.3.0 apps drop events
they cannot parse). Desktop and Android 0.3.0 apps offer the 0.4.0 update themselves.

---------------------------------------------------------------------------------------------

## 11. Proto (already in `proto/src/lib.rs`)

Added: `LIKE`, `Font`, `FONTS`, `NAME_COLORS`, `valid_font`, `valid_name_color`, `limits::*`;
`User.avatar_still/name_font/name_color` (+ `Default`); `Profile.bio/banner_id/remove_banner/
name_font/name_color` (+ `Default`); `Relation`, `UserProfile`, `Friend`, `FriendRequest`,
`FriendRequests`, `NewFriendRequest`, `FriendRequestStatus`, `FriendRequestOutcome`,
`FriendNoticeKind`, `FriendNotice`, `SharedMedia`, `SharedMediaPage`; `Message.edited_at/
mentions`, `EditMessage`; `Conversation.locked`; `CallAction::Screen`, `ScreenShare`;
`StoryState`, `Story`, `StoryReel`, `NewStory`, `StoryPage`; `worker::{JobKind,
EncodeProfile, WorkerJob, WorkerResult, WorkerFailure}`; `ClientEvent::Active`;
`ServerEvent::{Friends, Presence, UserChanged, Story, StoryGone}`. All additions are additive
with `#[serde(default)]` where old JSON must still parse. Changing proto after this point
requires updating this section and telling the other packages.

---------------------------------------------------------------------------------------------

## 12. Work split

Order: **S** and **A** start at once (A never waits). **B, C, D, E, F1** start when S is done.
**E's `bridge.js` part** starts when the call-connection fix (package X, separate) has landed.
**F2** runs last, after A to E. Each package keeps every file it owns compiling at each
check (stubs first, then fill in). A check failure in a file owned by another package is not
yours: wait and re-run, do not edit it. Checks are serialized on the build machine and upload
the whole tree.

Verification for everyone: `powershell -ExecutionPolicy Bypass -File build\check.ps1 app`
(app packages) or `... server` (A), and finally `check.ps1` (all) → `=== CHECK OK`. Never
build APK/desktop packages, never deploy. Each report lists a manual test checklist for the
user (devices are not available to agents).

### S — Scaffold (light, first, ~1 h)
- Scope: the plumbing of §7.1 and §7.8: enums, `Ctx` fields and navigation API, event
  dispatch, three-pane/tab skeleton, overlays rendering at App level, stub modules with the
  exact public signatures below, CSS files, `index.html` links, `motion.css` tokens,
  `GoofyBridge` + presence in `bridge.js`, new icons. No feature work; behaviour otherwise
  unchanged.
- Creates: `app/src/{nav,profile,friends,home,stories,camera,call}.rs`,
  `app/{nav,profile,chat,stories,call,look,motion,fonts}.css`, `app/fonts/LICENSES.txt`
  (placeholder line).
- Edits: `app/src/main.rs` (all of §7.1; moves `Welcome` to `home.rs` as `DesktopHome`,
  `announce` to `chat.rs`, `CallView` to `call.rs`), `app/src/chat.rs` (removes
  `CallOverlay` → `call.rs`; adds `row_status` stub returning `None`, `announce`),
  `app/src/ui.rs` (adds `UserAvatar`, `Name`, new `Avatar` props `still/online/animate/size`
  with a simple implementation, icon paths: `home, activity, camera, flip, edit, at, heart,
  screen, screenoff, userplus, usercheck, archive, grid, reshare, expand, shrink, eye`),
  `app/src/activity.rs` (adds `ActivityPage`, `SidePane` stubs), `app/src/screens.rs`
  (rename `Home` → `Conversations` only), `app/js/bridge.js` (`GoofyBridge`, presence only),
  `app/index.html` (links only). `GoofyBridge.vibrate(ms)`:
  `window.GoofyNative?.vibrate ? GoofyNative.vibrate(ms) : navigator.vibrate?.(ms)` (the native
  method arrives with package C; older APKs fall back).
- Stub signatures (bodies minimal; owners replace):
  `nav::{MobileHome(), TabBar(), SideNav()}`; `profile::{ProfileScreen(user_id: i64),
  ProfilePane(user_id: i64), EditProfile(), MyProfile()}`; `friends::{FriendsScreen(),
  async load(Ctx), async on_friends(Ctx, Option<FriendNotice>), on_presence(Ctx, i64, bool),
  on_user_changed(Ctx, User)}`; `home::{HomeTab(), DesktopHome()}`;
  `stories::{StoriesRow(), RecentStories(), StoryViewer(user_id: i64), StoryComposer(),
  StoryArchive(), async load(Ctx), on_story(Ctx, Story, User), on_story_gone(Ctx, i64, i64)}`;
  `camera::Camera(conversation_id: i64)`; `call::{CallView, CallOverlay(call: CallView,
  wide: bool)}`; `chat::{row_status(&Conversation, i64) -> Option<String>, announce(Ctx,
  &Message)}`; `activity::{ActivityPage(), SidePane()}`; `screens::Conversations()` may stay
  the existing `Home` list renamed (S renames `screens::Home` → `screens::Conversations`).
- Depends on: proto (done). Verify: `check.ps1 app`.

### A — Server, worker, deploy (heavy)
- Scope: §2, §3, §4 server side, §5, §6, README server/deploy/features sections.
- Owns: `server/**` (schema, `state.rs` migrate/config/presence state, `api.rs`, `auth.rs`,
  `account.rs`, `messages.rs`, `media.rs`, `music.rs` (audience), `ws.rs` (active, presence,
  relay friends rule), new `friends.rs`, `profiles.rs`, `stories.rs`, `internal.rs` (worker
  API), `imaging.rs`, `mentions.rs`, `src/bin/story-worker.rs`, `tests.rs`, `Cargo.toml` +
  `Cargo.lock`, `main.rs`, `Dockerfile`, new `Dockerfile.worker`), `deploy/docker-compose.yml`
  (only the `server` service and the new `stories` service; the `turn` service belongs to
  package X), `README.md`. `proto/src/lib.rs`: fixes only, with a contract update.
- Tests to add (`server/src/tests.rs`): friend request/accept/decline/cancel/remove/mutual
  auto-accept; blocked pair invisible; direct and group creation need friends; locked
  conversation refuses send/edit/react, history still readable; migration
  `friends_from_direct` on an old database (extend `old_databases_are_migrated`); profile
  validation (bio, font, colour, banner) and an animated GIF avatar producing a 256×256 PNG
  still (fixture encoded in the test with the `image` crate); `/api/users` friends only;
  `/api/users/{id}` visibility; shared media page; edit + mentions (parser unit tests);
  stories: photo live, video 503 without worker, video through a fake worker on
  `internal_router` (claim → input → output → done → live), key checks (live key after expiry
  → 403, archive key → 200), expiry function called with a forged clock, reshare, delete;
  internal API rejects a bad token. Existing tests that create direct conversations between
  strangers get a `befriend` helper.
- Depends on: nothing. Verify: `check.ps1 server`, then `check.ps1`.

### B — Navigation UI, profiles, friends, activity, settings (heavy)
- Scope: §7.2 to §7.5, removal of "Personnes", `NewChat` friends-only, avatar fix,
  `UserAvatar`/`Name` everywhere outside C/D/E files.
- Owns: `app/src/{nav,profile,friends,screens,settings,activity,ui}.rs`,
  `app/{nav,profile}.css`. Regions: `app/src/main.rs` (navigation functions and `Ctx`
  helpers only, if S's version needs fixes; removing `people_tab`), `app/style.css` rules whose
  selector starts with `.avatar`, `.tabs`, `.tab`, `.fab`, `.split`, `.side-pane`,
  `.main-pane`, `.row`, `.welcome`, `.person`, `.profile`; `app/music.css` rules starting with
  `.activity-` or `.rail-`.
- Depends on: S. Uses D's `stories::{StoriesRow, RecentStories}` and C's `chat::row_status`
  through S's stubs.
- Verify: `check.ps1 app`.

### C — Messages (heavy)
- Scope: §7.6 and §4.3.
- Owns: `app/src/chat.rs` (may split into `bubble.rs`, `composer.rs`, declared in `main.rs`
  mod list — a one-line region), `app/src/camera.rs`, `app/src/photos.rs` (attach menu:
  "Caméra" entry), `app/js/{gestures,camera}.js`, `app/chat.css`,
  `app/android/ChatService.kt`, `app/android/MainActivity.kt` (`Bridge` class: add
  `@JavascriptInterface fun vibrate(ms: Long)`). Regions: `app/style.css` rules starting with
  `.bubble`, `.messages`, `.composer`, `.status`, `.quote`, `.reaction`, `.emojis`, `.backdrop`,
  `.sheet`, `.action`, `.viewer`, `.recbar`, `.rec-`, `.day`, `.gone`, `.replying`.
- Depends on: S; server behaviour from A (edit endpoint, mentions, locked) only at run time.
- Verify: `check.ps1 app`.

### D — Home and stories (heavy)
- Scope: §7.2 Accueil, §7.3 desktop home, §7.7.
- Owns: `app/src/{home,stories}.rs`, `app/js/stories.js`, `app/stories.css`.
- Depends on: S. Verify: `check.ps1 app`.

### E — Desktop call UI and screen sharing (heavy: WebRTC renegotiation)
- Scope: §9.
- Owns: `app/src/call.rs`, `app/call.css`. Regions: `app/js/bridge.js` from the line
  `// ---- calls ----` to the end (**only after package X has landed**), `app/src/js.rs` call
  bindings (`on_call_state` … `toggle_camera`, `start_call_with`, plus new ones),
  `app/src-tauri/src/desktop.rs` (`allow_media` functions only), `app/style.css` rules starting
  with `.call` (move them into `call.css`).
- Depends on: S; X for the bridge part. Verify: `check.ps1 app`.

### F — Look and feel (F1 light, parallel after S; F2 medium, last)
- F1 scope: §8.2, §8.3. Owns `app/src/theme.rs`, `app/js/look.js`, `app/look.css` (moves the
  `[data-bubble]`, `[data-wall]`, `[data-compact]` rules out of `style.css`), `app/fonts.css`,
  `app/fonts/*`, the inline `<script>` of `app/index.html`.
- F2 scope: §8.1, §8.4 harmonisation: every transition uses tokens, reduced motion honoured
  everywhere, no stray gradients on the login page, consistent sheet/menu/page animations.
  Owns (after A–E are done): `app/motion.css`, `app/style.css`, `screens::Auth`, and
  transition/animation declarations in any CSS file. Final `check.ps1` (all).
- Depends on: S (F1); everything (F2).

### Summary

| Package | Weight | Starts after | Main files |
|---|---|---|---|
| S scaffold | light | – | main.rs, stubs, index.html, motion.css |
| A server + worker | heavy | – | server/**, deploy compose, README |
| B shell/profiles/friends/activity | heavy | S | nav, profile, friends, screens, settings, activity, ui |
| C messages | heavy | S | chat (+split), camera, gestures.js, camera.js, ChatService.kt |
| D home + stories | heavy | S | home, stories, stories.js |
| E calls UI + screen share | heavy | S (+X for bridge.js) | call.rs, call.css, bridge.js calls |
| F1 customization + fonts | light | S | theme.rs, look.js, look.css, fonts |
| F2 polish | medium | A–E, F1 | motion.css, style.css, Auth |

---------------------------------------------------------------------------------------------

## 13. User answers (2026-11-27). These override anything above that disagrees.

1. Groups: only friends can be added when creating a group. (Default confirmed.)
2. Name colour: a **free colour picker**, not a palette. `name_color` is "" or `#rrggbb`
   (6 hex digits, lowercase on save); `proto::valid_name_color` accepts exactly that, and
   `NAME_COLORS` remains only as suggested swatches in the picker. The server cannot judge
   readability, so the app renders the name with a contrast guard: when the colour is too
   close to the current background (WCAG contrast below 3:1), it is lightened or darkened
   until it reaches 3:1, keeping its hue. Error message stays "Couleur inconnue." for an
   invalid value.
3. Presence on desktop: online whenever the desktop app runs, including in the tray (it can
   show notifications). (Default confirmed.)
4. Story limits confirmed (30 s video, 60 MB upload, archive kept forever and counted in the
   user quota). **The per-user quota becomes 5 GB**: `USER_QUOTA_MB` default `5120` in the
   server config and in `deploy/docker-compose.yml`. Downloading archived stories: no
   decision yet, not in 0.4.0.
5. In-chat camera: **like Snapchat**: tap the shutter for a photo, hold it for a short video
   (same limits as story videos: 30 s, recorded with MediaRecorder, sent as a normal video
   message, kept forever). Release ends the recording; the preview offers "Envoyer" and
   "Reprendre".
6. Worker resources: question re-explained to the user, no answer yet; keep the defaults.
