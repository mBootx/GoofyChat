//! Stories (contract 0.4.0 §3.5, §7.7): the reels in `Ctx::stories` and their events, the
//! bubbles row of the home, the recent stories of the activity page, the full-screen viewer,
//! the composer (camera, gallery, publish) and the author's private archive.
//!
//! The archive viewer reuses `Overlay::Stories` (same history entry, Escape and back
//! handling): the archive screen sets `ARCHIVE_OPEN` before opening the overlay, and
//! `StoryViewer` then browses the archive instead of the reels.

use crate::{
    api, js,
    ui::{Icon, Name, UserAvatar},
    Ctx, Overlay,
};
use dioxus::prelude::*;
use futures_util::StreamExt;
use gloo_timers::future::TimeoutFuture;
use proto::{limits, NewStory, Story, StoryPage, StoryReel, StoryState, User};
use wasm_bindgen::prelude::*;

/// Capture, pick, upload and video controls (`js/stories.js`). Errors are French strings.
#[wasm_bindgen(module = "/js/stories.js")]
extern "C" {
    /// Resolves to the facing in use ("user" or "environment"), "" if closed meanwhile.
    #[wasm_bindgen(catch, js_name = storyCameraOpen)]
    async fn camera_open(video_id: &str, facing: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch, js_name = storyFlip)]
    async fn camera_flip() -> Result<JsValue, JsValue>;
    /// Object URL of the JPEG preview.
    #[wasm_bindgen(catch, js_name = storyShoot)]
    async fn shoot() -> Result<JsValue, JsValue>;
    /// `on_tick` receives the elapsed ms, then "end" when it stopped by itself.
    #[wasm_bindgen(catch, js_name = storyRecordStart)]
    fn record_start(max_ms: f64, on_tick: &JsValue) -> Result<(), JsValue>;
    /// Object URL of the video, "" when the hold was too short for a video.
    #[wasm_bindgen(catch, js_name = storyRecordStop)]
    async fn record_stop() -> Result<JsValue, JsValue>;
    /// Object URL of the chosen (and checked or re-encoded) file, "" when none.
    #[wasm_bindgen(catch, js_name = storyPick)]
    async fn pick(input_id: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = storyPendingVideo)]
    fn pending_video() -> bool;
    /// Uploads the prepared file; `on_progress` receives the fraction sent. Media id.
    #[wasm_bindgen(catch, js_name = storyPublish)]
    async fn upload(on_progress: &JsValue) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = storyDiscard)]
    fn discard();
    #[wasm_bindgen(js_name = storyCameraClose)]
    fn camera_close();
    #[wasm_bindgen(js_name = storyVideoStart)]
    fn video_start(id: &str);
    #[wasm_bindgen(js_name = storyVideoPaused)]
    fn video_paused(id: &str, paused: bool);
    #[wasm_bindgen(js_name = storyVibrate)]
    fn vibrate(ms: f64);
}

const CAM_ID: &str = "story-cam";
const FILE_ID: &str = "story-file";
const VIDEO_ID: &str = "story-video";
const PREVIEW_ID: &str = "story-preview";

// ---- state ----

/// The author's archive as loaded by the archive screen (shared with the viewer).
#[derive(Clone, Default, PartialEq)]
struct ArchiveList {
    stories: Vec<Story>,
    next: Option<i64>,
    loaded: bool,
}

static ARCHIVE: GlobalSignal<ArchiveList> = Signal::global(ArchiveList::default);
/// The archived story shown by the viewer; `Some` puts `StoryViewer` in archive mode.
static ARCHIVE_OPEN: GlobalSignal<Option<i64>> = Signal::global(|| None);

/// Initial state, on (re)connection and after a `friends` event. Older servers have no
/// stories: errors other than a bad session are ignored.
pub async fn load(mut ctx: Ctx) {
    match api::get::<Vec<StoryReel>>(&ctx.token(), "/api/stories").await {
        Ok(reels) => ctx.stories.set(reels),
        Err(f) if f.status == 401 => ctx.fail(f),
        Err(_) => {}
    }
}

fn refresh_unseen(reel: &mut StoryReel) {
    reel.unseen = reel.stories.iter().any(|s| !s.seen);
}

/// The story can be shown (files exist): live, or archived for its author.
fn playable(story: &Story) -> bool {
    matches!(story.state, StoryState::Live | StoryState::Archived)
}

fn first_unseen(reel: &StoryReel) -> usize {
    reel.stories.iter().position(|s| !s.seen).unwrap_or(0)
}

/// `ServerEvent::Story` (and local publishes): insert or replace in the reels, or remove it
/// when it is no longer live, processing or failed; keeps the archive screen current.
pub fn on_story(mut ctx: Ctx, story: Story, user: User) {
    let me = ctx.my_id();
    let author = story.user_id;
    if author == me {
        archive_apply(&story);
    }
    let pos = ctx.stories.peek().iter().position(|r| r.user.id == author);
    if story.state == StoryState::Archived {
        if let Some(i) = pos {
            let mut reels = ctx.stories.write();
            reels[i].stories.retain(|s| s.id != story.id);
            if reels[i].stories.is_empty() {
                reels.remove(i);
            } else {
                refresh_unseen(&mut reels[i]);
            }
        }
        return;
    }
    let mut story = story;
    if author == me {
        story.seen = true;
    }
    let mut reels = ctx.stories.write();
    match pos {
        Some(i) => {
            let reel = &mut reels[i];
            reel.user = user;
            match reel.stories.iter_mut().find(|s| s.id == story.id) {
                Some(existing) => {
                    // Seen locally before the server knew it.
                    story.seen |= existing.seen;
                    *existing = story;
                }
                None => {
                    reel.stories.push(story);
                    reel.stories.sort_by_key(|s| (s.created_at, s.id));
                }
            }
            refresh_unseen(reel);
        }
        None => {
            let unseen = !story.seen;
            // Own reel first, then the newest friend's reel.
            let at = if author == me { 0 } else { usize::from(reels.first().is_some_and(|r| r.user.id == me)) };
            reels.insert(at, StoryReel { user, stories: vec![story], unseen });
        }
    }
}

