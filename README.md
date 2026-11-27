# Goofy Chat

Self-hosted chat for Android, Windows and Linux: friends (added by username),
one-to-one and group conversations, rich profiles (bio, banner, animated avatar,
name font and colour), 24-hour photo and video stories with a private archive,
photo, video and audio sending, in-chat camera, voice messages, replies,
reactions, edits, `@mentions`, typing indicators, read receipts, online
presence, one-to-one voice and video calls with screen sharing, and
notifications without any third-party push service. People can block each other,
leave groups, delete their messages and delete their account. Each device can
change the theme, accent colour, bubble style and colours, message font, chat
wallpaper, text size, reduced motion and ringtone. The interface is in French.

There is no end-to-end encryption: HTTPS protects traffic in transit, but the
server administrator can read messages and files.

## Layout

| Folder | Content |
|---|---|
| `server/` | Rust API and WebSocket server (axum, SQLite), plus the `story-worker` binary |
| `proto/` | Types shared by the server and the app |
| `app/` | The app: Dioxus UI, `js/bridge.js` for media and calls, Tauri 2 shell (`src-tauri/`, with `desktop.rs` for the Windows/Linux tray, notifications and updates), Kotlin code for Android (`android/`) |
| `deploy/` | Docker Compose stack: chat server, stories worker (ffmpeg) and coturn (call relay) |
| `build/` | Scripts that build the signed APK (Docker, Linux) and the desktop packages |

## Server

Create `deploy/.env`:

```
TURN_HOST=chat.example.org
TURN_SECRET=<long random string>
PUBLIC_IP=<public IPv4 address>
LAN_IP=<LAN address of the server>
INVITE_CODE=<optional: required to sign up; leave empty for open sign-up>
WORKER_TOKEN=<long random string; empty disables video stories and archive compression>
# Optional: local-time hours window for compressing expired story archives (default 2-6; empty = any time).
ARCHIVE_HOURS=2-6
```

Then run `docker compose up -d --build` in `deploy/`. The server tests run
during the image build.

- A reverse proxy must terminate HTTPS, forward to `LAN_IP:8880` with WebSocket
  support, and set `X-Real-IP`.
- For calls, forward port `7880` (TCP and UDP) and UDP `10000-10100` on the router.
- The `stories` worker container runs `ffmpeg` at low priority (`nice -n 19`,
  1 CPU core, `cpu_shares: 128`, 512 MB RAM, `-threads 1`), one job at a time.
  It prepares new video stories right away (remuxing without re-encoding when
  the upload already fits the live format) and compresses expired story archives
  only during `ARCHIVE_HOURS`. It talks to the server over the private Docker
  network on port `8081` (never published).
- Per-user media storage quota defaults to 5 GB (`USER_QUOTA_MB=5120`), shared
  across messages, profile media and archived stories.
- All data is in the `goofychat_data` Docker volume. Back it up. It includes
  `secret.key`, which signs music, photo and story links and encrypts the stored
  FileBrowser passwords. Losing it means everyone must link their photos again.

## SelfHost-hub (music and photos)

