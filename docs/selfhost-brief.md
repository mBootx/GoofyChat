# SelfHost-hub: Navidrome, FileBrowser, camera backup, server facts

Sources read (repo mBootx/SelfHost-hub, read-only):
- crates/hub-net/src/filebrowser.rs (FileBrowser client)
- crates/hub-net/src/subsonic/client.rs, models.rs, json.rs (Subsonic client)
- crates/hub-core/src/subsonic.rs (token auth, query params)
- crates/hub-core/src/vault.rs (folder constants, Photo struct, date derivation, album folder naming)
- crates/hub-core/src/camera_backup.rs (upload layout, naming, month folders)
- crates/hub-core/src/paths.rs (free_name, join_path)
- crates/hub-net/src/url.rs (query encoding)
- Server: `docker ps`, `docker ps -a`, `docker network ls`, `docker inspect navidrome` / `goofychat-server-1` (no env vars printed)

Note on base URLs: the Rust code never hard-codes a host. The Navidrome and FileBrowser base URLs come from the user's settings at runtime (so their exact values are not in the repo).

---

## 1. Subsonic / Navidrome

### Auth (token scheme, Subsonic API >= 1.13)
Every call is a GET (or a POST form for the playlist-replace call) to:

    {base}/rest/{method}.view?{params}

`{base}` = configured Navidrome URL, trimmed, no trailing slash (`base_url()` in hub-core/src/subsonic.rs). The example port in the server is 4533.

Auth query params (`NavidromeLogin::query_parameters`, in this order):

| param | value |
|---|---|
| `u` | username |
| `t` | `md5_hex(password + salt)` (lowercase hex, 32 chars) |
| `s` | salt |
| `v` | `1.16.1` (API_VERSION) |
| `c` | client name: `SelfHostHub` (desktop/phone) or `SelfHostHubWear` (watch) |
| `f` | `json` |

Salt rules:
- `NavidromeLogin::stable`: salt = first 12 hex chars of `md5_hex("{username}@{base_url}")`. Fixed per account and server, so cover URLs stay stable.
- `from_password`: random 12-hex salt.
- The watch gets a salt+token pair from the phone; the server accepts any salt.
- No password is ever put in the URL.

HTTP method: GET for everything, except `createPlaylist` with a full song list (`replace_playlist_songs`), which is a POST with `Content-Type: application/x-www-form-urlencoded` and the signed params as the body.

### Endpoints used (hub-net/src/subsonic/client.rs)