/// `ServerEvent::StoryGone`: deleted, or expired for friends.
pub fn on_story_gone(mut ctx: Ctx, id: i64, user_id: i64) {
    if ARCHIVE.peek().stories.iter().any(|s| s.id == id) {
        ARCHIVE.write().stories.retain(|s| s.id != id);
    }
    let Some(i) = ctx.stories.peek().iter().position(|r| r.user.id == user_id && r.stories.iter().any(|s| s.id == id)) else {
        return;
    };
    let mut reels = ctx.stories.write();
    reels[i].stories.retain(|s| s.id != id);
    if reels[i].stories.is_empty() {
        reels.remove(i);
    } else {
        refresh_unseen(&mut reels[i]);
    }
}

/// An own story changed: an expired original joins the archive (or is replaced by its
/// compressed copy) when the archive screen has loaded it.
fn archive_apply(story: &Story) {
    let original = story.state == StoryState::Archived && story.reshared_from.is_none();
    let present = ARCHIVE.peek().stories.iter().any(|s| s.id == story.id);
    if !ARCHIVE.peek().loaded || (!original && !present) {
        return;
    }
    let mut archive = ARCHIVE.write();
    if !original {
        archive.stories.retain(|s| s.id != story.id);
    } else if let Some(existing) = archive.stories.iter_mut().find(|s| s.id == story.id) {
        *existing = story.clone();
    } else {
        // Newest first.
        let at = archive.stories.iter().position(|s| s.id < story.id).unwrap_or(archive.stories.len());
        archive.stories.insert(at, story.clone());
    }
}

/// Marks a friend's story seen, locally at once and then on the server.
fn mark_seen(mut ctx: Ctx, author: i64, id: i64) {
    let unseen = ctx
        .stories
        .peek()
        .iter()
        .find(|r| r.user.id == author)
        .and_then(|r| r.stories.iter().find(|s| s.id == id))
        .is_some_and(|s| !s.seen);
    if !unseen {
        return;
    }
    if let Some(reel) = ctx.stories.write().iter_mut().find(|r| r.user.id == author) {
        if let Some(story) = reel.stories.iter_mut().find(|s| s.id == id) {
            story.seen = true;
        }
        refresh_unseen(reel);
    }
    spawn(async move {
        let _ = api::post_empty(&ctx.token(), &format!("/api/stories/{id}/seen")).await;
    });
}

// ---- helpers ----

fn media_url(story: &Story) -> String {
    format!("{}/api/stories/{}/media?k={}", api::SERVER, story.id, story.key)
}

fn poster_url(story: &Story) -> String {
    format!("{}/api/stories/{}/poster?k={}", api::SERVER, story.id, story.key)
}

/// A still for grids and tiles: the photo itself, or the video's poster.
fn thumb_url(story: &Story) -> Option<String> {
    if !playable(story) {
        None
    } else if story.is_video {
        story.has_poster.then(|| poster_url(story))
    } else {
        Some(media_url(story))
    }
}

/// How long the story stays on screen.
fn duration(story: &Story) -> i64 {
    if story.is_video && playable(story) && story.duration_ms > 0 {
        story.duration_ms
    } else {
        limits::STORY_PHOTO_MS
    }
}

/// "il y a 3 h"
fn ago(ms: i64) -> String {
    let minutes = ((js_sys::Date::now() as i64 - ms) / 60_000).max(0);
    match minutes {
        0 => "à l'instant".into(),
        m if m < 60 => format!("il y a {m} min"),
        m if m < 24 * 60 => format!("il y a {} h", m / 60),
        m => format!("il y a {} j", m / (24 * 60)),
    }
}

const MONTHS: [&str; 12] = [
    "janvier", "février", "mars", "avril", "mai", "juin", "juillet", "août", "septembre", "octobre", "novembre", "décembre",
];

fn date(ms: i64) -> js_sys::Date {
    js_sys::Date::new(&JsValue::from_f64(ms as f64))
}

/// "novembre 2026"
fn month_label(ms: i64) -> String {
    let d = date(ms);
    format!("{} {}", MONTHS[d.get_month() as usize % 12], d.get_full_year())
}

/// "27 novembre 2026"
fn day_label(ms: i64) -> String {
    let d = date(ms);
    format!("{} {} {}", d.get_date(), MONTHS[d.get_month() as usize % 12], d.get_full_year())
}

/// "0:12"
fn clock(ms: i64) -> String {
    let s = (ms / 1000).max(0);
    format!("{}:{:02}", s / 60, s % 60)
}

fn reason(e: JsValue, fallback: &str) -> String {
    e.as_string().filter(|s| !s.is_empty()).unwrap_or_else(|| fallback.to_string())
}

fn me(ctx: Ctx) -> Option<User> {
    ctx.session.peek().as_ref().map(|s| s.user.clone())
}

// ---- row and recent stories ----

/// The row of bubbles on the home: own "Ta story" (with "+"), then friends' reels.
#[component]
pub fn StoriesRow() -> Element {
    let ctx = use_context::<Ctx>();
    let Some(user) = ctx.session.read().as_ref().map(|s| s.user.clone()) else {
        return rsx! {};
    };
    let my_id = user.id;
    let reels = ctx.stories.read().clone();
    let own = reels.iter().find(|r| r.user.id == my_id);
    let has_own = own.is_some_and(|r| !r.stories.is_empty());
    let ring = match own {
        Some(r) if r.stories.iter().any(|s| s.state == StoryState::Live) => "story-ring own",
        Some(r) if !r.stories.is_empty() => "story-ring pending",
        _ => "story-ring none",
    };
    let friends: Vec<StoryReel> = reels.iter().filter(|r| r.user.id != my_id).cloned().collect();
    let alone = friends.is_empty();
    rsx! {
        div { class: "stories-row",
            div { class: "story-bubble",
                button { class: "story-open",
                    title: if has_own { "Voir ma story" } else { "Ajouter une story" },
                    onclick: move |_| {
                        if has_own {
                            ctx.open_overlay(Overlay::Stories { user_id: my_id });
                        } else {
                            ctx.open_overlay(Overlay::StoryComposer);
                        }
                    },
                    div { class: ring, UserAvatar { user } }
                    span { class: "story-label", "Ta story" }
                }
                button { class: "story-add", title: "Ajouter une story",
                    onclick: move |_| ctx.open_overlay(Overlay::StoryComposer),
                    Icon { name: "plus" }
                }
            }
            for reel in friends {
                ReelBubble { key: "{reel.user.id}", reel }
            }
            if alone {
                p { class: "stories-hint", "Les stories de vos amis s'afficheront ici pendant 24 h." }
            }
        }
    }
}

