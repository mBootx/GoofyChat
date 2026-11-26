# Goofy Chat

Self-hosted chat for Android: open sign-up, one-to-one and group conversations,
photo, video and audio sending, voice messages, one-to-one voice and video
calls, and notifications without any third-party push service. The interface is
in French.

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

## Known limits

- Calls are one-to-one only.
- There is no way yet to block a user, leave a group or delete a message or account.
- Android may still delay notifications on a sleeping phone. The notification
  service does not restart by itself after a reboot.
