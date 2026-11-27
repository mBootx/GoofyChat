//! Call UI: Discord-like desktop stage with participant tiles, speaking indicators, and
//! screen sharing, plus mobile/ringing call screen (contract 0.4.0 §9).

use crate::{api, js, ui, Ctx};
use dioxus::prelude::*;
use serde::Deserialize;

#[derive(Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CallView {
    pub state: String,
    pub video: bool,
    pub peer: String,
    pub muted: bool,
    pub camera_off: bool,
    pub message: String,
    /// Media id of the other person's profile photo.
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub peer_id: i64,
    #[serde(default)]
    pub screen_local: bool,
    #[serde(default)]
    pub screen_remote: bool,
    #[serde(default)]
    pub can_share: bool,
    #[serde(default)]
    pub speaking_local: bool,
    #[serde(default)]
    pub speaking_remote: bool,
    #[serde(default)]
    pub remote_camera: bool,
    #[serde(default)]
    pub since: i64,
}

fn tile_tint(name: &str) -> String {
    let hue = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 360;
    format!("background: linear-gradient(180deg, hsl({hue} 28% 20%), hsl({hue} 22% 13%))")
}

#[component]
pub fn CallOverlay(call: Signal<CallView>, #[props(default)] wide: bool) -> Element {
    let ctx = use_context::<Ctx>();
    let mut fullscreen = use_signal(|| false);

    // Re-attach active MediaStreams whenever call state or wide layout changes DOM video nodes.
    use_effect(move || {
        let _ = *ctx.wide.read();
        let c = call.read();
        let active = !c.state.is_empty() && c.state != "idle";
        if !active && *fullscreen.peek() {
            fullscreen.set(false);
        }
        let _ = (&c.state, c.screen_local, c.screen_remote, c.remote_camera, c.camera_off);
        js::sync_call_media();
    });

    let call_view = call.read().clone();
    let active = !call_view.state.is_empty() && call_view.state != "idle";
    let connected = call_view.state == "connected";
    let is_stage = active && wide && connected;

    let me = ctx.session.read().as_ref().map(|s| s.user.clone());
    let me_name = me.as_ref().map(|u| u.display_name.clone()).unwrap_or_else(|| "Vous".to_string());
    let me_photo = me.as_ref().and_then(|u| u.avatar.clone());

    let peer_photo = call_view.avatar.clone().filter(|a| !a.is_empty());
    let show_local_cam = call_view.video && !call_view.camera_off;
    let show_remote_cam = call_view.remote_camera;
    let has_screen = call_view.screen_remote || call_view.screen_local;
    let focus_remote_screen = call_view.screen_remote;

    let toggle_share = move |_| {
        if call.peek().screen_local {
            js::stop_screen_share();
        } else {
            spawn(async move {
                if let Err(e) = js::start_screen_share().await {
                    let msg = e.as_string().unwrap_or_else(|| "Partage d'écran impossible.".into());
                    if !msg.is_empty() {
                        ctx.notify(msg);
                    }
                }
            });
        }
    };

    let toggle_fs = move |_| {
        js::toggle_fullscreen("call-stage");
        fullscreen.set(!fullscreen());
    };
    let call = call_view;

    // Desktop connected stage (covers the main pane only).
    if is_stage {
        let kind = if call.video { "Appel vidéo" } else { "Appel vocal" };
        let peer_tile_class = ui::pick(call.speaking_remote, "call-tile speaking", "call-tile");
        let self_tile_class = ui::pick(call.speaking_local, "call-tile speaking", "call-tile");
        let peer_tint = tile_tint(&call.peer);
        let me_tint = tile_tint(&me_name);

        let peer_tile = rsx! {
            div { class: peer_tile_class, style: "{peer_tint}",
                video {
                    id: "remote-video",
                    class: ui::pick(show_remote_cam, "", "media-hidden"),
                    autoplay: true,
                    playsinline: true,
                }
                if !show_remote_cam {
                    div { class: "call-tile-avatar",
                        ui::Avatar { name: call.peer.clone(), photo: peer_photo.clone(), animate: true }
                    }
                }
                div { class: "call-tile-label", "{call.peer}" }
            }
        };

        let self_tile = rsx! {
            div { class: self_tile_class, style: "{me_tint}",
                video {
                    id: "local-video",
                    class: ui::pick(show_local_cam, "mirror", "media-hidden"),
                    autoplay: true,
                    playsinline: true,
                    muted: true,
                }
                if !show_local_cam {
                    div { class: "call-tile-avatar",
                        ui::Avatar { name: me_name.clone(), photo: me_photo.clone(), animate: true }
                    }
                }
                div { class: "call-tile-label",
                    if call.muted {
                        ui::Icon { name: "micoff" }
                    }
                    "{me_name}"
                }
            }
        };

        return rsx! {
            div { id: "call-stage", class: "call stage", onmounted: move |_| js::sync_call_media(),
                div { class: "call-top",
                    div { class: "call-meta",
                        h2 { "{call.peer}" }
                        span { class: "call-badge", "{kind}" }
                        span { class: "call-timer", id: "call-timer", "data-t": "0:00" }
                    }
                }

                if has_screen {
                    div { class: "call-grid with-screen",
                        div {
                            class: "call-focus",
                            title: "Double-clic pour le plein écran",
                            ondoubleclick: toggle_fs,
                            video {
                                id: "remote-screen",
                                class: ui::pick(focus_remote_screen, "", "media-hidden"),
                                autoplay: true,
                                playsinline: true,
                            }
                            video {
                                id: "local-screen",
                                class: ui::pick(!focus_remote_screen && call.screen_local, "", "media-hidden"),
                                autoplay: true,
                                playsinline: true,
                                muted: true,
                            }
                            div { class: "call-screen-pill",
                                ui::Icon { name: "screen" }
                                if focus_remote_screen {
                                    "Écran de {call.peer}"
                                } else {
                                    "Vous partagez votre écran"
                                }
                            }
                        }
                        div { class: "call-strip",
                            {peer_tile}
                            {self_tile}
                        }
                    }
                } else {
                    video { id: "remote-screen", class: "media-hidden", autoplay: true, playsinline: true }
                    video { id: "local-screen", class: "media-hidden", autoplay: true, playsinline: true, muted: true }
                    div { class: "call-grid two",
                        {peer_tile}
                        {self_tile}
                    }
                }

                div { class: "call-dock",
                    div { class: "call-bar",
                        button {
                            class: ui::pick(call.muted, "round on", "round"),
                            title: ui::pick(call.muted, "Activer le micro", "Couper le micro"),
                            onclick: move |_| js::toggle_mute(),
                            ui::Icon { name: ui::pick(call.muted, "micoff", "mic") }
                        }
                        if call.video {
                            button {
                                class: ui::pick(call.camera_off, "round on", "round"),
                                title: ui::pick(call.camera_off, "Activer la caméra", "Couper la caméra"),
                                onclick: move |_| js::toggle_camera(),
                                ui::Icon { name: ui::pick(call.camera_off, "videooff", "video") }
                            }
                        }
                        if call.can_share {
                            button {
                                class: ui::pick(call.screen_local, "round accent", "round"),
                                title: ui::pick(call.screen_local, "Arrêter le partage", "Partager l'écran"),
                                onclick: toggle_share,
                                ui::Icon { name: ui::pick(call.screen_local, "screenoff", "screen") }
                            }
                        }
                        button {
                            class: "round",
                            title: ui::pick(fullscreen(), "Quitter le plein écran", "Plein écran"),
                            onclick: toggle_fs,
                            ui::Icon { name: ui::pick(fullscreen(), "shrink", "expand") }
                        }
                        button {
                            class: "round red",
                            title: "Raccrocher",
                            onclick: move |_| js::hangup(),
                            ui::Icon { name: "hangup" }
                        }
                    }
                }
            }
        };
    }

    // Ringing / connecting (both layouts) or mobile connected layout.
    let portrait = active && (!connected || (!call.video && !call.screen_remote));
    let mut class = if !active {
        String::from("call hidden")
    } else if portrait {
        String::from("call portrait")
    } else {
        String::from("call mobile-video")
    };
    if wide && active {
        class.push_str(" large");
    }

    let kind = if call.video { "Appel vidéo" } else { "Appel vocal" };
    let (headline, status) = match call.state.as_str() {
        "incoming" => (format!("{} vous appelle…", call.peer), kind.to_string()),
        "calling" => (call.peer.clone(), format!("{kind} · ça sonne…")),
        "connecting" => (call.peer.clone(), "Connexion…".to_string()),
        _ => (call.peer.clone(), String::new()),
    };
    let backdrop = peer_photo.as_ref().map(|id| format!("background-image: url('{}')", api::media_url(id)));
    let face_class = if call.state == "incoming" || call.state == "calling" {
        "call-face ringing"
    } else if call.speaking_remote {
        "call-face speaking"
    } else {
        "call-face"
    };

    let show_mobile_remote_video = active && connected && call.video && !call.screen_remote;
    let show_mobile_remote_screen = active && connected && call.screen_remote;
    let show_mobile_local_video = active && connected && call.video && !call.camera_off;

    rsx! {
        div { class,
            if portrait {
                div {
                    class: "call-backdrop",
                    style: backdrop.clone().unwrap_or_default(),
                    "data-empty": "{backdrop.is_none()}",
                }
            }
            video {
                id: "remote-video",
                class: ui::pick(show_mobile_remote_video, "", "media-hidden"),
                autoplay: true,
                playsinline: true,
            }
            video {
                id: "remote-screen",
                class: ui::pick(show_mobile_remote_screen, "", "media-hidden"),
                autoplay: true,
                playsinline: true,
            }
            video {
                id: "local-video",
                class: ui::pick(show_mobile_local_video, "", "media-hidden"),
                autoplay: true,
                playsinline: true,
                muted: true,
            }
            video { id: "local-screen", class: "media-hidden", autoplay: true, playsinline: true, muted: true }

            if show_mobile_remote_screen {
                div { class: "call-screen-pill",
                    ui::Icon { name: "screen" }
                    "Écran partagé"
                }
            }

            div { class: "who",
                if portrait {
                    div { class: face_class,
                        ui::Avatar { name: call.peer.clone(), photo: peer_photo, animate: true }
                    }
                }
                h2 { "{headline}" }
                if connected {
                    div { class: "call-timer", id: "call-timer", "data-t": "0:00" }
                } else {
                    div { class: "call-status", "{status}" }
                }
            }

            div { class: "buttons",
                if call.state == "incoming" {
                    div { class: "answer",
                        button { class: "round red", title: "Refuser", onclick: move |_| js::reject_call(),
                            ui::Icon { name: "hangup" }
                        }
                        span { "Refuser" }
                    }
                    div { class: "answer",
                        button { class: "round green pulse", title: "Répondre", onclick: move |_| js::accept_call(),
                            ui::Icon { name: ui::pick(call.video, "video", "phone") }
                        }
                        span { "Répondre" }
                    }
                } else {
                    button {
                        class: ui::pick(call.muted, "round on", "round"),
                        title: ui::pick(call.muted, "Activer le micro", "Couper le micro"),
                        onclick: move |_| js::toggle_mute(),
                        ui::Icon { name: ui::pick(call.muted, "micoff", "mic") }
                    }
                    button { class: "round red", title: "Raccrocher", onclick: move |_| js::hangup(),
                        ui::Icon { name: "hangup" }
                    }
                    if call.video {
                        button {
                            class: ui::pick(call.camera_off, "round on", "round"),
                            title: ui::pick(call.camera_off, "Activer la caméra", "Couper la caméra"),
                            onclick: move |_| js::toggle_camera(),
                            ui::Icon { name: ui::pick(call.camera_off, "videooff", "video") }
                        }
                    }
                }
            }
        }
    }
}
