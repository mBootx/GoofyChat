//! goofysound (Navidrome) music: the one shared player, the shared-song card and
//! the search sheet of the composer.

use crate::{
    api, chat, js,
    links::LinkNeeded,
    ui::Icon,
    Ctx,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use futures_util::StreamExt;
use gloo_timers::future::TimeoutFuture;
use proto::{Links, NewMessage, Track};
use serde::Deserialize;
use std::cell::Cell;
use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/js/music.js")]
extern "C" {
    #[wasm_bindgen(js_name = musicOnState)]
    fn music_on_state(callback: &JsValue);
    #[wasm_bindgen(js_name = musicPlay)]
    fn music_play(slot: &str, url: &str, title: &str, artist: &str, cover: &str);
    #[wasm_bindgen(js_name = musicPause)]
    pub fn pause();
    #[wasm_bindgen(js_name = musicSeek)]
    fn music_seek(seconds: f64);
    /// Stops the player when its slot starts with the prefix ("" stops anything).
    #[wasm_bindgen(js_name = musicStop)]
    pub fn stop(prefix: &str);
    /// Per-device preference, empty when unset.
    #[wasm_bindgen(js_name = musicPref)]
    pub fn pref(key: &str) -> String;
    #[wasm_bindgen(js_name = setMusicPref)]
    pub fn set_pref(key: &str, value: &str);
}

/// State of the shared player, as reported by `js/music.js`.
#[derive(Clone, Default, Deserialize, PartialEq, Debug)]
pub struct Player {
    /// Who owns the player: "msg:<id>", "preview:<song>" or "activity:<user>".
    pub slot: String,
    pub playing: bool,
    pub loading: bool,
    pub time: f64,
    pub duration: f64,
    pub ended: bool,
    pub error: String,
}

pub static PLAYER: GlobalSignal<Player> = Signal::global(Player::default);

thread_local! {
    static WIRED: Cell<bool> = const { Cell::new(false) };
}

/// Starts forwarding the player state into [`PLAYER`], once.
fn wire() {
    if WIRED.with(|w| w.replace(true)) {
        return;
    }
    let (callback, mut states) = crate::channel();
    music_on_state(&callback);
    spawn_forever(async move {
        while let Some(json) = states.next().await {
            if let Ok(state) = serde_json::from_str::<Player>(&json) {
                *PLAYER.write() = state;
            }
        }
    });
}

/// The player state while it belongs to `slot`. Only changes for that slot re-render.
pub fn use_slot(slot: String) -> Memo<Option<Player>> {
    use_hook(wire);
    use_memo(use_reactive!(|slot| {
        let player = PLAYER.read();
        (player.slot == slot).then(|| player.clone())
    }))
}

fn encode(value: &str) -> String {
    String::from(js_sys::encode_uri_component(value))
}

pub fn stream_url(track: &Track) -> String {
    format!("{}/api/music/{}/stream?k={}", api::SERVER, encode(&track.id), encode(&track.key))
}

pub fn cover_url(track: &Track, size: u32) -> String {
    format!("{}/api/music/{}/cover?k={}&size={size}", api::SERVER, encode(&track.id), encode(&track.key))
}

/// Plays the track in `slot`, pausing anything else.
pub fn play(slot: &str, track: &Track) {
    wire();
    let cover = if track.has_cover { cover_url(track, 300) } else { String::new() };
    music_play(slot, &stream_url(track), &track.title, &track.artist, &cover);
}

/// Play or pause, depending on what `slot` is doing.
pub fn toggle(slot: &str, track: &Track, state: Option<&Player>) {
    if state.is_some_and(|p| p.playing || p.loading) {
        pause();
    } else {
        play(slot, track);
    }
}

/// "3:07"
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Animated bars shown while something plays.
#[component]
pub fn Eq(#[props(default)] paused: bool) -> Element {
    rsx! {
        span { class: if paused { "eq paused" } else { "eq" }, aria_hidden: "true",
            i {} i {} i {} i {}
        }
    }
}

/// Cover art, or a gradient with a note when there is none (or it fails to load).
#[component]
pub fn Cover(track: Track, size: u32, #[props(default)] playing: bool) -> Element {
    let mut broken = use_signal(|| false);
    let show = track.has_cover && !broken();
    rsx! {
        div { class: "cover",
            if show {
                img { src: cover_url(&track, size), alt: "", loading: "lazy", draggable: "false",
                    onerror: move |_| broken.set(true) }
            } else {
                div { class: "cover-empty", Icon { name: "music" } }
            }
            if playing {
                div { class: "cover-eq", Eq {} }
            }
        }
    }
}

/// A shared song inside a message bubble.
#[component]
pub fn TrackCard(track: Track, slot: String) -> Element {
    let state = use_slot(slot.clone());
    let current = state();
    let playing = current.as_ref().is_some_and(|p| p.playing);
    let loading = current.as_ref().is_some_and(|p| p.loading);
    let total = current
        .as_ref()
        .map(|p| p.duration)
        .filter(|d| *d > 0.0)
        .unwrap_or(track.duration as f64)
        .max(1.0);
    let time = current.as_ref().map(|p| if p.ended { 0.0 } else { p.time }).unwrap_or(0.0).min(total);
    let error = current.as_ref().map(|p| p.error.clone()).unwrap_or_default();
    let percent = time / total * 100.0;
    let meta = match (track.artist.is_empty(), track.album.is_empty()) {
        (false, false) => format!("{} · {}", track.artist, track.album),
        (false, true) => track.artist.clone(),
        (true, false) => track.album.clone(),
        (true, true) => "goofysound".to_string(),
    };
    let label = if playing { "Pause" } else { "Lire" };
    let class = match (current.is_some(), playing) {
        (_, true) => "track-card playing",
        (true, false) => "track-card active",
        _ => "track-card",
    };
    let toggled = {
        let (slot, track) = (slot.clone(), track.clone());
        move |_| toggle(&slot, &track, state.peek().as_ref())
    };
    let seek = {
        let (slot, track) = (slot.clone(), track.clone());
        move |e: FormEvent| {
            let Ok(seconds) = e.value().parse::<f64>() else { return };
            // Seeking a card that is not loaded starts it there.
            if state.peek().is_none() {
                play(&slot, &track);
            }
            music_seek(seconds);
        }
    };
    rsx! {
        div { class,
            div { class: "track-top",
                Cover { track: track.clone(), size: 128, playing }
                div { class: "track-info",
                    div { class: "track-title", title: "{track.title}", "{track.title}" }
                    div { class: "track-meta", title: "{meta}", "{meta}" }
                    div { class: "track-brand", Icon { name: "music" } "goofysound" }
                }
                button { class: "track-play", title: label, aria_label: label, onclick: toggled,
                    if loading {
                        span { class: "spinner" }
                    } else {
                        Icon { name: crate::ui::pick(playing, "pause", "play") }
                    }
                }
            }
            div { class: "track-progress",
                input { class: "seek", r#type: "range", min: "0", max: "{total}", step: "0.5", value: "{time}",
                    aria_label: "Position", style: "--p: {percent}%", oninput: seek }
                div { class: "track-times",
                    span { "{clock(time)}" }
                    if !error.is_empty() {
                        span { class: "track-error", "{error}" }
                    }
                    span { "{clock(total)}" }
                }
            }
        }
    }
}

/// One search result: preview and send.
#[component]
fn ResultRow(track: Track, sending: bool, busy: bool, on_send: EventHandler<Track>) -> Element {
    let slot = format!("preview:{}", track.id);
    let state = use_slot(slot.clone());
    let current = state();
    let playing = current.as_ref().is_some_and(|p| p.playing);
    let loading = current.as_ref().is_some_and(|p| p.loading);
    let label = if playing { "Arrêter l'extrait" } else { "Écouter un extrait" };
    let toggled = {
        let track = track.clone();
        move |_| toggle(&slot, &track, state.peek().as_ref())
    };
    let sent = track.clone();
    rsx! {
        div { class: if playing { "music-row playing" } else { "music-row" },
            Cover { track: track.clone(), size: 96, playing }
            div { class: "music-info",
                div { class: "music-title", title: "{track.title}", "{track.title}" }
                div { class: "music-meta",
                    span { "{track.artist}" }
                    if !track.album.is_empty() {
                        span { class: "music-album", " · {track.album}" }
                    }
                }
            }
            span { class: "music-duration", "{clock(track.duration as f64)}" }
            button { class: "mini-play", title: label, aria_label: label, onclick: toggled,
                if loading { span { class: "spinner" } } else { Icon { name: crate::ui::pick(playing, "pause", "play") } }
            }
            button { class: "send-track", disabled: busy, onclick: move |_| on_send.call(sent.clone()),
                if sending { "Envoi…" } else { "Envoyer" }
            }
        }
    }
}

/// Composer sheet: find a song on goofysound and share it.
/// `on_close` receives true when a song was sent.
#[component]
pub fn MusicSheet(id: i64, reply_to: Option<i64>, on_close: EventHandler<bool>) -> Element {
    let ctx = use_context::<Ctx>();
    let mut query = use_signal(String::new);
    let mut results = use_signal(|| None::<Vec<Track>>);
    let mut searching = use_signal(|| false);
    let mut locked = use_signal(String::new);
    let mut error = use_signal(String::new);
    let mut sending = use_signal(|| None::<String>);
    let mut generation = use_signal(|| 0u32);

    use_hook(move || {
        if ctx.desktop || *ctx.wide.peek() {
            js::focus_input("music-search");
        }
        // Tell at once when there is no goofysound account to search with.
        spawn(async move {
            if let Ok(links) = api::get::<Links>(&ctx.token(), "/api/links").await {
                if links.navidrome.is_none() {
                    locked.set("Liez votre compte goofysound dans les réglages.".into());
                }
            }
        });
    });
    // A preview never outlives the sheet.
    use_drop(|| stop("preview:"));

    let search = move |e: FormEvent| {
        let raw = e.value();
        query.set(raw.clone());
        let n = *generation.peek() + 1;
        generation.set(n);
        let q = raw.trim().to_string();
        if q.is_empty() {
            searching.set(false);
            results.set(None);
            error.set(String::new());
            return;
        }
        searching.set(true);
        spawn(async move {
            TimeoutFuture::new(300).await;
            if *generation.peek() != n {
                return;
            }
            let result = api::get::<Vec<Track>>(&ctx.token(), &format!("/api/music/search?q={}", encode(&q))).await;
            // Ignore answers to an outdated query.
            if *generation.peek() != n {
                return;
            }
            searching.set(false);
            match result {
                Ok(list) => {
                    error.set(String::new());
                    results.set(Some(list));
                }
                Err(f) if f.status == 403 => locked.set(f.message),
                Err(f) if f.status == 401 => ctx.fail(f),
                Err(f) => error.set(f.message),
            }
        });
    };

    let send = move |track: Track| {
        if sending.peek().is_some() {
            return;
        }
        sending.set(Some(track.id.clone()));
        spawn(async move {
            let message = NewMessage { body: String::new(), media_id: None, reply_to, track_id: Some(track.id.clone()) };
            if chat::post_message(ctx, id, message).await {
                on_close.call(true);
            } else {
                sending.set(None);
            }
        });
    };

    let busy = sending().is_some();
    rsx! {
        div { class: "backdrop", onclick: move |_| on_close.call(false),
            div { class: "sheet music-sheet", onclick: move |e| e.stop_propagation(),
                div { class: "sheet-head",
                    div { class: "sheet-badge", Icon { name: "music" } }
                    div { class: "sheet-title",
                        b { "Partager un morceau" }
                        span { "depuis goofysound" }
                    }
                    button { class: "icon", title: "Fermer", onclick: move |_| on_close.call(false), Icon { name: "hangup" } }
                }
                if !locked().is_empty() {
                    LinkNeeded { message: locked(), service: "goofysound" }
                } else {
                    label { class: "search-field",
                        Icon { name: "search" }
                        input { id: "music-search", placeholder: "Titre, artiste ou album…", value: "{query}",
                            autocomplete: "off", autocapitalize: "none", r#type: "search", oninput: search }
                        if searching() {
                            span { class: "spinner" }
                        }
                    }
                    div { class: "music-results",
                        if !error().is_empty() {
                            div { class: "sheet-note error-note", "{error}" }
                        }
                        match results() {
                            None => rsx! {
                                div { class: "sheet-note",
                                    div { class: "note-art", Icon { name: "music" } }
                                    "Cherchez un titre, un artiste ou un album de votre bibliothèque."
                                }
                            },
                            Some(list) if list.is_empty() => rsx! {
                                div { class: "sheet-note", "Aucun morceau ne correspond à « {query.peek().trim()} »." }
                            },
                            Some(list) => rsx! {
                                for track in list {
                                    ResultRow { key: "{track.id}", sending: sending().as_deref() == Some(track.id.as_str()),
                                        busy, track: track.clone(), on_send: send }
                                }
                            },
                        }
                    }
                }
            }
        }
    }
}
