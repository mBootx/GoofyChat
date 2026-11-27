//! Profiles: full screen, desktop side pane, edit, own profile tab, and the photos and
//! videos exchanged with a user (grid and full-screen viewer). Contract 0.4.0 §3.1, §7.5.

use crate::{
    activity, api, friends, js,
    ui::{self, Icon, Name, UserAvatar},
    Ctx, Overlay, Screen,
};
use dioxus::core::spawn_forever;
use dioxus::prelude::*;
use proto::{
    limits, NewConversation, Profile, Relation, SharedMedia, SharedMediaPage, User, UserProfile, FONTS,
    NAME_COLORS,
};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(inline_js = r#"
export async function profileUpload(inputId, maxBytes, types, error) {
  const input = document.getElementById(inputId);
  const file = input?.files?.[0];
  if (input) input.value = "";
  if (!file) return "";
  if (!types.split(",").includes(file.type) || file.size > maxBytes) throw error;
  return await window.GoofyBridge.uploadBlob(file, file.name || "image");
}
export function profileNearBottom(id) {
  const el = document.getElementById(id);
  return !!el && el.scrollTop + el.clientHeight >= el.scrollHeight - 500;
}
export function profileScrollTo(id) {
  setTimeout(() => document.getElementById(id)?.scrollIntoView({ behavior: "smooth", block: "start" }), 120);
}
"#)]
extern "C" {
    /// Checks the picked file (mime in `types`, comma separated, and size) and uploads it
    /// with `GoofyBridge.uploadBlob`: the media id, "" when nothing was picked, or rejects
    /// with `error` / the upload's French message.
    #[wasm_bindgen(catch, js_name = profileUpload)]
    async fn profile_upload(input_id: &str, max_bytes: f64, types: &str, error: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(js_name = profileNearBottom)]
    fn near_bottom(id: &str) -> bool;
    #[wasm_bindgen(js_name = profileScrollTo)]
    fn scroll_to(id: &str);
}

const AVATAR_TYPES: &str = "image/jpeg,image/png,image/webp,image/gif";
const BANNER_TYPES: &str = "image/jpeg,image/png,image/webp";

// ---------------------------------------------------------------------------------------
// Data

/// `GET /api/users/{id}`, fetched again when friendships, requests, blocks or the own
/// profile change. The second signal is true when the user is not visible (404).
fn use_profile(user_id: i64) -> (Signal<Option<UserProfile>>, Signal<bool>) {
    let ctx = use_context::<Ctx>();
    let mut data = use_signal(|| None::<UserProfile>);
    let mut missing = use_signal(|| false);
    use_effect(move || {
        let _ = ctx.friends.read();
        let _ = ctx.requests.read();
        let _ = ctx.blocked.read();
        let _ = ctx.session.read();
        spawn(async move {
            match api::get::<UserProfile>(&ctx.token(), &format!("/api/users/{user_id}")).await {
                Ok(profile) => {
                    missing.set(false);
                    data.set(Some(profile));
                }
                Err(e) if e.status == 404 => missing.set(true),
                Err(e) => ctx.fail(e),
            }
        });
    });
    (data, missing)
}

/// A profile built from what the app already knows, shown while the real one loads.
fn placeholder(user: User, relation: Relation) -> UserProfile {
    UserProfile { user, relation, ..Default::default() }
}

/// The photos and videos exchanged with one user, shared by the grid and the viewer.
#[derive(Clone, Default, PartialEq)]
struct Shared {
    user_id: i64,
    items: Vec<SharedMedia>,
    next: Option<i64>,
    loading: bool,
    /// The first page arrived.
    loaded: bool,
    /// Bumped by each reset, so late answers for an older list are dropped.
    generation: u32,
}

static SHARED: GlobalSignal<Shared> = Signal::global(Shared::default);

/// The profile screen should scroll to its photos once shown (the pane's "Photos" button).
static SCROLL_TO_PHOTOS: GlobalSignal<bool> = Signal::global(|| false);

/// Loads the first page (`reset`) or the next one.
async fn load_photos(ctx: Ctx, user_id: i64, reset: bool) {
    let (before, generation) = {
        let mut shared = SHARED.write();
        if reset {
            let generation = shared.generation.wrapping_add(1);
            *shared = Shared { user_id, generation, ..Default::default() };
        } else if shared.user_id != user_id || shared.loading || !shared.loaded || shared.next.is_none() {
            return;
        }
        shared.loading = true;
        (shared.next, shared.generation)
    };
    let path = match before {
        Some(before) => format!("/api/users/{user_id}/media?before={before}"),
        None => format!("/api/users/{user_id}/media"),
    };
    let result = api::get::<SharedMediaPage>(&ctx.token(), &path).await;
    let failure = {
        let mut shared = SHARED.write();
        if shared.generation != generation {
            return;
        }
        shared.loading = false;
        shared.loaded = true;
        match result {
            Ok(page) => {
                for item in page.items {
                    if !shared.items.iter().any(|i| i.message_id == item.message_id) {
                        shared.items.push(item);
                    }
                }
                shared.next = page.next;
                None
            }
            Err(e) => {
                shared.next = None;
                Some(e)
            }
        }
    };
    if let Some(e) = failure {
        ctx.fail(e);
    }
}

fn is_video(item: &SharedMedia) -> bool {
    item.media.mime.starts_with("video/")
}

// ---------------------------------------------------------------------------------------
// Actions

/// Starts a voice call with a friend, creating the one-to-one conversation if needed.
fn call(ctx: Ctx, user: User, direct: Option<i64>) {
    spawn_forever(async move {
        let conversation = match direct.or_else(|| friends::direct_with(ctx, user.id)) {
            Some(id) => id,
            None => {
                #[derive(serde::Deserialize)]
                struct Created {
                    id: i64,
                }
                let body = NewConversation { member_ids: vec![user.id], name: None };
                match api::post::<_, Created>(&ctx.token(), "/api/conversations", &body).await {
                    Ok(created) => {
                        ctx.refresh().await;
                        created.id
                    }
                    Err(e) => return ctx.fail(e),
                }
            }
        };
        let photo = user.avatar.clone().unwrap_or_default();
        js::start_call_with(conversation as f64, &user.display_name, false, &photo, user.id as f64);
    });
}

fn block(ctx: Ctx, user_id: i64) {
    spawn_forever(async move {
        match api::put_unit(&ctx.token(), &format!("/api/blocks/{user_id}"), &()).await {
            Ok(()) => {
                ctx.notify("Utilisateur bloqué.");
                ctx.back();
                ctx.load_blocked().await;
                friends::load(ctx).await;
                ctx.refresh().await;
            }
            Err(e) => ctx.fail(e),
        }
    });
}

// ---------------------------------------------------------------------------------------
// Pieces

/// Banner, big animated avatar, name, username, bio and dates.
#[component]
fn ProfileCard(profile: UserProfile, #[props(default)] pane: bool) -> Element {
    let ctx = use_context::<Ctx>();
    let user = profile.user.clone();
    let id = user.id;
    let me = profile.relation == Relation::Me || id == ctx.my_id();
    let online = me || ctx.is_online(id);
    let track = activity::track_of(ctx, id);
    let reel = ctx.stories.read().iter().find(|r| r.user.id == id && !r.stories.is_empty()).map(|r| r.unseen);
    let tint = if user.name_color.is_empty() { String::new() } else { format!("--tint: {}", user.name_color) };
    let size = if pane { 84 } else { 108 };
    rsx! {
        div { class: if pane { "profile-card pane" } else { "profile-card" },
            div { class: "profile-banner", style: "{tint}",
                if let Some(banner) = profile.banner.clone() {
                    img { src: api::media_url(&banner), alt: "", draggable: "false" }
                }
            }
            div { class: "profile-head",
                if let Some(unseen) = reel {
                    button { class: if unseen { "profile-avatar story-ring unseen" } else { "profile-avatar story-ring" },
                        title: "Voir la story", onclick: move |_| ctx.open_overlay(Overlay::Stories { user_id: id }),
                        UserAvatar { user: user.clone(), animate: true, online, size: Some(size) }
                    }
                } else {
                    div { class: "profile-avatar",
                        UserAvatar { user: user.clone(), animate: true, online, size: Some(size) }
                    }
                }
            }
            div { class: "profile-body",
                div { class: "profile-name", Name { user: user.clone() } }
                div { class: "profile-handle", "@{user.username}" }
                if let Some(t) = track {
                    div { class: "listening profile-listening", "{activity::line(&t)}" }
                }
                if !profile.bio.is_empty() {
                    p { class: "profile-bio", "{profile.bio}" }
                }
                if profile.joined_at > 0 || profile.friends_since.is_some() {
                    div { class: "profile-facts",
                        if profile.joined_at > 0 {
                            div { Icon { name: "calendar" } span { "Membre depuis le {ui::long_day(profile.joined_at)}" } }
                        }
                        if let Some(since) = profile.friends_since {
                            div { Icon { name: "usercheck" } span { "Amis depuis le {ui::long_day(since)}" } }
                        }
                    }
                }
            }
        }
    }
}

/// What the viewer can do, by relation.
#[component]
fn ProfileActions(profile: UserProfile) -> Element {
    let ctx = use_context::<Ctx>();
    let mut busy = use_signal(|| false);
    let mut menu = use_signal(|| false);
    let user = profile.user.clone();
    let id = user.id;
    let username = user.username.clone();
    let direct = profile.direct_id;
    let mut run = move |what: friends::Change| {
        busy.set(true);
        spawn(async move {
            friends::change(ctx, id, what).await;
            if let Ok(mut b) = busy.try_write() {
                *b = false;
            }
        });
    };
    let actions = match profile.relation {
        Relation::Me => rsx! {
            button { class: "primary", onclick: move |_| ctx.open(Screen::EditProfile), Icon { name: "edit" } "Modifier le profil" }
        },
        Relation::Friend => rsx! {
            button { class: "primary", onclick: move |_| match direct {
                    Some(conversation) => ctx.open(Screen::Chat(conversation)),
                    None => friends::open_direct(ctx, id),
                },
                Icon { name: "chat" } "Message"
            }
            button { class: "ghost", onclick: { let user = user.clone(); move |_| call(ctx, user.clone(), direct) },
                Icon { name: "phone" } "Appeler"
            }
        },
        Relation::Incoming => rsx! {
            button { class: "primary", disabled: busy(), onclick: move |_| run(friends::Change::Accept), Icon { name: "usercheck" } "Accepter" }
            button { class: "ghost", disabled: busy(), onclick: move |_| run(friends::Change::Decline), "Refuser" }
        },
        Relation::Outgoing => rsx! {
            span { class: "pending", Icon { name: "check" } "Demande envoyée" }
            button { class: "ghost", disabled: busy(), onclick: move |_| run(friends::Change::Cancel), "Annuler" }
        },
        Relation::None => rsx! {
            button { class: "primary", disabled: busy(),
                onclick: move |_| {
                    busy.set(true);
                    let username = username.clone();
                    spawn(async move {
                        friends::send_request(ctx, username).await;
                        if let Ok(mut b) = busy.try_write() {
                            *b = false;
                        }
                    });
                },
                Icon { name: "userplus" } "Ajouter en ami"
            }
        },
    };
    rsx! {
        div { class: "profile-actions",
            {actions}
            if profile.relation != Relation::Me {
                button { class: "icon", title: "Plus", onclick: move |_| menu.set(true), Icon { name: "more" } }
            }
        }
        if menu() {
            ProfileMenu { user: user.clone(), friend: profile.relation == Relation::Friend, on_close: move |_| menu.set(false) }
        }
    }
}

/// "Retirer des amis" and "Bloquer", each confirmed by a second tap.
#[component]
fn ProfileMenu(user: User, friend: bool, on_close: EventHandler<()>) -> Element {
    let ctx = use_context::<Ctx>();
    let id = user.id;
    let mut confirm = use_signal(|| None::<&'static str>);
    rsx! {
        div { class: "backdrop", onclick: move |_| on_close.call(()),
            div { class: "sheet", onclick: move |e| e.stop_propagation(),
                div { class: "sheet-title", Name { user: user.clone() } }
                if friend {
                    button { class: "action danger",
                        onclick: move |_| {
                            if confirm() != Some("remove") {
                                confirm.set(Some("remove"));
                                return;
                            }
                            on_close.call(());
                            spawn_forever(friends::change(ctx, id, friends::Change::Remove));
                        },
                        if confirm() == Some("remove") { "Confirmer : retirer des amis" } else { "Retirer des amis" }
                    }
                }
                button { class: "action danger",
                    onclick: move |_| {
                        if confirm() != Some("block") {
                            confirm.set(Some("block"));
                            return;
                        }
                        on_close.call(());
                        block(ctx, id);
                    },
                    if confirm() == Some("block") { "Confirmer : bloquer" } else { "Bloquer" }
                }
                button { class: "action", onclick: move |_| on_close.call(()), "Annuler" }
            }
        }
    }
}

/// The grid of photos and videos exchanged with this user (3 columns, squares).
#[component]
fn SharedPhotos(user_id: i64, #[props(default)] limit: Option<usize>) -> Element {
    let ctx = use_context::<Ctx>();
    use_hook(move || {
        spawn(load_photos(ctx, user_id, true));
        if *SCROLL_TO_PHOTOS.peek() {
            *SCROLL_TO_PHOTOS.write() = false;
            scroll_to("profile-photos");
        }
    });
    let shared = SHARED.read().clone();
    let mine = shared.user_id == user_id;
    let mut items = if mine { shared.items } else { vec![] };
    let more = mine && shared.next.is_some();
    let clipped = limit.is_some_and(|n| items.len() > n);
    if let Some(n) = limit {
        items.truncate(n);
    }
    rsx! {
        section { class: "profile-photos", id: "profile-photos",
            h3 { class: "section-title", Icon { name: "grid" } "Photos" }
            if mine && shared.loaded && items.is_empty() {
                div { class: "hint", "Aucune photo ni vidéo échangée pour l'instant." }
            }
            div { class: "photo-grid",
                for item in items {
                    {
                        let message_id = item.message_id;
                        let url = api::media_url(&item.media.id);
                        let video = is_video(&item);
                        rsx! {
                            button { key: "{message_id}", class: "photo-cell",
                                onclick: move |_| ctx.open_overlay(Overlay::Media { user_id, message_id }),
                                if video {
                                    video { src: "{url}#t=0.1", preload: "metadata", muted: true, playsinline: true }
                                    span { class: "photo-play", Icon { name: "play" } }
                                } else {
                                    img { src: "{url}", alt: "", loading: "lazy", draggable: "false" }
                                }
                            }
                        }
                    }
                }
            }
            if !mine || (shared.loading && !shared.loaded) {
                div { class: "hint", "Chargement…" }
            }
            if clipped {
                button { class: "link", onclick: move |_| { *SCROLL_TO_PHOTOS.write() = true; ctx.open(Screen::Profile(user_id)); }, "Tout voir" }
            } else if more && limit.is_none() {
                button { class: "link", disabled: shared.loading,
                    onclick: move |_| { spawn(load_photos(ctx, user_id, false)); },
                    if shared.loading { "Chargement…" } else { "Voir plus" }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Screens

/// A user's profile, full screen (`Screen::Profile`).
#[component]
pub fn ProfileScreen(user_id: i64) -> Element {
    let ctx = use_context::<Ctx>();
    let (data, missing) = use_profile(user_id);
    // Shown at once from what the app knows (friends, conversations, self).
    let known = data.read().clone().or_else(|| {
        if user_id == ctx.my_id() {
            return ctx.session.peek().as_ref().map(|s| placeholder(s.user.clone(), Relation::Me));
        }
        if let Some(friend) = ctx.friend(user_id) {
            return Some(placeholder(friend.user, Relation::Friend));
        }
        None
    });
    let title = known.as_ref().map(|p| p.user.display_name.clone()).unwrap_or_else(|| "Profil".into());
    let show_photos = known.as_ref().is_some_and(|p| p.relation != Relation::Me);
    rsx! {
        div { class: "screen profile-screen",
            header {
                button { class: "icon", title: "Retour", onclick: move |_| ctx.back(), Icon { name: "back" } }
                h2 { "{title}" }
            }
            div { class: "list profile-scroll", id: "profile-scroll",
                onscroll: move |_| {
                    if near_bottom("profile-scroll") {
                        spawn(load_photos(ctx, user_id, false));
                    }
                },
                if missing() && data.read().is_none() {
                    div { class: "empty", "Profil introuvable." }
                } else if let Some(profile) = known {
                    div { class: "profile-page",
                        ProfileCard { profile: profile.clone() }
                        if data.read().is_some() {
                            ProfileActions { profile: profile.clone() }
                        }
                        if show_photos {
                            SharedPhotos { user_id }
                        }
                    }
                } else {
                    div { class: "empty", "Chargement…" }
                }
            }
        }
    }
}

/// The right pane of the desktop layout while a one-to-one conversation is open.
#[component]
pub fn ProfilePane(user_id: i64) -> Element {
    let ctx = use_context::<Ctx>();
    let (data, missing) = use_profile(user_id);
    let known = data.read().clone().or_else(|| {
        let user = ctx
            .conversations
            .read()
            .iter()
            .flat_map(|c| c.members.iter())
            .find(|u| u.id == user_id)
            .cloned()?;
        Some(placeholder(user, Relation::None))
    });
    let Some(profile) = known else {
        return rsx! { div { class: "profile-pane" } };
    };
    rsx! {
        div { class: "profile-pane",
            ProfileCard { profile: profile.clone(), pane: true }
            if missing() {
                div { class: "hint pane-hint", "Ce profil n'est plus visible." }
            } else {
                div { class: "pane-buttons",
                    button { class: "ghost", onclick: move |_| ctx.open(Screen::Profile(user_id)), "Profil complet" }
                    button { class: "ghost",
                        onclick: move |_| {
                            *SCROLL_TO_PHOTOS.write() = true;
                            ctx.open(Screen::Profile(user_id));
                        },
                        Icon { name: "grid" } "Photos"
                    }
                }
                if data.read().is_some() {
                    SharedPhotos { key: "{user_id}", user_id, limit: Some(9) }
                }
            }
        }
    }
}

/// The "Profil" tab of the mobile home: own profile and the ways to everything personal.
#[component]
pub fn MyProfile() -> Element {
    let ctx = use_context::<Ctx>();
    let me = ctx.my_id();
    let (data, _) = use_profile(me);
    let requests = ctx.requests.read().incoming.len();
    let known = data.read().clone().or_else(|| ctx.session.read().as_ref().map(|s| placeholder(s.user.clone(), Relation::Me)));
    rsx! {
        div { class: "screen my-profile",
            header {
                h2 { "Profil" }
                button { class: "icon", title: "Réglages", onclick: move |_| ctx.open(Screen::Settings), Icon { name: "settings" } }
            }
            div { class: "list",
                if let Some(profile) = known {
                    ProfileCard { profile }
                }
                div { class: "menu-list",
                    button { class: "menu-item", onclick: move |_| ctx.open(Screen::EditProfile),
                        Icon { name: "edit" } span { "Modifier le profil" } Icon { name: "chevron" } }
                    button { class: "menu-item", onclick: move |_| ctx.open(Screen::Friends),
                        Icon { name: "users" } span { "Amis" }
                        if requests > 0 {
                            b { class: "badge", "{requests}" }
                        }
                        Icon { name: "chevron" } }
                    button { class: "menu-item", onclick: move |_| ctx.open(Screen::StoryArchive),
                        Icon { name: "archive" } span { "Archives des stories" } Icon { name: "chevron" } }
                    button { class: "menu-item", onclick: move |_| ctx.open(Screen::Settings),
                        Icon { name: "settings" } span { "Réglages" } Icon { name: "chevron" } }
                }
            }
        }
    }
}

/// Edit own profile (`Screen::EditProfile`): photo (animated allowed), banner, name, bio,
/// name font and colour, with a live preview.
#[component]
pub fn EditProfile() -> Element {
    let ctx = use_context::<Ctx>();
    let Some(user) = ctx.session.read().as_ref().map(|s| s.user.clone()) else { return rsx! {} };
    let mut name = use_signal(|| user.display_name.clone());
    let mut bio = use_signal(String::new);
    let mut font = use_signal(|| user.name_font.clone());
    let mut color = use_signal(|| user.name_color.clone());
    // A newly uploaded photo / banner, or the current one removed.
    let mut new_avatar = use_signal(|| None::<String>);
    let mut drop_avatar = use_signal(|| false);
    let mut new_banner = use_signal(|| None::<String>);
    let mut drop_banner = use_signal(|| false);
    let mut banner = use_signal(|| None::<String>);
    let mut joined = use_signal(|| 0i64);
    let mut uploading = use_signal(|| None::<&'static str>);
    let mut saving = use_signal(|| false);
    let me = user.id;
    use_hook(move || {
        spawn(async move {
            match api::get::<UserProfile>(&ctx.token(), &format!("/api/users/{me}")).await {
                Ok(profile) => {
                    bio.set(profile.bio);
                    banner.set(profile.banner);
                    joined.set(profile.joined_at);
                }
                Err(e) => ctx.fail(e),
            }
        });
    });

    let mut pick = move |kind: &'static str| {
        let (input, max, types, error) = if kind == "avatar" {
            ("avatar-file", limits::AVATAR_BYTES, AVATAR_TYPES, "Choisissez une image JPEG, PNG, WebP ou GIF de 8 Mo au plus.")
        } else {
            ("banner-file", limits::BANNER_BYTES, BANNER_TYPES, "Choisissez une bannière JPEG, PNG ou WebP de 8 Mo au plus.")
        };
        uploading.set(Some(kind));
        spawn(async move {
            let result = profile_upload(input, max as f64, types, error).await;
            if let Ok(mut u) = uploading.try_write() {
                *u = None;
            }
            match result {
                Ok(value) => {
                    let Some(id) = value.as_string().filter(|s| !s.is_empty()) else { return };
                    if kind == "avatar" {
                        new_avatar.set(Some(id));
                        drop_avatar.set(false);
                    } else {
                        new_banner.set(Some(id));
                        drop_banner.set(false);
                    }
                }
                Err(e) => ctx.notify(e.as_string().unwrap_or_else(|| "Échec de l'envoi.".into())),
            }
        });
    };

    // What the profile will look like.
    let shown_avatar = new_avatar().or_else(|| if drop_avatar() { None } else { user.avatar.clone() });
    let shown_banner = new_banner().or_else(|| if drop_banner() { None } else { banner() });
    let bio_chars = bio().trim().chars().count();
    let bio_lines = bio().trim().lines().count();
    let bio_ok = bio_chars <= limits::BIO_CHARS && bio_lines <= limits::BIO_LINES;
    let name_chars = name().trim().chars().count();
    let name_ok = (1..=40).contains(&name_chars);
    let preview_user = User {
        display_name: if name().trim().is_empty() { user.display_name.clone() } else { name().trim().to_string() },
        avatar: shown_avatar.clone(),
        avatar_still: None,
        name_font: font(),
        name_color: color(),
        ..user.clone()
    };
    let preview = UserProfile {
        user: preview_user.clone(),
        bio: bio().trim().to_string(),
        banner: shown_banner.clone(),
        relation: Relation::Me,
        joined_at: joined(),
        ..Default::default()
    };
    let adjusted = !color().is_empty() && crate::theme::name_color(&color()) != color();
    let picker_value = if color().is_empty() { "#6c4cf1".to_string() } else { color() };

    let save = move |_| {
        if saving() || uploading().is_some() {
            return;
        }
        saving.set(true);
        let profile = Profile {
            display_name: name().trim().to_string(),
            avatar_id: new_avatar(),
            remove_avatar: drop_avatar() && new_avatar().is_none(),
            bio: Some(bio().trim().to_string()),
            banner_id: new_banner(),
            remove_banner: drop_banner() && new_banner().is_none(),
            name_font: Some(font()),
            name_color: Some(color().to_lowercase()),
        };
        spawn_forever(async move {
            match api::put::<_, User>(&ctx.token(), "/api/me", &profile).await {
                Ok(user) => {
                    ctx.set_user(user);
                    ctx.notify("Profil enregistré.");
                    ctx.back();
                }
                Err(e) => ctx.fail(e),
            }
            if let Ok(mut s) = saving.try_write() {
                *s = false;
            }
        });
    };

    rsx! {
        div { class: "screen edit-profile",
            header {
                button { class: "icon", title: "Retour", onclick: move |_| ctx.back(), Icon { name: "back" } }
                h2 { "Modifier le profil" }
                button { class: "primary header-save", disabled: saving() || uploading().is_some() || !bio_ok || !name_ok, onclick: save,
                    if saving() { "Enregistrement…" } else { "Enregistrer" }
                }
            }
            div { class: "list edit-body",
                div { class: "edit-preview",
                    span { class: "setting-label", "Aperçu" }
                    ProfileCard { profile: preview, pane: true }
                }

                input { id: "avatar-file", class: "hidden", r#type: "file", accept: AVATAR_TYPES, onchange: move |_| pick("avatar") }
                input { id: "banner-file", class: "hidden", r#type: "file", accept: BANNER_TYPES, onchange: move |_| pick("banner") }

                section { class: "edit-section",
                    h3 { "Photo de profil" }
                    p { class: "hint", "JPEG, PNG, WebP ou GIF, 8 Mo au plus. Les GIF et WebP animés restent animés sur votre profil." }
                    div { class: "edit-row",
                        button { class: "ghost", disabled: uploading().is_some(), onclick: move |_| js::pick_file("avatar-file"),
                            if uploading() == Some("avatar") { "Envoi…" } else { "Changer la photo" }
                        }
                        if shown_avatar.is_some() {
                            button { class: "link", onclick: move |_| { new_avatar.set(None); drop_avatar.set(true); }, "Retirer" }
                        }
                    }
                }

                section { class: "edit-section",
                    h3 { "Bannière" }
                    p { class: "hint", "JPEG, PNG ou WebP, 8 Mo au plus, affichée au format 3:1." }
                    div { class: "edit-row",
                        button { class: "ghost", disabled: uploading().is_some(), onclick: move |_| js::pick_file("banner-file"),
                            if uploading() == Some("banner") { "Envoi…" } else { "Changer la bannière" }
                        }
                        if shown_banner.is_some() {
                            button { class: "link", onclick: move |_| { new_banner.set(None); drop_banner.set(true); }, "Retirer" }
                        }
                    }
                }

                section { class: "edit-section",
                    h3 { "Nom affiché" }
                    input { class: "field", maxlength: "40", placeholder: "Nom affiché", value: "{name}",
                        oninput: move |e| name.set(e.value()) }
                    if !name_ok {
                        div { class: "field-error", "Nom affiché : 1 à 40 caractères." }
                    }
                }

                section { class: "edit-section",
                    h3 { "Bio" }
                    textarea { class: "field bio-field", rows: "4", placeholder: "Quelques mots sur vous…", value: "{bio}",
                        oninput: move |e| bio.set(e.value()) }
                    div { class: if bio_ok { "counter" } else { "counter over" },
                        "{bio_chars} / {limits::BIO_CHARS}"
                        if bio_lines > limits::BIO_LINES { " · {limits::BIO_LINES} lignes au plus" }
                    }
                }

                section { class: "edit-section",
                    h3 { "Police du nom" }
                    div { class: "font-grid",
                        button { class: if font().is_empty() { "font-choice on" } else { "font-choice" }, onclick: move |_| font.set(String::new()),
                            span { class: "name", "{preview_user.display_name}" }
                            small { "Police système" }
                        }
                        for f in FONTS {
                            button { key: "{f.id}", class: if font() == f.id { "font-choice on" } else { "font-choice" },
                                onclick: move |_| font.set(f.id.to_string()),
                                span { class: "name font-{f.id}", "{preview_user.display_name}" }
                                small { "{f.name}" }
                            }
                        }
                    }
                }

                section { class: "edit-section",
                    h3 { "Couleur du nom" }
                    div { class: "swatches",
                        button { class: if color().is_empty() { "swatch default on" } else { "swatch default" }, title: "Par défaut",
                            onclick: move |_| color.set(String::new()), "Aa" }
                        for c in NAME_COLORS {
                            button { key: "{c}", class: if color() == c { "swatch on" } else { "swatch" }, title: "{c}",
                                style: "background: {c}", onclick: move |_| color.set(c.to_string()) }
                        }
                        label { class: if !color().is_empty() && !NAME_COLORS.contains(&color().as_str()) { "swatch custom on" } else { "swatch custom" },
                            title: "Autre couleur",
                            input { r#type: "color", value: "{picker_value}", oninput: move |e| color.set(e.value().to_lowercase()) }
                        }
                    }
                    div { class: "hint",
                        if color().is_empty() { "Par défaut" } else { "{color}" }
                        if adjusted { " · ajustée pour rester lisible sur votre thème" }
                    }
                }

                button { class: "primary", disabled: saving() || uploading().is_some() || !bio_ok || !name_ok, onclick: save,
                    if saving() { "Enregistrement…" } else { "Enregistrer" }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------
// Viewer

/// Full-screen viewer of the photos and videos exchanged with a user (`Overlay::Media`):
/// arrows or swipe to move, Escape / back to close.
#[component]
pub fn MediaViewer(user_id: i64, message_id: i64) -> Element {
    let ctx = use_context::<Ctx>();
    let mut current = use_signal(|| message_id);
    let mut start = use_signal(|| None::<f64>);
    use_hook(|| js::focus_input("media-viewer"));
    let shared = SHARED.read().clone();
    let items = if shared.user_id == user_id { shared.items } else { vec![] };
    let index = items.iter().position(|i| i.message_id == current());
    let total = items.len();

    let mut go = move |delta: i64| {
        let (target, near_end, more) = {
            let shared = SHARED.peek();
            let Some(pos) = shared.items.iter().position(|i| i.message_id == *current.peek()) else { return };
            let target = pos as i64 + delta;
            if target < 0 || target as usize >= shared.items.len() {
                return;
            }
            let target = target as usize;
            (shared.items[target].message_id, target + 3 >= shared.items.len(), shared.next.is_some())
        };
        current.set(target);
        if near_end && more {
            spawn(load_photos(ctx, user_id, false));
        }
    };

    let item = index.map(|i| items[i].clone());
    let me = ctx.my_id();
    let sender = item.as_ref().map(|i| {
        if i.sender_id == me {
            "Vous".to_string()
        } else {
            ctx.friend(i.sender_id).map(|f| f.user.display_name).unwrap_or_default()
        }
    });
    rsx! {
        div { class: "media-viewer", id: "media-viewer", tabindex: "0",
            onkeydown: move |e| match e.key() {
                Key::ArrowLeft => go(-1),
                Key::ArrowRight => go(1),
                _ => {}
            },
            onpointerdown: move |e| start.set(Some(e.client_coordinates().x)),
            onpointerup: move |e| {
                if let Some(x) = start() {
                    let dx = e.client_coordinates().x - x;
                    if dx > 60.0 {
                        go(-1);
                    } else if dx < -60.0 {
                        go(1);
                    }
                }
                start.set(None);
            },
            div { class: "media-top",
                button { class: "icon", title: "Fermer", onclick: move |_| ctx.close_overlay(), Icon { name: "back" } }
                div { class: "media-info",
                    if let (Some(item), Some(sender)) = (&item, &sender) {
                        b { "{sender}" }
                        span { "{ui::long_day(item.created_at)} · {ui::time(item.created_at)}" }
                    }
                }
                if let Some(i) = index {
                    span { class: "media-count", "{i + 1} / {total}" }
                }
            }
            div { class: "media-stage",
                if let Some(item) = item.clone() {
                    if is_video(&item) {
                        video { key: "{item.message_id}", src: api::media_url(&item.media.id), controls: true, autoplay: true, playsinline: true }
                    } else {
                        img { key: "{item.message_id}", src: api::media_url(&item.media.id), alt: "", draggable: "false" }
                    }
                } else {
                    div { class: "hint", "Chargement…" }
                }
            }
            if index.is_some_and(|i| i > 0) {
                button { class: "media-nav prev", title: "Précédente", onclick: move |e| { e.stop_propagation(); go(-1) },
                    Icon { name: "back" } }
            }
            if index.is_some_and(|i| i + 1 < total) {
                button { class: "media-nav next", title: "Suivante", onclick: move |e| { e.stop_propagation(); go(1) },
                    Icon { name: "chevron" } }
            }
        }
    }
}