#[component]
fn ReelBubble(reel: StoryReel) -> Element {
    let ctx = use_context::<Ctx>();
    let user_id = reel.user.id;
    rsx! {
        div { class: "story-bubble",
            button { class: "story-open", title: "Story de {reel.user.display_name}",
                onclick: move |_| ctx.open_overlay(Overlay::Stories { user_id }),
                div { class: if reel.unseen { "story-ring unseen" } else { "story-ring seen" },
                    UserAvatar { user: reel.user.clone() }
                }
                span { class: "story-label", Name { user: reel.user.clone() } }
            }
        }
    }
}

/// Friends' reels with live stories, newest first, as tiles (activity page and pane).
/// Renders nothing when there are none, so the caller can show its own empty state.
#[component]
pub fn RecentStories() -> Element {
    let ctx = use_context::<Ctx>();
    let me = ctx.my_id();
    let mut reels: Vec<(StoryReel, Story)> = ctx
        .stories
        .read()
        .iter()
        .filter(|r| r.user.id != me)
        .filter_map(|r| {
            let latest = r.stories.iter().filter(|s| s.state == StoryState::Live).max_by_key(|s| s.created_at)?;
            Some((r.clone(), latest.clone()))
        })
        .collect();
    reels.sort_by(|a, b| b.1.created_at.cmp(&a.1.created_at));
    if reels.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "recent-stories",
            for (reel, latest) in reels {
                RecentTile { key: "{reel.user.id}", reel, latest }
            }
        }
    }
}

#[component]
fn RecentTile(reel: StoryReel, latest: Story) -> Element {
    let ctx = use_context::<Ctx>();
    let user_id = reel.user.id;
    let thumb = thumb_url(&latest);
    let mut broken = use_signal(|| false);
    rsx! {
        button { class: if reel.unseen { "recent-story unseen" } else { "recent-story" },
            title: "Story de {reel.user.display_name}",
            onclick: move |_| ctx.open_overlay(Overlay::Stories { user_id }),
            if let Some(src) = thumb.filter(|_| !broken()) {
                img { class: "recent-thumb", src, alt: "", loading: "lazy", draggable: "false", onerror: move |_| broken.set(true) }
            } else {
                div { class: "recent-thumb blank" }
            }
            div { class: if reel.unseen { "story-ring unseen" } else { "story-ring seen" },
                UserAvatar { user: reel.user.clone(), small: true }
            }
            div { class: "recent-text",
                Name { user: reel.user.clone() }
                span { "{ago(latest.created_at)}" }
            }
        }
    }
}

// ---- viewer ----

/// Playback state shared by the reel and archive viewers.
#[derive(Clone, Copy)]
struct Player {
    ctx: Ctx,
    /// Press and hold: everything stops.
    held: Signal<bool>,
    menu: Signal<bool>,
    confirm: Signal<bool>,
    /// The media is still loading: the progress bar waits.
    buffering: Signal<bool>,
    broken: Signal<bool>,
    /// A delete or reshare is in flight.
    busy: Signal<bool>,
    /// Bumped on every move, so the media and bars restart.
    nonce: Signal<u32>,
    closing: Signal<bool>,
    /// Pointer y at press time.
    press: Signal<Option<f64>>,
    press_gen: Signal<u32>,
}

impl PartialEq for Player {
    fn eq(&self, other: &Self) -> bool {
        self.nonce == other.nonce
    }
}

impl Player {
    fn new(ctx: Ctx, loading: bool) -> Self {
        Player {
            ctx,
            held: Signal::new(false),
            menu: Signal::new(false),
            confirm: Signal::new(false),
            buffering: Signal::new(loading),
            broken: Signal::new(false),
            busy: Signal::new(false),
            nonce: Signal::new(0),
            closing: Signal::new(false),
            press: Signal::new(None),
            press_gen: Signal::new(0),
        }
    }

    /// The video stops for the user (hold, menus), not while it loads.
    fn video_held(&self) -> bool {
        *self.held.read() || *self.menu.read() || *self.confirm.read()
    }

    fn paused(&self) -> bool {
        self.video_held() || *self.buffering.read()
    }

    /// Shows another story (or restarts this one).
    fn reset(mut self, loading: bool) {
        self.held.set(false);
        self.menu.set(false);
        self.confirm.set(false);
        self.broken.set(false);
        self.buffering.set(loading);
        self.press.set(None);
        *self.nonce.write() += 1;
    }

    fn close(mut self) {
        if *self.closing.peek() {
            return;
        }
        self.closing.set(true);
        self.ctx.close_overlay();
    }

    fn down(mut self, y: f64) {
        self.press.set(Some(y));
        let n = *self.press_gen.peek() + 1;
        self.press_gen.set(n);
        let (mut held, press, generation) = (self.held, self.press, self.press_gen);
        spawn(async move {
            TimeoutFuture::new(220).await;
            if *generation.peek() == n && press.peek().is_some() {
                held.set(true);
            }
        });
    }

    /// True for a tap (the caller moves); a swipe down closes, the end of a hold resumes,
    /// a tap beside an open menu closes it.
    fn up(mut self, y: f64) -> bool {
        let Some(start) = *self.press.peek() else { return false };
        self.press.set(None);
        *self.press_gen.write() += 1;
        if y - start > 90.0 {
            self.close();
            return false;
        }
        if *self.held.peek() {
            self.held.set(false);
            return false;
        }
        if *self.menu.peek() || *self.confirm.peek() {
            self.menu.set(false);
            self.confirm.set(false);
            return false;
        }
        true
    }

    fn cancel(mut self) {
        self.press.set(None);
        *self.press_gen.write() += 1;
        self.held.set(false);
    }
}

