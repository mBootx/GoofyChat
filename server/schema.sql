-- Tables are created here. Columns added to an existing table also need an entry
-- in `migrate` (state.rs) for databases created before them.

CREATE TABLE IF NOT EXISTS users (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    username      TEXT NOT NULL UNIQUE,
    display_name  TEXT NOT NULL,
    password_hash TEXT NOT NULL,
    created_at    INTEGER NOT NULL,
    avatar        TEXT,
    -- At most 190 characters and 6 lines.
    bio           TEXT NOT NULL DEFAULT '',
    -- Media id of the banner (JPEG, PNG or WebP).
    banner        TEXT,
    -- Media id of a 256x256 PNG of the first frame when `avatar` is animated.
    avatar_still  TEXT,
    -- "" or a proto::FONTS id.
    name_font     TEXT NOT NULL DEFAULT '',
    -- "" or #rrggbb (lowercase).
    name_color    TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS sessions (
    token_hash TEXT PRIMARY KEY,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS conversations (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT NOT NULL DEFAULT '',
    is_group   INTEGER NOT NULL,
    direct_key TEXT UNIQUE,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS members (
    conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    user_id         INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    last_read       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (conversation_id, user_id)
);

CREATE TABLE IF NOT EXISTS media (
    id         TEXT PRIMARY KEY,
    owner_id   INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    mime       TEXT NOT NULL,
    name       TEXT NOT NULL,
    size       INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation_id INTEGER NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    sender_id       INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    body            TEXT NOT NULL,
    media_id        TEXT REFERENCES media(id),
    created_at      INTEGER NOT NULL,
    -- Not a foreign key: the answer outlives the original.
    reply_to        INTEGER,
    deleted         INTEGER NOT NULL DEFAULT 0,
    -- A shared goofysound song, copied when it was sent.
    track_id        TEXT,
    track_title     TEXT,
    track_artist    TEXT,
    track_album     TEXT,
    track_duration  INTEGER,
    track_cover     INTEGER,
    -- Unix ms of the last edit by the sender.
    edited_at       INTEGER
);

-- goofysound (Navidrome) account of a user: never the password, only a
-- Subsonic salt and token (md5(password + salt)).
CREATE TABLE IF NOT EXISTS navidrome_links (
    user_id    INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    username   TEXT NOT NULL,
    -- One Goofy Chat account per Navidrome user.
    username_key TEXT NOT NULL UNIQUE,
    salt       TEXT NOT NULL,
    token      TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

-- FileBrowser account of a user. The password is encrypted with the server secret.
CREATE TABLE IF NOT EXISTS photo_links (
    user_id    INTEGER PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    username   TEXT NOT NULL,
    password   TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS reactions (
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    emoji      TEXT NOT NULL,
    PRIMARY KEY (message_id, user_id)
);

CREATE TABLE IF NOT EXISTS blocks (
    user_id    INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    blocked_id INTEGER NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (user_id, blocked_id)
);

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
CREATE INDEX IF NOT EXISTS idx_stories_source ON stories(source_id);

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
CREATE INDEX IF NOT EXISTS idx_story_jobs_story ON story_jobs(story_id);

CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conversation_id, id);
CREATE INDEX IF NOT EXISTS idx_messages_media ON messages(media_id);
CREATE INDEX IF NOT EXISTS idx_members_user ON members(user_id);
CREATE INDEX IF NOT EXISTS idx_media_owner ON media(owner_id);
CREATE INDEX IF NOT EXISTS idx_blocks_blocked ON blocks(blocked_id);
