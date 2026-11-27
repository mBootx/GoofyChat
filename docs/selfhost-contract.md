# Goofy Chat × SelfHost-hub — API contract (v0.3.0)

Types live in `proto/src/lib.rs` (already added): `Track`, `Activity`, `Links`, `LinkAccount`, `Photo`,
`ImportPhoto`, `Message.track`, `NewMessage.track_id`, `ServerEvent::Activity`.
All JSON field names are the Rust field names (snake_case). Errors: `{"error": "<French message>"}` as today.
Bearer auth (`Authorization: Bearer <session token>`) on everything except the signed `k=` URLs.

## Account links
- `GET /api/links` → `Links`.
- `POST /api/links/navidrome` body `LinkAccount` → verifies with Subsonic `ping` (salt+token), stores the
  Navidrome username + a salt/token pair (never the raw password) → `Links`. 400 "Identifiant ou mot de passe goofysound incorrect."
  A Navidrome username can be linked to only one Goofy Chat account (409 otherwise).
- `DELETE /api/links/navidrome` → 204.
- `POST /api/links/photos` body `LinkAccount` → logs into FileBrowser, stores username + password encrypted at
  rest (key generated once into the data volume), → `Links`. `DELETE /api/links/photos` → 204.
- Deleting the Goofy Chat account deletes its links.

## Music (needs a linked goofysound account for search/send; playback works for every member)
- `GET /api/music/search?q=` → `Vec<Track>` (Subsonic search3 songs, max 30). 403 "Liez votre compte goofysound dans les réglages." when unlinked.
- Sending: `POST /api/conversations/{id}/messages` with `track_id` (body may be empty). The server looks the song
  up and stores id, title, artist, album, duration, has_cover in the message row (columns added via `migrate`).
  Sender id is kept, so streaming can use the sender's credentials.
- `Track.key` = hex HMAC-SHA256(server secret, "track:" + id). The secret is generated once into the data volume.
- `GET /api/music/{id}/stream?k=` → proxied Navidrome stream with Range support (MP3 at most 192 kbps is fine),
  using the credentials of any linked account (prefer the sharer's); 403 on a bad key; 503 when no linked account.
- `GET /api/music/{id}/cover?k=&size=` → proxied cover art (image/*), cacheable.

## Listening activity ("like Discord")
- The server polls Subsonic `getNowPlaying` (with any linked account) every 10 s, maps entries' `username` to
  linked Goofy Chat users, keeps only entries with minutesAgo <= 1 (inferred: Navidrome reports one per player).
- On change it sends `ServerEvent::Activity { user_id, activity }` to everyone who shares a conversation with that
  user (and the user's own devices), skipping blocked pairs. `activity: None` when they stopped.
- `GET /api/activity` → `Vec<Activity>` of the people visible to the caller (same rule), for the initial state.
- Polling stops when no account is linked.

## Photos ("Envoyer depuis mes photos")
- `GET /api/photos?from=YYYY-MM-DD&to=YYYY-MM-DD&cursor=` → `{ "photos": Vec<Photo>, "next": Option<String> }`
  newest first, pages of ~120, from the user's SelfHost-hub camera-backup folder (year/month folders: date
  filter narrows which folders are listed). 403 "Liez vos photos dans les réglages." when unlinked.
- `Photo.key` = HMAC(server secret, "photo:" + user_id + ":" + path).
- `GET /api/photos/thumb?path=&k=` → small preview image (FileBrowser preview/thumbnail endpoint), cacheable.
  Videos: thumbnail if FileBrowser gives one, else 404 (client shows a video icon).
- `POST /api/photos/import` body `ImportPhoto` → downloads the file from FileBrowser into Goofy Chat media
  (same mime allow-list, size limit and quota as uploads, owner = caller) → `Media`. The client then sends a
  normal message with `media_id`.

## Config (deploy/docker-compose.yml, env; empty disables the feature)
- `NAVIDROME_URL` (internal URL reachable from the goofychat-server container)
- `FILEBROWSER_URL`
- `PHOTOS_FOLDER` default = SelfHost-hub's default backup folder with `{user}` = FileBrowser username.

## Client-only (localStorage, per device)
- Theme/customization and ringtone. Ringtone = built-in id, or a `Track` (stream URL via key) from goofysound.