Goofy Chat can use the services of [SelfHost-hub](https://github.com/mBootx/SelfHost-hub):

- **goofysound** (Navidrome): share a song in a conversation as a card with a
  player, use a song as a ringtone, and see what friends are listening to
  (the Activité column on wide screens, 🎵 lines in lists and chat headers).
- **Photos** (FileBrowser Quantum): "Mes photos SelfHost-hub" in the attach
  menu lists the camera backups, filtered by date, and sends the chosen ones.

Optional settings in `deploy/.env` (an empty value turns the feature off):

```
NAVIDROME_URL=http://navidrome:4533          # default; reached through the navidrome_default network
FILEBROWSER_URL=https://files.example.org
PHOTOS_FOLDER=/backups/photos/{user}         # default; {user} is the FileBrowser account name
```

The compose file attaches the server to Navidrome's existing Docker network,
`navidrome_default`. That network must exist before `docker compose up`.

Each person links their accounts once, in Réglages › SelfHost-hub, with their
goofysound and FileBrowser usernames and passwords. The server checks them
first, then keeps:

- for goofysound, the username and a Subsonic salt and token. It never keeps
  the password. A goofysound account can be linked to one Goofy Chat account only;
- for Photos, the username and the password, encrypted with `secret.key`
  (FileBrowser has no other way to log in again).

Unlinking, or deleting the Goofy Chat account, removes them. Shared songs play
for every member of the conversation, using the sharer's account. Who is
listening is visible to friends and to people who share a conversation with that
person, and never between people who blocked each other.

## APK

Run `sh build/build-apk.sh` on a Linux machine with Docker. The result is
`dist/GoofyChat.apk`. The server address is set by `SERVER` in `app/src/api.rs`.

The signing key is created on the first build in `~/goofychat-keys`. Back it up:
Android only installs an update signed with the same key.

## Desktop (Windows)

The usual way is to cross-build on the Linux machine that builds the APK: run
`sh build/build-windows.sh`. It builds a toolchain image on top of the APK
image (NSIS, LLVM, cargo-xwin, which downloads the Windows SDK once and
accepts its licence) and writes `dist/GoofyChat-setup.exe`. Tauri calls this
cross-build experimental, and the installer is not signed.

To build on Windows instead, run `powershell -ExecutionPolicy Bypass -File build\build-desktop.ps1` on
Windows with Rust (MSVC), Visual Studio Build Tools and the WebView2 runtime.
The first run installs nothing automatically except the wasm target; install
the tools once with `cargo install trunk --locked` and
`cargo install tauri-cli --version "^2" --locked`. The result is
`dist-desktop\GoofyChat-setup.exe` (per-user install, no admin needed). Use
`-Server <url>` to build for another server.

- Same features as the phone app. A wide window shows the conversation list
  and the chat side by side.
- Closing the window keeps Goofy Chat in the system tray, so messages and calls
  still notify. Use Quitter in the tray menu to close it.
- Updates: the app checks the same latest GitHub release and installs the asset
  named exactly `GoofyChat-setup.exe`. No signing key is needed, but Windows
  SmartScreen may warn on the first install because the installer is unsigned.

## Desktop (Linux)

Run `sh build/build-linux.sh` on the Linux build machine. The result is
`dist/GoofyChat.AppImage` (x86_64, about 160 MB because it carries WebKitGTK and
the GStreamer plugins for sound, video and calls). It runs on distributions
with glibc 2.36 or newer (Debian 12, Ubuntu 23.04, Fedora 37 and later), not on
Ubuntu 22.04.

- Make it executable (`chmod +x GoofyChat.AppImage`) and start it. Keep it in a
  folder you can write to, such as `~/Applications`: updates replace the file.
- Same shell as on Windows: tray icon, notifications through the desktop's
  notification server, start with the session.
- Updates: the AppImage checks the latest GitHub release, downloads the asset
  named exactly `GoofyChat.AppImage` over itself and restarts.
- Calls depend on WebRTC support in the bundled WebKitGTK; try them first.

## Releasing an update

The app looks at the latest release of `mBootx/GoofyChat` when opened (at most
every six hours) and offers to install it when its version is higher. The
repository must stay public.

1. Raise `version` in `app/src-tauri/tauri.conf.json`, for example to `0.3.0`.
2. Build the APK and the desktop apps (`build/build-apk.sh`,
   `build/build-windows.sh` and `build/build-linux.sh`, all on the Linux build
   machine).
3. On GitHub, publish a release tagged `v0.3.0` and attach
   `GoofyChat.apk` (rename `dist/GoofyChat.apk` if needed: the phone takes the
   first asset ending in `.apk`), `GoofyChat-setup.exe` and
   `GoofyChat.AppImage`. Drafts and pre-releases are ignored.

Always sign with the same key, or Android refuses the update. Anyone who can
publish a release can offer an update, so protect the GitHub account.

## Known limits

- Calls are one-to-one only.
- Blocking stops private messages, calls and new invitations. Messages from a
  blocked person stay visible in groups you share.
- Members cannot be added to an existing group.
- A notification already shown stays after its message is deleted.
- Android may still delay notifications on a sleeping phone, and some
  manufacturers block the restart after a reboot.
