# Goofy Chat 0.4.0: plan

Agreed with the user on 2026-11-27. Everything ships in **one release** (APK, Windows
installer, Linux AppImage). Do not run the builds until the user asks. No extra features
beyond this list for now; ideas for later are at the end.

## Decisions

- **Only friends can start a direct conversation.** Anyone else can only send a friend
  request, found by exact username. The "Personnes" page (all members) is removed.
- **Stories after 24 hours:** compressed heavily and kept on the server, visible only to
  their author (a private story archive). The author can share an archived story with
  friends again.
- One release for everything.

## Calls (fix first)

Symptom: calls stay on "connecting" and then time out.

Known facts (2026-11-27): coturn runs (`turn` service, host network, port 7880, relay ports
10000-10100, `PUBLIC_IP=82.65.79.199` matches DNS for goofychat.duckdns.org). Two clients
created relay sessions at the same time, so port 7880 is probably reachable from outside.
The user has no router access and is not sure about the forwarding.

Suspects, in order:
1. UDP 10000-10100 not forwarded: allocations work, but no media flows.
2. `--denied-peer-ip=${PUBLIC_IP}` blocks relay-to-relay traffic: when both sides are
   relayed (common on 4G), each side's peer address is the public IP.
3. The app's ICE handling (`app/js/bridge.js`, `RTCPeerConnection`): candidates sent before
   the remote description, or no ICE restart.

Method: run coturn with `--verbose` for one test call, log ICE candidate types and
connection states in the app, then fix. Also remove the empty argument that makes coturn log
`CONFIG: Unknown argument`.

## Profiles

- Banner, bio, animated avatar (GIF or animated WebP, size limit; a still first frame for
  lists, animation on the profile and on hover).
- Username font from about ten bundled fonts (no external font service), and a name colour.
- Fix avatars that look oval or glitched: square crop, `aspect-ratio: 1`, `object-fit: cover`,
  no shrinking in flex rows.
- Desktop: the profile of the person you are talking with shows on the right by default.
- Mobile: tap the conversation header to open the profile.
- From a profile: the photo history between you and that person (grid and full-screen viewer).
- Settings move under your own avatar.

## Friends

- Add by exact username; requests can be accepted or declined; friends list; pending
  requests; remove a friend. Existing conversations between non-friends: decide at migration
  (keep readable, block new messages until friends).

## Messages

- Mobile: a send button in the input bar; the keyboard's Enter key adds a new line.
- "Vu" in the conversation list, from `members.last_read`, without opening the conversation.
- Typing indicator: three animated dots instead of the box.
- Double tap a message: like (heart).
- Long press with a vibration: reactions, edit (store an edited time), delete, ping (@mention
  that notifies the person).
- Snapchat-style camera inside the conversation: shoot and send in one step; the photo stays
  forever.

## Look and feel

- Smooth animations on menus, sheets, page changes and the long-press menu; respect the
  system's reduced-motion setting.
- Chat customization: bubble shape, bubble colour, message font (extend `app/src/theme.rs`).
- Login page: remove the gradients.

## Desktop calls

- Call screen like Discord's: participant tiles, a control bar at the bottom, a speaking
  indicator.
- Screen sharing with `getDisplayMedia`. WebView2 (Windows) supports it; WebKitGTK (Linux)
  is uncertain.

## Mobile home and stories

- Tabs: Accueil, Messages, Activité, Profil.
- Accueil: a friends' posts area with a placeholder (no posting yet) and the stories row.
- Stories: a photo or video for 24 hours, then compressed into the author's archive.
  Compression needs ffmpeg: use a small separate worker container, not the 512 MB chat server.

## Activity page

- Like Discord's: friends' online status, what they listen to on goofysound (already
  exists), recent stories.

## Ideas for later (not in 0.4.0)

Profile song from goofysound, music on stories, listening together, photo memories from
FileBrowser, shared albums, custom status and presence, short notes above the avatar, close
friends list, story replies in DMs, custom emoji and stickers, search, pinned messages, polls,
badges, web version, admin page.