/// Full-screen viewer (`Overlay::Stories`): the reels starting at this author, or the
/// archive when the archive screen opened it.
#[component]
pub fn StoryViewer(user_id: i64) -> Element {
    let archive = use_hook(|| ARCHIVE_OPEN.peek().is_some());
    use_drop(|| {
        if ARCHIVE_OPEN.peek().is_some() {
            *ARCHIVE_OPEN.write() = None;
        }
    });
    if archive {
        rsx! { ArchiveViewer {} }
    } else {
        rsx! { ReelViewer { user_id } }
    }
}

/// Moves through `Ctx::stories`.
#[derive(Clone, Copy)]
struct Reels {
    player: Player,
    user: Signal<i64>,
    index: Signal<usize>,
}

impl Reels {
    fn go(mut self, user: i64, index: usize) {
        let loading = self
            .player
            .ctx
            .stories
            .peek()
            .iter()
            .find(|r| r.user.id == user)
            .and_then(|r| r.stories.get(index))
            .is_some_and(playable);
        self.user.set(user);
        self.index.set(index);
        self.player.reset(loading);
    }

    /// Next story, else the next reel's first unseen story, else close.
    fn next(self) {
        let target = {
            let reels = self.player.ctx.stories.peek();
            let user = *self.user.peek();
            reels.iter().position(|r| r.user.id == user).and_then(|pos| {
                let i = *self.index.peek();
                if i + 1 < reels[pos].stories.len() {
                    Some((user, i + 1))
                } else {
                    reels.get(pos + 1).map(|r| (r.user.id, first_unseen(r)))
                }
            })
        };
        match target {
            Some((user, index)) => self.go(user, index),
            None => self.player.close(),
        }
    }

    /// Previous story, else the previous reel's last story, else restart.
    fn prev(self) {
        let target = {
            let reels = self.player.ctx.stories.peek();
            let user = *self.user.peek();
            reels.iter().position(|r| r.user.id == user).map(|pos| {
                let i = (*self.index.peek()).min(reels[pos].stories.len().saturating_sub(1));
                if i > 0 {
                    (user, i - 1)
                } else if pos > 0 {
                    let r = &reels[pos - 1];
                    (r.user.id, r.stories.len().saturating_sub(1))
                } else {
                    (user, 0)
                }
            })
        };
        match target {
            Some((user, index)) => self.go(user, index),
            None => self.player.close(),
        }
    }
}

#[component]
fn ReelViewer(user_id: i64) -> Element {
    let ctx = use_context::<Ctx>();
    let me = ctx.my_id();
    let (start, loading) = use_hook(move || {
        ctx.stories
            .peek()
            .iter()
            .find(|r| r.user.id == user_id)
            .map(|r| {
                let i = first_unseen(r);
                (i, r.stories.get(i).is_some_and(playable))
            })
            .unwrap_or((0, false))
    });
    let player = use_hook(move || Player::new(ctx, loading));
    let reel_user = use_signal(|| user_id);
    let index = use_signal(|| start);
    let nav = Reels { player, user: reel_user, index };

    // The reel went away (expired, deleted): leave.
    use_effect(move || {
        let user = reel_user();
        let gone = !ctx.stories.read().iter().any(|r| r.user.id == user && !r.stories.is_empty());
        if gone {
            player.close();
        }
    });
    // Each friend story shown is seen.
    use_effect(move || {
        let user = reel_user();
        let i = index();
        if user == ctx.my_id() {
            return;
        }
        let id = ctx.stories.read().iter().find(|r| r.user.id == user).and_then(|r| {
            let last = r.stories.len().checked_sub(1)?;
            r.stories.get(i.min(last)).filter(|s| s.state == StoryState::Live && !s.seen).map(|s| s.id)
        });
        if let Some(id) = id {
            mark_seen(ctx, user, id);
        }
    });
    use_effect(move || video_paused(VIDEO_ID, player.video_held()));

    let reel = ctx.stories.read().iter().find(|r| r.user.id == reel_user() && !r.stories.is_empty()).cloned();
    let Some(reel) = reel else {
        return rsx! { div { class: "story-viewer" } };
    };
    let count = reel.stories.len();
    let current = index().min(count - 1);
    let story = reel.stories[current].clone();
    let mine = reel.user.id == me;
    let mut caption = match story.state {
        StoryState::Processing => "Publication…".to_string(),
        _ => ago(story.created_at),
    };
    if story.reshared_from.is_some() {
        caption.push_str(" · partagée à nouveau");
    }
    let id = story.id;
    let failed = story.state == StoryState::Failed;
    let (mut menu, mut confirm, mut busy) = (player.menu, player.confirm, player.busy);

    let mut index = index;
    let remove = move |_| {
        if busy() {
            return;
        }
        busy.set(true);
        spawn(async move {
            match api::delete(&ctx.token(), &format!("/api/stories/{id}")).await {
                Ok(()) => {
                    busy.set(false);
                    on_story_gone(ctx, id, me);
                    ctx.notify("Story supprimée.");
                    // Keep the index in range if the last story was removed.
                    let next_len = ctx.stories.peek().iter().find(|r| r.user.id == me).map(|r| r.stories.len()).unwrap_or(0);
                    if next_len > 0 {
                        let clamped = current.min(next_len - 1);
                        index.set(clamped);
                        let loading = ctx
                            .stories
                            .peek()
                            .iter()
                            .find(|r| r.user.id == me)
                            .and_then(|r| r.stories.get(clamped))
                            .is_some_and(playable);
                        player.reset(loading);
                    }
                }
                Err(f) => {
                    busy.set(false);
                    confirm.set(false);
                    ctx.fail(f);
                }
            }
        });
    };

    rsx! {
        Stage { player, story, author: reel.user.clone(), caption, count, current, auto: true,
            on_next: move |_| nav.next(), on_prev: move |_| nav.prev(),
            tools: rsx! {
                if mine {
                    button { class: "icon story-more", title: "Options", onclick: move |_| menu.toggle(), Icon { name: "more" } }
                }
            },
            if menu() {
                div { class: "story-menu",
                    button { class: "danger", onclick: move |_| { menu.set(false); confirm.set(true); },
                        Icon { name: "trash" } "Supprimer"
                    }
                }
            }
            if failed && !confirm() {
                div { class: "story-actions",
                    button { class: "story-action danger", onclick: move |_| confirm.set(true), Icon { name: "trash" } "Supprimer" }
                }
            }
            if confirm() {
                Confirm { busy: busy(), on_cancel: move |_| confirm.set(false), on_confirm: remove,
                    text: "Supprimer cette story ?", detail: "Elle disparaît pour tout le monde." }
            }
        }
    }
}

