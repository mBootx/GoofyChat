//! Online presence, in memory. A socket counts once it sends `active: true` and until it
//! sends `active: false` or closes. A user is online while one of their sockets counts;
//! friends hear `presence(true)` at once and `presence(false)` only after
//! `AppState::offline_grace` without any active socket.

use crate::{friends, state::AppState};
use proto::ServerEvent;

pub fn is_online(state: &AppState, user: i64) -> bool {
    state.presence.lock().unwrap().online.contains(&user)
}

async fn announce(state: &AppState, user: i64, online: bool) {
    match friends::friend_ids(state, user).await {
        Ok(to) => state.send(&to, &ServerEvent::Presence { user_id: user, online }),
        Err(e) => tracing::warn!("presence: {}", e.1),
    }
}

/// One more active socket for the user.
pub async fn activate(state: &AppState, user: i64) {
    let came_online = {
        let mut p = state.presence.lock().unwrap();
        *p.active.entry(user).or_default() += 1;
        // Within the grace delay the user never looked offline to friends.
        p.online.insert(user)
    };
    if came_online {
        announce(state, user, true).await;
    }
}

/// One active socket fewer. The last one starts the grace delay.
pub fn deactivate(state: &AppState, user: i64) {
    let epoch = {
        let mut p = state.presence.lock().unwrap();
        let Some(count) = p.active.get_mut(&user) else { return };
        *count = count.saturating_sub(1);
        if *count > 0 {
            return;
        }
        p.active.remove(&user);
        let epoch = p.epoch.entry(user).or_default();
        *epoch += 1;
        *epoch
    };
    let state = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(state.offline_grace).await;
        let went_offline = {
            let mut p = state.presence.lock().unwrap();
            let current = p.epoch.get(&user).copied() == Some(epoch) && !p.active.contains_key(&user);
            current && p.online.remove(&user)
        };
        if went_offline {
            announce(&state, user, false).await;
        }
    });
}

/// Forgets a deleted account at once.
pub fn forget(state: &AppState, user: i64) {
    let mut p = state.presence.lock().unwrap();
    p.active.remove(&user);
    p.online.remove(&user);
    *p.epoch.entry(user).or_default() += 1;
}
