# Goofy Chat

Self-hosted chat for Android: one-to-one and group conversations, photo, video
and audio sending, voice messages, replies, reactions, typing indicators, read
receipts, profile photos, a directory of all members, one-to-one voice and video calls, and notifications
without any third-party push service. People can block each other, leave groups,
delete their messages and delete their account. The interface is in French.

There is no end-to-end encryption: HTTPS protects traffic in transit, but the
server administrator can read messages and files.

## Layout

| Folder | Content |
|---|---|
| `server/` | Rust API and WebSocket server (axum, SQLite) |
| `proto/` | Types shared by the server and the app |
| `app/` | Android app: Tauri 2 shell, Dioxus UI, `js/bridge.js` for media and calls, Kotlin notification service |
| `deploy/` | Docker Compose stack: chat server and coturn (call relay) |
| `build/` | Docker image and script that build the signed APK |

## Server

Create `deploy/.env`:

```
TURN_HOST=chat.example.org
TURN_SECRET=<long random string>
PUBLIC_IP=<public IPv4 address>
LAN_IP=<LAN address of the server>
INVITE_CODE=<optional: required to sign up; leave empty for open sign-up>
```

Then run `docker compose up -d --build` in `deploy/`. The server tests run
during the image build.

- A reverse proxy must terminate HTTPS, forward to `LAN_IP:8880` with WebSocket
  support, and set `X-Real-IP`.
- For calls, forward port `7880` (TCP and UDP) and UDP `10000-10100` on the router.
- All data is in the `goofychat_data` Docker volume. Back it up.

## APK

Run `sh build/build-apk.sh` on a Linux machine with Docker. The result is
`dist/GoofyChat.apk`. The server address is set by `SERVER` in `app/src/api.rs`.

The signing key is created on the first build in `~/goofychat-keys`. Back it up:
Android only installs an update signed with the same key.

## Desktop (Windows)

Run `powershell -ExecutionPolicy Bypass -File build\build-desktop.ps1` on
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

## Releasing an update

The app looks at the latest release of `mBootx/GoofyChat` when opened (at most
every six hours) and offers to install it when its version is higher. The
repository must stay public.

1. Raise `version` in `app/src-tauri/tauri.conf.json`, for example to `0.3.0`.
2. Build the APK and the desktop installer.
3. On GitHub, publish a release tagged `v0.3.0` and attach both
   `GoofyChat.apk` (rename `dist/GoofyChat.apk` if needed: the phone takes the
   first asset ending in `.apk`) and `GoofyChat-setup.exe`. Drafts and
   pre-releases are ignored.

Always sign with the same key, or Android refuses the update. Anyone who can
publish a release can offer an update, so protect the GitHub account.

## Known limits

- Calls are one-to-one only.
- Blocking stops private messages, calls and new invitations. Messages from a
  blocked person stay visible in groups you share.
- Messages cannot be edited, and members cannot be added to an existing group.
- A notification already shown stays after its message is deleted.
- Android may still delay notifications on a sleeping phone, and some
  manufacturers block the restart after a reboot.