/// Moves through the archive list (no automatic advance).
#[derive(Clone, Copy)]
struct Archived {
    player: Player,
}

impl Archived {
    fn current(&self) -> Option<usize> {
        let open = (*ARCHIVE_OPEN.peek())?;
        ARCHIVE.peek().stories.iter().position(|s| s.id == open)
    }

    fn go(self, index: usize) {
        let target = ARCHIVE.peek().stories.get(index).map(|s| (s.id, playable(s)));
        match target {
            Some((id, loading)) => {
                *ARCHIVE_OPEN.write() = Some(id);
                self.player.reset(loading);
            }
            None => self.player.close(),
        }
    }

    fn next(self) {
        match self.current() {
            Some(i) => self.go(i + 1),
            None => self.player.close(),
        }
    }

    fn prev(self) {
        match self.current() {
            Some(i) => self.go(i.saturating_sub(1)),
            None => self.player.close(),
        }
    }
}

#[component]
fn ArchiveViewer() -> Element {
    let ctx = use_context::<Ctx>();
    let loading = use_hook(|| {
        let open = *ARCHIVE_OPEN.peek();
        ARCHIVE.peek().stories.iter().find(|s| Some(s.id) == open).is_some_and(playable)
    });
    let player = use_hook(move || Player::new(ctx, loading));
    let nav = Archived { player };

    let story = {
        let open = *ARCHIVE_OPEN.read();
        ARCHIVE.read().stories.iter().find(|s| Some(s.id) == open).cloned()
    };
    use_effect(move || {
        let open = *ARCHIVE_OPEN.read();
        let missing = !ARCHIVE.read().stories.iter().any(|s| Some(s.id) == open);
        if missing {
            player.close();
        }
    });
    use_effect(move || video_paused(VIDEO_ID, player.video_held()));

    let (Some(story), Some(author)) = (story, me(ctx)) else {
        return rsx! { div { class: "story-viewer" } };
    };
    let id = story.id;
    let caption = day_label(story.created_at);
    let (mut confirm, mut busy) = (player.confirm, player.busy);

    let reshare = move |_| {
        if busy() {
            return;
        }
        busy.set(true);
        spawn(async move {
            match api::post::<_, Story>(&ctx.token(), &format!("/api/stories/{id}/reshare"), &()).await {
                Ok(story) => {
                    if let Some(user) = me(ctx) {
                        on_story(ctx, story, user);
                    }
                    ctx.notify("Story partagée à nouveau avec vos amis pour 24 h.");
                }
                Err(f) => ctx.fail(f),
            }
            busy.set(false);
        });
    };
    let remove = move |_| {
        if busy() {
            return;
        }
        busy.set(true);
        spawn(async move {
            match api::delete(&ctx.token(), &format!("/api/stories/{id}")).await {
                Ok(()) => {
                    // Show the next one (older), else the previous, else close.
                    let i = nav.current();
                    let me_id = ctx.my_id();
                    on_story_gone(ctx, id, me_id);
                    busy.set(false);
                    ctx.notify("Story supprimée.");
                    let len = ARCHIVE.peek().stories.len();
                    match i {
                        Some(i) if len > 0 => nav.go(i.min(len - 1)),
                        _ => player.close(),
                    }
                }
                Err(f) => {
                    busy.set(false);
                    confirm.set(false);
                    ctx.fail(f);
                }
            }
        });
    };

    rsx! {
        Stage { player, story, author, caption, count: 1, current: 0, auto: false,
            on_next: move |_| nav.next(), on_prev: move |_| nav.prev(),
            tools: rsx! {},
            if !confirm() {
                div { class: "story-actions",
                    button { class: "story-action", disabled: busy(), onclick: reshare, Icon { name: "reshare" } "Partager à nouveau" }
                    button { class: "story-action danger", disabled: busy(), onclick: move |_| confirm.set(true), Icon { name: "trash" } "Supprimer" }
                }
            } else {
                Confirm { busy: busy(), on_cancel: move |_| confirm.set(false), on_confirm: remove,
                    text: "Supprimer cette story ?", detail: "Elle sera retirée de vos archives pour de bon." }
            }
        }
    }
}

/// The frame of a story: media, progress bars, author, tap zones; `tools` go in the
/// header, `children` above the zones (menus, actions).
#[component]
fn Stage(
    player: Player,
    story: Story,
    author: User,
    caption: String,
    count: usize,
    current: usize,
    /// Go to the next story when this one ends.
    auto: bool,
    on_next: EventHandler<()>,
    on_prev: EventHandler<()>,
    tools: Element,
    children: Element,
) -> Element {
    let nonce = *player.nonce.read();
    let ready = playable(&story);
    let paused = player.paused() || !ready;
    let holding = *player.held.read();
    let ms = duration(&story);
    // A playing video ends the story itself (`onended`); the bar only shows the progress.
    let video = story.is_video && ready;
    let broken = player.broken;
    let ended = move |_| {
        if auto {
            on_next.call(());
        }
    };
    let bar_end = move |_| {
        if auto && (!video || *broken.peek()) {
            on_next.call(());
        }
    };
    let media_key = format!("{}-{}-{:?}", story.id, nonce, story.state);
    rsx! {
        div { class: if holding { "story-viewer holding" } else { "story-viewer" },
            oncontextmenu: move |e| e.prevent_default(),
            div { class: "story-stage",
                StoryMedia { key: "{media_key}", story: story.clone(), buffering: player.buffering, broken, on_ended: ended }
                div { class: "story-shade" }
                Bars { key: "{media_key}", count, current, ms, paused, on_end: bar_end }
                div { class: "story-head",
                    UserAvatar { user: author.clone(), small: true }
                    div { class: "story-who",
                        Name { user: author.clone() }
                        span { class: "story-time", "{caption}" }
                    }
                    {tools}
                    button { class: "icon story-close", title: "Fermer", onclick: move |_| player.close(), Icon { name: "hangup" } }
                }
                div { class: "story-zone prev",
                    onpointerdown: move |e| player.down(e.client_coordinates().y),
                    onpointerup: move |e| if player.up(e.client_coordinates().y) { on_prev.call(()) },
                    onpointercancel: move |_| player.cancel(),
                }
                div { class: "story-zone next",
                    onpointerdown: move |e| player.down(e.client_coordinates().y),
                    onpointerup: move |e| if player.up(e.client_coordinates().y) { on_next.call(()) },
                    onpointercancel: move |_| player.cancel(),
                }
                {children}
            }
        }
    }
}

