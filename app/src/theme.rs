//! "Apparence" and "Sons" settings: theme, accent, bubbles, wallpaper, text
//! size, compact mode, ringtone and message sounds. Stored per device by
//! `js/look.js`, applied at once.

use crate::{api, js, music, ui::Icon, Ctx};
use dioxus::prelude::*;
use futures_util::StreamExt;
use gloo_timers::future::TimeoutFuture;
use proto::Track;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq)]
struct Look {
    preset: String,
    /// "auto", "light" or "dark".
    mode: String,
    /// "#rrggbb", empty for the preset's colour.
    accent: String,
    /// "round", "square" or "flat".
    bubble: String,
    /// "s", "m", "l" or "xl".
    size: String,
    /// "none", "dots", "grid", "waves" or "glow".
    wall: String,
    compact: bool,
}

#[derive(Deserialize, Clone, PartialEq)]
struct Preset {
    id: String,
    name: String,
    a: String,
    b: String,
    bg: String,
    surface: String,
    incoming: String,
}

/// A goofysound song used as ringtone. `url` is what `look.js` plays.
#[derive(Serialize, Deserialize, Clone, PartialEq)]
struct RingTrack {
    id: String,
    title: String,
    artist: String,
    duration: i64,
    url: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
struct Sounds {
    /// A built-in tone id, or "track".
    ring: String,
    track: Option<RingTrack>,
    /// Where the song starts, in seconds.
    start: f64,
    msg: bool,
    #[serde(rename = "msgTone")]
    msg_tone: String,
}

fn load_look() -> Look {
    serde_json::from_str(&js::look_get()).unwrap_or(Look {
        preset: "violet".into(),
        mode: "auto".into(),
        accent: String::new(),
        bubble: "round".into(),
        size: "m".into(),
        wall: "none".into(),
        compact: false,
    })
}

fn load_sounds() -> Sounds {
    serde_json::from_str(&js::sounds_get()).unwrap_or(Sounds {
        ring: "classic".into(),
        track: None,
        start: 30.0,
        msg: false,
        msg_tone: "pop".into(),
    })
}

fn presets() -> Vec<Preset> {
    serde_json::from_str(&js::look_presets()).unwrap_or_default()
}

const MODES: [(&str, &str); 3] = [("auto", "Auto"), ("light", "Clair"), ("dark", "Sombre")];
const BUBBLES: [(&str, &str); 3] = [("round", "Arrondies"), ("square", "Carrées"), ("flat", "Unies")];
const WALLS: [(&str, &str); 5] =
    [("none", "Aucun"), ("dots", "Points"), ("grid", "Grille"), ("waves", "Vagues"), ("glow", "Lueur")];
const SIZES: [(&str, &str); 4] = [("s", "Petit"), ("m", "Normal"), ("l", "Grand"), ("xl", "Très grand")];
const ACCENTS: [&str; 8] = ["#6c4cf1", "#0b84d8", "#2f9e5b", "#ef5a3c", "#ec4899", "#f5a524", "#e11d48", "#0fb39a"];
const RINGS: [(&str, &str); 5] =
    [("classic", "Classique"), ("soft", "Douce"), ("retro", "Rétro"), ("goofy", "Goofy"), ("marimba", "Marimba")];
const TONES: [(&str, &str); 5] = [("pop", "Pop"), ("ding", "Ding"), ("drop", "Goutte"), ("bit", "8-bit"), ("boing", "Boing")];

/// Segmented choice among a few options.
#[component]
fn Segments(options: Vec<(&'static str, &'static str)>, value: String, on_pick: EventHandler<&'static str>) -> Element {
    rsx! {
        div { class: "segments", role: "radiogroup",
            for (id, label) in options {
                button { key: "{id}", role: "radio", aria_checked: "{value == id}",
                    class: crate::ui::pick(value == id, "segment on", "segment"),
                    onclick: move |_| on_pick.call(id),
                    "{label}"
                }
            }
        }
    }
}

#[component]
pub fn Appearance() -> Element {
    let mut look = use_signal(load_look);
    // Swatches depend on the mode in effect.
    let mut swatches = use_signal(presets);
    let mut set = move |change: Box<dyn FnOnce(&mut Look)>| {
        let mut next = look.peek().clone();
        change(&mut next);
        if let Ok(json) = serde_json::to_string(&next) {
            js::look_set(&json);
        }
        look.set(next);
        swatches.set(presets());
    };
    let current = look();
    let accent_value = if current.accent.is_empty() {
        swatches.read().iter().find(|p| p.id == current.preset).map(|p| p.a.clone()).unwrap_or("#6c4cf1".into())
    } else {
        current.accent.clone()
    };

    rsx! {
        h3 { "Apparence" }
        div { class: "look-preview", "data-bubble": "{current.bubble}",
            div { class: "bubble", "Salut ! Ça te va, ce thème ?" }
            div { class: "bubble mine", "Carrément 😍" }
        }
        div { class: "setting-label", "Thème" }
        div { class: "presets",
            for p in swatches() {
                button { key: "{p.id}", title: "{p.name}",
                    class: crate::ui::pick(current.preset == p.id, "preset on", "preset"),
                    style: "--pa: {p.a}; --pb: {p.b}; --pbg: {p.bg}; --psurf: {p.surface}; --pin: {p.incoming}",
                    onclick: { let id = p.id.clone(); move |_| { let id = id.clone(); set(Box::new(move |l| { l.preset = id; l.accent.clear(); })) } },
                    span { class: "preset-art", i {} i {} }
                    span { class: "preset-name", "{p.name}" }
                }
            }
        }
        div { class: "setting-label", "Mode" }
        Segments { options: MODES.to_vec(), value: current.mode.clone(),
            on_pick: move |id: &'static str| set(Box::new(move |l| l.mode = id.into())) }
        div { class: "setting-label", "Couleur d'accent" }
        div { class: "accents",
            for color in ACCENTS {
                button { key: "{color}", class: crate::ui::pick(current.accent == color, "accent-dot on", "accent-dot"),
                    style: "background: {color}", title: "{color}",
                    onclick: move |_| set(Box::new(move |l| l.accent = color.into())) }
            }
            label { class: "accent-dot custom", title: "Couleur personnalisée",
                input { r#type: "color", value: "{accent_value}",
                    oninput: move |e| { let v = e.value(); set(Box::new(move |l| l.accent = v)) } }
            }
            if !current.accent.is_empty() {
                button { class: "link", onclick: move |_| set(Box::new(|l| l.accent.clear())), "Couleur du thème" }
            }
        }
        div { class: "setting-label", "Bulles" }
        Segments { options: BUBBLES.to_vec(), value: current.bubble.clone(),
            on_pick: move |id: &'static str| set(Box::new(move |l| l.bubble = id.into())) }
        div { class: "setting-label", "Fond des discussions" }
        div { class: "walls",
            for (id, label) in WALLS {
                button { key: "{id}", class: crate::ui::pick(current.wall == id, "wall on", "wall"), "data-wall": "{id}",
                    onclick: move |_| set(Box::new(move |l| l.wall = id.into())),
                    span { class: "wall-art" }
                    span { "{label}" }
                }
            }
        }
        div { class: "setting-label", "Taille du texte" }
        Segments { options: SIZES.to_vec(), value: current.size.clone(),
            on_pick: move |id: &'static str| set(Box::new(move |l| l.size = id.into())) }
        button { class: "switch-row", role: "switch", aria_checked: "{current.compact}",
            onclick: move |_| set(Box::new(|l| l.compact = !l.compact)),
            div {
                div { "Mode compact" }
                div { class: "hint", "Moins d'espace entre les messages et dans les listes." }
            }
            span { class: crate::ui::pick(current.compact, "switch on", "switch") }
        }
    }
}

#[component]
pub fn SoundSettings() -> Element {
    let ctx = use_context::<Ctx>();
    let mut sounds = use_signal(load_sounds);
    // What the preview button is playing: "ring:<id>" or empty.
    let mut previewing = use_signal(String::new);
    let mut picking = use_signal(|| false);

    use_hook(move || {
        let (callback, mut ends) = crate::channel();
        js::on_preview_end(&callback);
        spawn(async move {
            while ends.next().await.is_some() {
                previewing.set(String::new());
            }
        });
    });
    use_drop(js::sound_stop);

    let mut save = move |change: Box<dyn FnOnce(&mut Sounds)>| {
        let mut next = sounds.peek().clone();
        change(&mut next);
        if let Ok(json) = serde_json::to_string(&next) {
            js::sounds_set(&json);
        }
        sounds.set(next);
    };
    let mut preview = move |kind: String| {
        let playing = js::sound_preview(&kind);
        previewing.set(if playing { kind } else { String::new() });
    };
    let current = sounds();
    let track = current.track.clone();
    let max_start = track.as_ref().map(|t| (t.duration - 5).max(0)).unwrap_or(0);

    rsx! {
        h3 { "Sons" }
        div { class: "setting-label", "Sonnerie des appels" }
        div { class: "tones",
            for (id, label) in RINGS {
                div { key: "{id}", class: crate::ui::pick(current.ring == id, "tone on", "tone"),
                    button { class: "tone-pick", onclick: move |_| save(Box::new(move |s| s.ring = id.into())),
                        span { class: "radio" } "{label}" }
                    button { class: "icon tone-play", title: "Écouter",
                        onclick: move |_| preview(format!("ring:{id}")),
                        Icon { name: crate::ui::pick(previewing() == format!("ring:{id}"), "stop", "play") } }
                }
            }
            div { class: crate::ui::pick(current.ring == "track", "tone on", "tone"),
                button { class: "tone-pick",
                    onclick: move |_| {
                        if sounds.peek().track.is_some() {
                            save(Box::new(|s| s.ring = "track".into()));
                        } else {
                            picking.set(true);
                        }
                    },
                    span { class: "radio" }
                    span { class: "tone-song",
                        "Un morceau goofysound"
                        if let Some(t) = &track {
                            small { "🎵 {t.title} — {t.artist}" }
                        }
                    }
                }
                if track.is_some() {
                    button { class: "icon tone-play", title: "Écouter",
                        onclick: move |_| preview("ring:track".into()),
                        Icon { name: crate::ui::pick(previewing() == "ring:track", "stop", "play") } }
                }
            }
        }
        if current.ring == "track" {
            if let Some(t) = track.clone() {
                div { class: "ring-start",
                    span { "Commencer à {music::clock(current.start)}" }
                    input { r#type: "range", min: "0", max: "{max_start}", step: "1", value: "{current.start}",
                        oninput: move |e| { if let Ok(v) = e.value().parse::<f64>() { save(Box::new(move |s| s.start = v)) } } }
                    button { class: "link", onclick: move |_| picking.set(true), "Changer de morceau" }
                }
                div { class: "hint", "Si « {t.title} » ne peut pas être lu, la sonnerie classique prend le relais." }
            }
        }
        button { class: "switch-row", role: "switch", aria_checked: "{current.msg}",
            onclick: move |_| save(Box::new(|s| s.msg = !s.msg)),
            div {
                div { "Son des nouveaux messages" }
                div { class: "hint", "Quand la discussion n'est pas à l'écran." }
            }
            span { class: crate::ui::pick(current.msg, "switch on", "switch") }
        }
        if current.msg {
            div { class: "chips tone-chips",
                for (id, label) in TONES {
                    button { key: "{id}", class: crate::ui::pick(current.msg_tone == id, "filter-chip on", "filter-chip"),
                        onclick: move |_| { save(Box::new(move |s| s.msg_tone = id.into())); preview(format!("msg:{id}")); },
                        "{label}"
                    }
                }
            }
        }
        if picking() {
            RingtonePicker {
                on_close: move |chosen: Option<Track>| {
                    picking.set(false);
                    if let Some(t) = chosen {
                        let ring = RingTrack {
                            url: music::stream_url(&t),
                            id: t.id,
                            title: t.title,
                            artist: t.artist,
                            duration: t.duration,
                        };
                        let start = if ring.duration > 60 { 30.0 } else { 0.0 };
                        save(Box::new(move |s| { s.ring = "track".into(); s.track = Some(ring); s.start = start; }));
                        ctx.notify("Sonnerie enregistrée.");
                    }
                }
            }
        }
    }
}

/// Searches goofysound for a ringtone song.
#[component]
fn RingtonePicker(on_close: EventHandler<Option<Track>>) -> Element {
    let ctx = use_context::<Ctx>();
    let mut query = use_signal(String::new);
    let mut results = use_signal(Vec::<Track>::new);
    let mut message = use_signal(|| "Cherchez un morceau de votre bibliothèque.".to_string());
    let mut generation = use_signal(|| 0u32);
    use_hook(|| js::focus_input("ring-search"));
    use_drop(|| music::stop("ring:"));

    let search = move |e: FormEvent| {
        let q = e.value();
        query.set(q.clone());
        let n = *generation.peek() + 1;
        generation.set(n);
        spawn(async move {
            TimeoutFuture::new(300).await;
            if *generation.peek() != n || q.trim().is_empty() {
                return;
            }
            let path = format!("/api/music/search?q={}", String::from(js_sys::encode_uri_component(q.trim())));
            let found = api::get::<Vec<Track>>(&ctx.token(), &path).await;
            if *generation.peek() != n {
                return;
            }
            match found {
                Ok(list) => {
                    message.set(if list.is_empty() { "Aucun morceau trouvé.".into() } else { String::new() });
                    results.set(list);
                }
                Err(f) if f.status == 401 => ctx.fail(f),
                Err(f) => {
                    results.set(vec![]);
                    message.set(f.message);
                }
            }
        });
    };

    rsx! {
        div { class: "backdrop", onclick: move |_| on_close.call(None),
            div { class: "sheet music-sheet", onclick: move |e| e.stop_propagation(),
                div { class: "sheet-head",
                    div { class: "sheet-badge", Icon { name: "phone" } }
                    div { class: "sheet-title", b { "Choisir une sonnerie" } span { "depuis goofysound" } }
                    button { class: "icon", title: "Fermer", onclick: move |_| on_close.call(None), Icon { name: "hangup" } }
                }
                label { class: "search-field",
                    Icon { name: "search" }
                    input { id: "ring-search", r#type: "search", placeholder: "Titre, artiste ou album…", value: "{query}",
                        autocomplete: "off", oninput: search }
                }
                div { class: "music-results",
                    if !message().is_empty() {
                        div { class: "sheet-note", "{message}" }
                    }
                    for t in results() {
                        button { key: "{t.id}", class: "music-row pickable",
                            onclick: { let t = t.clone(); move |_| on_close.call(Some(t.clone())) },
                            music::Cover { track: t.clone(), size: 96 }
                            div { class: "music-info",
                                div { class: "music-title", "{t.title}" }
                                div { class: "music-meta", span { "{t.artist}" } }
                            }
                            span { class: "music-duration", "{music::clock(t.duration as f64)}" }
                        }
                    }
                }
            }
        }
    }
}