| Purpose | Method name | Extra params | Result key |
|---|---|---|---|
| Ping | `ping` | none | `subsonic-response` (reads `serverVersion` or `version`) |
| Search | `search3` | `query`, `artistCount`, `albumCount`, `songCount` | `subsonic-response.searchResult3` with arrays `artist`, `album`, `song` |
| Stream audio | `stream` | `id`, optional `maxBitRate` (kbit/s, >0 only) | binary audio |
| Original file | `stream` | `id`, `format=raw` | binary |
| Download | `download` | `id` | binary |
| Cover art | `getCoverArt` | `id`, `size` (bucket: 160, 320 or 640 only) | image/* bytes |
| Now playing | `getNowPlaying` | none | `subsonic-response.nowPlaying.entry[]` (defined in client.rs as `now_playing()`; I did not check whether any UI calls it) |

Other methods defined in the same file (for reference): `getArtists`, `getArtist`(`id`), `getAlbumList2`(`type`,`size`,`offset`), `getAlbum`(`id`), `getPlaylists`, `getPlaylist`(`id`), `createPlaylist`(`name`, `songId`...), `updatePlaylist`, `deletePlaylist`(`id`), `star`/`unstar`(`id`), `getStarred2`, `getLyrics`(`artist`,`title`), `scrobble`(`id`,`submission`,`time`), `getScanStatus`, `startScan`, `getSong`, `getRandomSongs`, `getSimilarSongs(2)`, `getGenres`.

Example `search3` request:

    GET {base}/rest/search3.view?u=USER&t=TOKEN&s=SALT&v=1.16.1&c=SelfHostHub&f=json&query=daft&artistCount=0&albumCount=0&songCount=20

Song shape the client reads (`json::song` in json.rs). Field names:
- required: `id`
- `title` (falls back to `"?"`), `artist` (string), `artistId`, `album`, `albumId`, `coverArt`, `duration` (seconds, int), `track`, `year`, `suffix`, plus replay gain and others.

Example response (inferred from the parser, standard Subsonic JSON):

    {"subsonic-response":{"status":"ok","version":"1.16.1","searchResult3":{
      "song":[{"id":"123","title":"Song","artist":"Artist","artistId":"9","album":"Album","albumId":"45","coverArt":"al-45","duration":213,"track":3,"year":2001,"suffix":"flac"}],
      "album":[],"artist":[]}}}

Cover art: `getCoverArt` with `id` = the song's `coverArt` value, `size` = 160/320/640. The response must be `image/*`. A missing cover can come back as HTTP 200 with a JSON error, which the client rejects.

Now-playing shape (standard Subsonic; the client parses it this way): `nowPlaying.entry[]` where each entry has the song fields plus `username`, `minutesAgo` (int), `playerName`.

### Error shape
Subsonic sends HTTP 200 with a failed status in the body:

    {"subsonic-response":{"status":"failed","version":"1.16.1","error":{"code":40,"message":"Wrong username or password"}}}

Known codes: 40, 41, 44 = auth failure (the client maps these to `is_auth_failure`); 70 = not found. A non-2xx HTTP status is reported as "Requête échouée (<status>)". A body that isn't `subsonic-response` is reported as "Réponse Navidrome invalide".

---

## 2. FileBrowser

### Flavour: FileBrowser Quantum (1.3 and later), with legacy fallbacks
The file's own header says so. Evidence: `/api/auth/login` with `X-Password`, `/api/users?id=self`, `/api/resources`, `/api/share`, cookie `filebrowser_quantum_jwt`. Legacy routes are used only on a 404: `/api/search` (instead of `/api/tools/search`), `/api/shares` (instead of `/api/share/list`), `/api/preview` (instead of `/api/resources/preview`).

### Login
- Method and URL: `POST {base}/api/auth/login?username=USER&recaptcha=`
- Headers: `X-Password: <percent-encoded password>` (encodeURIComponent-style), `X-Secret: ` (empty)
- No body. The password is never in the URL.
- Token: response header `Set-Cookie: filebrowser_quantum_jwt=<JWT>; Path=/...`. The client extracts the value after `filebrowser_quantum_jwt=` up to `;`.
- After login the client sends `Authorization: Bearer <JWT>` on every request. It never puts the token in a URL. Direct media URLs carry no login at all: the apps send the Authorization header with them.
- Login failure: 401/403 gives "Identifiants invalides". A 200 without that cookie gives an error mentioning two-factor auth (2FA is not supported).
- Token lifetime / renewal: the client does NOT refresh the token. It signs the user out on 401/403 only ("Session expirée" on a 401 from a listing). Lifetime is not stated in the repo (not checked on the server, since no FileBrowser runs there).

### Source / scope resolution (after login)
1. `GET {base}/api/users?id=self` returns `username` and `scopes[]`. Source = `scopes[0].name`. Account name = `username`.
2. If no scopes: `GET {base}/api/sources`, take the first item's `name`.
3. Else the literal string `default`.
Every resource call then sends `source=<that name>`.

### Listing a folder
`GET {base}/api/resources?path=/Folder/Sub&source=SOURCE`

Response (fields read):
- `folders[]`: `name`, `type` ("directory"), `size`, `modified`, `isShared`... (folders are treated as dirs)
- `files[]`: `name`, `size` (number), `type` (MIME, e.g. `image/jpeg`), `modified` (ISO-8601 string; `mtime` is the fallback name), `hasPreview` (bool, may be absent), `isShared` (bool), `isDir` (bool, optional)

Example:

    {"folders":[{"name":"2026","type":"directory"}],
     "files":[{"name":"IMG_0001.HEIC","size":2345678,"type":"image/heic","hasPreview":true,"isShared":false,"modified":"2026-09-15T10:00:00Z"}]}

The client builds each item's path as `join_path(path, name)`.

### Download / raw / zip
- Download: `GET {base}/api/resources/download?source=SOURCE&file=/path/to/file`
- View inline: same plus `&inline=true` (used for the raw/preview URL)
- Several items as zip: `...&algo=zip&file=/a&file=/b`
(These URLs carry no login. Send the `Authorization: Bearer` header with them.)

### Thumbnails / previews
- `GET {base}/api/resources/preview?source=SOURCE&path=/path/file.jpg&size=small|large&inline=true`
- Legacy: `GET {base}/api/preview?source=...&path=...&size=...&inline=true`
- Sizes: `small` or `large` only (the client picks one by a flag).
- The `hasPreview` field tells whether a thumbnail exists (false for videos without ffmpeg on the server).

### Search
`GET {base}/api/tools/search?query=TEXT&source=SOURCE&scope=/Folder` with a `SessionId: <16 hex chars>` header. Legacy: `/api/search`. Response is an array of `{path, type, size, modified, hasPreview}`. Only indexed content is found.

### Other calls (for reference)
- Upload: `POST {base}/api/resources?path=/dir/name.jpg&source=SOURCE&override=false` with the bytes as body. 409 = a file of that name exists.
- New folder: `POST {base}/api/resources?path=/dir&source=SOURCE&isDir=true`
- Move/rename: `PATCH {base}/api/resources` with JSON `{"items":[{"fromSource","fromPath","toSource","toPath"}],"action":"move","overwrite":false,"rename":false}`
- Delete: `DELETE {base}/api/resources/bulk` with a JSON array `[{"source","path"}]`
- Share: `POST {base}/api/share` JSON `{path, source, expires, unit, password}`; public URL `{base}/public/share/{hash}`
- Usage: `GET {base}/api/settings/sources`

---

## 3. Camera backup layout (hub-core/src/vault.rs and camera_backup.rs)

### Default folder and {user}
- `DEFAULT_BACKUP_FOLDER = "/backups/photos/{user}"`
- `LEGACY_BACKUP_FOLDER = "/Appareil photo"` (old default; an install still on it is migrated, and the old folder is kept as `legacyFolder`)
- `USER_TOKEN = "{user}"`. `vault_root_for(template, username)` replaces `{user}` with `sanitize_username(account name)`, where the account name is the FileBrowser account name as the server spells it (`account_name()`, falling back to the typed username). Then it runs path checks (no `..`, no backslash, no control chars).
- So for account `alice`: root = `/backups/photos/alice`.
- `TRASH_FOLDER = "Corbeille"`, kept 30 days (`TRASH_DAYS`), inside the account root.

### Layout
- Camera roll: `{root}/{YYYY}/{MM}/{filename}`, e.g. `/backups/photos/alice/2026/09/IMG_0001.HEIC`
- Month folder (`month_folder`): year and zero-padded 2-digit month, in the PHONE'S time zone, from the capture time (ms). Format `{root}/{year}/{MM}`. So `"2026/11"` with a slash, not `"2026-11"`.
- Other phone albums: `{root}/{AlbumName}/{YYYY}/{MM}/{filename}`, e.g. `/backups/photos/alice/Screenshots/2026/11/Screenshot_1.png`.
- Album folder name = `album_folder_name(title)`: sanitized like account names; if it looks like a year (4 digits) or equals `Corbeille`, it gets the suffix ` (album)`.
- The camera's own album is titled "Camera"; it has no album folder.
- Album "Camera" is excluded from `device_albums`.

### File naming
- The filename is the phone's own name (`file.filename`, e.g. `IMG_0001.HEIC`).
- If a file with that name exists on the server with the same size, it is skipped (counted as "already there").
- If the name is taken with a different size: `free_name` gives `name (2).ext`, then `(3)`, etc. (`paths.rs`).
- Uploads are sent with `override=false`, so the server never overwrites.

### How the gallery dates photos (hub-core/src/vault.rs, `place_of` and `to_photo`)
- Year and month come ONLY from the folder names below the account root, not from EXIF or the file name.
  - Album = first folder segment, unless it is a 4-digit year.
  - Year = the 4-digit folder (`is_year_folder`: 4 ASCII digits).
  - Month = the next folder, if it is 2 digits and 1..12.
  - Photos without a year folder have `year = None` and sort last.
- `modified` = the server's `modified` field (ISO string parsed by `parse_time`, stored as ms since epoch). Used as the tie-breaker. 0 when unknown.
- Sort by date ("newest first"): compare `year*100+month` (from folders), then `modified`, then name.
- Month headers in the grid use the folder year/month (`month_title`), not the modified time.

---

## 4. Server facts (goofyfiles.duckdns.org, read-only)

Running containers (`docker ps`), name | image | ports | networks:
- `goofychat-server-1` | goofychat-server | 192.168.1.159:8880->8080/tcp | `goofychat_default`
- `goofychat-turn-1` | coturn/coturn:4.9 | (none) | host
- `selfhost-hub` | selfhost-hub-server | 0.0.0.0:51823->51823/tcp | `deploy_default`, `navidrome_default`
- `crafty` | crafty-4 | several | `crafty_default`
- `selfhost-hub-docker-proxy` | tecnativa/docker-socket-proxy:0.3.0 | 2375/tcp | `deploy_default`
- `downtify` | henriquesebastiao/downtify:latest | 0.0.0.0:8087->8000/tcp | `downtify_default`
- `navidrome` | deluan/navidrome:latest | 0.0.0.0:4533->4533/tcp | `navidrome_default`
- `nginx-proxy-manager` | jc21/nginx-proxy-manager | 80-81, 443 | `bridge`
- `dashy` | lissy93/dashy | 0.0.0.0:8085->8080/tcp | `bridge`

Stopped containers: none extra. `docker ps -a` shows 9 containers total, the same list.

Docker networks: `bridge`, `crafty_default`, `deploy_default`, `downtify_default`, `goofychat_default`, `host`, `mboot_default`, `navidrome_default`, `none`.

Navidrome: container `navidrome` is on `navidrome_default` only, IP 172.27.0.2 (gateway 172.27.0.1). Internal port 4533 (exposed `4533/tcp`). Aliases/DNS name: `navidrome`.

FileBrowser: NO container named filebrowser (or containing "file"/"brows") runs or exists on this server. `docker inspect filebrowser` = "No such object". So the FileBrowser the apps use is not on this host. Either the user's config points at another machine, or it is down/removed. This needs checking.

goofychat-server-1: on `goofychat_default` only, IP 172.21.0.2. It is NOT on `navidrome_default` and NOT on `deploy_default`. So it cannot reach `navidrome:4533` by container name.

Reaching Navidrome from goofychat-server-1 (inferred, not tested):
- Container DNS `http://navidrome:4533` will NOT resolve from goofychat-server-1 (different user-defined network).
- Options:
  1. Host gateway: `http://172.21.0.1:4533` (the default gateway of goofychat_default, which reaches host-published ports). Navidrome publishes 0.0.0.0:4533, so this should work. Inferred.
  2. Host LAN IP `http://<host LAN IP>:4533` (the same as the goofychat port binding 192.168.1.159 suggests the host LAN address; not confirmed).
  3. Attach goofychat-server-1 to `navidrome_default` (`docker network connect navidrome_default goofychat-server-1`), then use `http://navidrome:4533`. This is a config change, so it is not done here.
- Same for FileBrowser: no target exists on this host.

---

## Open items / not verified
- Whether `getNowPlaying` is called by the UI (only defined in hub-net client.rs; call sites not searched).
- FileBrowser token lifetime (not in repo; no server to check).
- Host LAN IP (not in ssh output).
- Navidrome `/rest` path prefix: assumes Navidrome at the root URL (default). If a reverse proxy adds a prefix, the base URL includes it.