#[component]
fn Bars(count: usize, current: usize, ms: i64, paused: bool, on_end: EventHandler<()>) -> Element {
    let state = if paused { "paused" } else { "running" };
    rsx! {
        div { class: "story-bars",
            for n in 0..count {
                div { key: "{n}", class: "story-bar",
                    if n < current {
                        span { class: "done" }
                    } else if n == current {
                        span { class: "run",
                            style: "animation-duration: {ms}ms; animation-play-state: {state}",
                            onanimationend: move |_| on_end.call(()),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn StoryMedia(story: Story, buffering: Signal<bool>, broken: Signal<bool>, on_ended: EventHandler<()>) -> Element {
    if story.state == StoryState::Processing {
        return rsx! {
            div { class: "story-placeholder",
                span { class: "spinner" }
                b { "Publication…" }
                p { "Votre vidéo est en préparation, elle sera en ligne dans quelques instants." }
            }
        };
    }
    if story.state == StoryState::Failed {
        return rsx! {
            div { class: "story-placeholder",
                Icon { name: "videooff" }
                b { "La vidéo n'a pas pu être publiée." }
            }
        };
    }
    if broken() {
        return rsx! {
            div { class: "story-placeholder",
                Icon { name: "image" }
                b { "Story indisponible." }
            }
        };
    }
    let src = media_url(&story);
    let fail = move |_| {
        broken.set(true);
        buffering.set(false);
    };
    if story.is_video {
        let poster = if story.has_poster { poster_url(&story) } else { String::new() };
        rsx! {
            video { id: VIDEO_ID, class: "story-media", src, poster, preload: "auto",
                "playsinline": "true", "webkit-playsinline": "true",
                onmounted: move |_| video_start(VIDEO_ID),
                onplaying: move |_| buffering.set(false),
                onwaiting: move |_| buffering.set(true),
                onended: move |_| on_ended.call(()),
                onerror: fail,
            }
        }
    } else {
        rsx! {
            img { class: "story-media", src, alt: "", draggable: "false",
                onload: move |_| buffering.set(false),
                onerror: fail,
            }
        }
    }
}

#[component]
fn Confirm(text: &'static str, detail: &'static str, busy: bool, on_cancel: EventHandler<()>, on_confirm: EventHandler<()>) -> Element {
    rsx! {
        div { class: "story-confirm",
            b { "{text}" }
            p { "{detail}" }
            div { class: "story-confirm-buttons",
                button { class: "ghost", disabled: busy, onclick: move |_| on_cancel.call(()), "Annuler" }
                button { class: "primary danger", disabled: busy, onclick: move |_| on_confirm.call(()),
                    if busy { "Suppression…" } else { "Supprimer" }
                }
            }
        }
    }
}

// ---- composer ----

#[derive(Clone, PartialEq)]
enum Cam {
    Opening,
    Ready,
    /// No camera (or refused): the gallery still works.
    Off(String),
}

#[derive(Clone, PartialEq)]
enum Shot {
    None,
    Photo(String),
    Video(String),
}

#[derive(Clone, Copy)]
struct Composer {
    ctx: Ctx,
    cam: Signal<Cam>,
    /// Front camera: the preview is mirrored.
    mirror: Signal<bool>,
    shot: Signal<Shot>,
    recording: Signal<bool>,
    rec_ms: Signal<i64>,
    pressing: Signal<bool>,
    press_gen: Signal<u32>,
    /// Upload fraction while publishing.
    progress: Signal<Option<f64>>,
}

impl Composer {
    fn open_camera(mut self) {
        spawn(async move {
            match camera_open(CAM_ID, "").await {
                Ok(facing) => {
                    let facing = facing.as_string().unwrap_or_default();
                    if !facing.is_empty() {
                        self.mirror.set(facing == "user");
                        self.cam.set(Cam::Ready);
                    }
                }
                Err(e) => self.cam.set(Cam::Off(reason(e, "Caméra indisponible."))),
            }
        });
    }

    fn flip(mut self) {
        if *self.recording.peek() || *self.cam.peek() != Cam::Ready {
            return;
        }
        spawn(async move {
            match camera_flip().await {
                Ok(facing) => self.mirror.set(facing.as_string().unwrap_or_default() == "user"),
                Err(e) => self.ctx.notify(reason(e, "Caméra indisponible.")),
            }
        });
    }

    /// Shutter pressed: a photo on release, a video when held.
    fn down(mut self) {
        if *self.cam.peek() != Cam::Ready || *self.shot.peek() != Shot::None || *self.recording.peek() {
            return;
        }
        self.pressing.set(true);
        let n = *self.press_gen.peek() + 1;
        self.press_gen.set(n);
        spawn(async move {
            TimeoutFuture::new(280).await;
            if *self.pressing.peek() && *self.press_gen.peek() == n {
                self.start_recording();
            }
        });
    }

    /// Released anywhere.
    fn up(mut self) {
        if !*self.pressing.peek() {
            return;
        }
        self.pressing.set(false);
        if *self.recording.peek() {
            self.finish_recording();
        } else {
            self.take_photo();
        }
    }

    fn start_recording(mut self) {
        let (callback, mut ticks) = crate::channel();
        // A little under the limit, so the server never sees more than 30 s.
        if let Err(e) = record_start((limits::STORY_VIDEO_MS - 200) as f64, &callback) {
            self.pressing.set(false);
            self.ctx.notify(reason(e, "Enregistrement impossible."));
            return;
        }
        vibrate(12.0);
        self.rec_ms.set(0);
        self.recording.set(true);
        spawn(async move {
            while let Some(tick) = ticks.next().await {
                if !*self.recording.peek() {
                    break;
                }
                if tick == "end" {
                    self.finish_recording();
                    break;
                }
                if let Ok(ms) = tick.parse::<i64>() {
                    self.rec_ms.set(ms);
                }
            }
        });
    }

    fn finish_recording(mut self) {
        if !*self.recording.peek() {
            return;
        }
        self.recording.set(false);
        self.pressing.set(false);
        spawn(async move {
            match record_stop().await {
                Ok(url) => {
                    let url = url.as_string().unwrap_or_default();
                    if url.is_empty() {
                        // Released too early for a video.
                        self.take_photo();
                    } else {
                        self.shot.set(Shot::Video(url));
                    }
                }
                Err(e) => self.ctx.notify(reason(e, "Enregistrement impossible.")),
            }
        });
    }

    fn take_photo(mut self) {
        spawn(async move {
            match shoot().await {
                Ok(url) => self.shot.set(Shot::Photo(url.as_string().unwrap_or_default())),
                Err(e) => self.ctx.notify(reason(e, "Photo impossible.")),
            }
        });
    }

    /// A file was chosen in the gallery input.
    fn picked(mut self) {
        spawn(async move {
            match pick(FILE_ID).await {
                Ok(url) => {
                    let url = url.as_string().unwrap_or_default();
                    if !url.is_empty() {
                        self.shot.set(if pending_video() { Shot::Video(url) } else { Shot::Photo(url) });
                    }
                }
                Err(e) => self.ctx.notify(reason(e, "Fichier illisible.")),
            }
        });
    }

    fn retake(mut self) {
        if self.progress.peek().is_some() {
            return;
        }
        discard();
        self.shot.set(Shot::None);
    }

    fn publish(mut self) {
        if self.progress.peek().is_some() {
            return;
        }
        let video = matches!(*self.shot.peek(), Shot::Video(_));
        self.progress.set(Some(0.0));
        let (callback, mut fractions) = crate::channel();
        let mut progress = self.progress;
        spawn(async move {
            while let Some(f) = fractions.next().await {
                let active = progress.peek().is_some();
                if active {
                    if let Ok(f) = f.parse::<f64>() {
                        progress.set(Some(f));
                    }
                }
            }
        });
        let ctx = self.ctx;
        spawn(async move {
            let media_id = match upload(&callback).await {
                Ok(id) => id.as_string().unwrap_or_default(),
                Err(e) => {
                    self.progress.set(None);
                    ctx.notify(reason(e, "Échec de l'envoi."));
                    return;
                }
            };
            match api::post::<_, Story>(&ctx.token(), "/api/stories", &NewStory { media_id }).await {
                Ok(story) => {
                    if let Some(user) = me(ctx) {
                        on_story(ctx, story, user);
                    }
                    ctx.notify(if video {
                        "Vidéo envoyée : elle sera en ligne dans quelques instants."
                    } else {
                        "Story publiée pour 24 h."
                    });
                    ctx.close_overlay();
                }
                Err(f) => {
                    self.progress.set(None);
                    ctx.fail(f);
                }
            }
        });
    }
}

/// Capture or pick, preview, publish (`Overlay::StoryComposer`).
#[component]
pub fn StoryComposer() -> Element {
    let ctx = use_context::<Ctx>();
    let comp = use_hook(|| Composer {
        ctx,
        cam: Signal::new(Cam::Opening),
        mirror: Signal::new(false),
        shot: Signal::new(Shot::None),
        recording: Signal::new(false),
        rec_ms: Signal::new(0),
        pressing: Signal::new(false),
        press_gen: Signal::new(0),
        progress: Signal::new(None),
    });
    use_drop(camera_close);

    let cam = comp.cam.read().clone();
    let shot = comp.shot.read().clone();
    let recording = *comp.recording.read();
    let rec_ms = *comp.rec_ms.read();
    let progress = *comp.progress.read();
    let mirror = *comp.mirror.read();
    let ready = cam == Cam::Ready;
    let pct = (progress.unwrap_or(0.0) * 100.0).round().clamp(0.0, 100.0);

    rsx! {
        div { class: "story-composer",
            onpointerup: move |_| comp.up(),
            onpointercancel: move |_| comp.up(),
            oncontextmenu: move |e| e.prevent_default(),
            video { id: CAM_ID, class: if mirror { "story-cam mirror" } else { "story-cam" },
                "playsinline": "true", "muted": "true", "autoplay": "true",
                onmounted: move |_| comp.open_camera(),
            }
            match cam {
                Cam::Opening => rsx! {
                    div { class: "story-cam-note", span { class: "spinner" } }
                },
                Cam::Off(message) => rsx! {
                    div { class: "story-cam-note",
                        Icon { name: "camera" }
                        b { "{message}" }
                        p { "Vous pouvez publier une photo ou une vidéo de l'appareil (30 s max)." }
                        button { class: "primary", onclick: move |_| js::pick_file(FILE_ID), "Choisir dans la galerie" }
                    }
                },
                Cam::Ready => rsx! {},
            }
            div { class: "composer-top",
                button { class: "icon", title: "Fermer", onclick: move |_| ctx.close_overlay(), Icon { name: "hangup" } }
                if recording {
                    span { class: "rec-clock", span { class: "rec-dot" } "{clock(rec_ms)}" }
                }
                if ready && !recording {
                    button { class: "icon", title: "Changer de caméra", onclick: move |_| comp.flip(), Icon { name: "flip" } }
                }
            }
            div { class: "composer-bottom",
                p { class: "composer-hint",
                    if recording { "Relâchez pour arrêter" } else if ready { "Appuyez pour une photo, maintenez pour une vidéo" }
                }
                div { class: "composer-controls",
                    button { class: "icon composer-gallery", title: "Galerie", disabled: recording,
                        onclick: move |_| js::pick_file(FILE_ID), Icon { name: "image" } }
                    button { class: if recording { "shutter recording" } else { "shutter" },
                        title: "Photo ou vidéo", disabled: !ready,
                        onpointerdown: move |_| comp.down(),
                        svg { view_box: "0 0 80 80",
                            circle { class: "shutter-track", cx: "40", cy: "40", r: "36" }
                            if recording {
                                circle { class: "shutter-ring", cx: "40", cy: "40", r: "36" }
                            }
                        }
                    }
                    span { class: "composer-spacer" }
                }
            }
            input { id: FILE_ID, class: "hidden", r#type: "file", accept: "image/*,video/*",
                onchange: move |_| comp.picked() }
            if shot != Shot::None {
                div { class: "story-preview",
                    if progress.is_none() {
                        div { class: "preview-top",
                            button { class: "icon", title: "Fermer", onclick: move |_| ctx.close_overlay(), Icon { name: "hangup" } }
                        }
                    }
                    match shot {
                        Shot::Video(url) => rsx! {
                            video { id: PREVIEW_ID, class: "story-media", src: url, "loop": "true",
                                "playsinline": "true", "webkit-playsinline": "true",
                                onmounted: move |_| video_start(PREVIEW_ID),
                            }
                        },
                        Shot::Photo(url) => rsx! { img { class: "story-media", src: url, alt: "" } },
                        Shot::None => rsx! {},
                    }
                    div { class: "preview-bottom",
                        if progress.is_some() {
                            div { class: "story-upload",
                                span { "Publication… {pct}%" }
                                div { class: "bar", div { style: "width: {pct}%" } }
                            }
                        } else {
                            div { class: "preview-actions",
                                button { class: "ghost", onclick: move |_| comp.retake(), "Reprendre" }
                                button { class: "primary", onclick: move |_| comp.publish(), Icon { name: "send" } "Publier" }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ---- archive ----

/// The author's private archive (`Screen::StoryArchive`): expired stories by month.
#[component]
pub fn StoryArchive() -> Element {
    let ctx = use_context::<Ctx>();
    let mut loading = use_signal(|| false);
    let mut error = use_signal(String::new);

    let mut load_page = move |reset: bool| {
        if *loading.peek() || (!reset && ARCHIVE.peek().next.is_none()) {
            return;
        }
        let path = match (reset, ARCHIVE.peek().next) {
            (false, Some(before)) => format!("/api/stories/archive?before={before}"),
            _ => "/api/stories/archive".to_string(),
        };
        if reset {
            *ARCHIVE.write() = ArchiveList::default();
        }
        loading.set(true);
        error.set(String::new());
        spawn(async move {
            let result = api::get::<StoryPage>(&ctx.token(), &path).await;
            loading.set(false);
            match result {
                Ok(page) => {
                    let mut archive = ARCHIVE.write();
                    for story in page.stories {
                        if !archive.stories.iter().any(|s| s.id == story.id) {
                            archive.stories.push(story);
                        }
                    }
                    archive.next = page.next;
                    archive.loaded = true;
                }
                Err(f) if f.status == 401 => ctx.fail(f),
                Err(f) => error.set(f.message),
            }
        });
    };
    use_hook(move || load_page(true));

    let scrolled = move |e: ScrollEvent| {
        let bottom = e.scroll_top() + e.client_height() as f64;
        if bottom > e.scroll_height() as f64 - 600.0 {
            load_page(false);
        }
    };

    let archive = ARCHIVE.read().clone();
    let mut groups: Vec<(String, Vec<Story>)> = vec![];
    for story in archive.stories {
        let label = month_label(story.created_at);
        match groups.last_mut() {
            Some((last, items)) if *last == label => items.push(story),
            _ => groups.push((label, vec![story])),
        }
    }
    let empty = archive.loaded && groups.is_empty();

    rsx! {
        div { class: "screen story-archive",
            header {
                button { class: "icon", title: "Retour", onclick: move |_| ctx.back(), Icon { name: "back" } }
                h2 { "Archives des stories" small { "Visibles par vous seul" } }
            }
            div { class: "archive-scroll", onscroll: scrolled,
                if !error().is_empty() {
                    div { class: "archive-note error",
                        "{error}"
                        button { class: "link", onclick: move |_| load_page(true), "Réessayer" }
                    }
                }
                if empty {
                    div { class: "archive-empty",
                        div { class: "archive-art", Icon { name: "archive" } }
                        p { "Vos stories expirées arrivent ici. Elles ne sont visibles que par vous." }
                    }
                }
                for (label, items) in groups {
                    section { key: "{label}", class: "archive-month",
                        h4 { "{label}" }
                        div { class: "archive-grid",
                            for story in items {
                                ArchiveTile { key: "{story.id}", story }
                            }
                        }
                    }
                }
                if loading() {
                    div { class: "archive-loading", span { class: "spinner" } "Chargement…" }
                } else if archive.next.is_some() {
                    button { class: "ghost more", onclick: move |_| load_page(false), "Afficher plus" }
                }
            }
        }
    }
}

#[component]
fn ArchiveTile(story: Story) -> Element {
    let ctx = use_context::<Ctx>();
    let mut broken = use_signal(|| false);
    let id = story.id;
    let thumb = thumb_url(&story);
    let d = date(story.created_at);
    let day = d.get_date();
    rsx! {
        button { class: "archive-tile", title: "{day_label(story.created_at)}",
            onclick: move |_| {
                *ARCHIVE_OPEN.write() = Some(id);
                let me = ctx.my_id();
                ctx.open_overlay(Overlay::Stories { user_id: me });
            },
            if let Some(src) = thumb.filter(|_| !broken()) {
                img { src, alt: "", loading: "lazy", decoding: "async", draggable: "false", onerror: move |_| broken.set(true) }
            } else {
                div { class: "archive-missing", Icon { name: crate::ui::pick(story.is_video, "video", "image") } }
            }
            span { class: "archive-day", "{day}" }
            if story.is_video {
                span { class: "archive-length", Icon { name: "play" } "{clock(story.duration_ms)}" }
            }
        }
    }
}
