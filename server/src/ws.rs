use crate::{
    api::member_ids,
    auth::user_for_token,
    state::{AppState, PendingCall, RING_TIMEOUT},
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use proto::{CallAction, CallSignal, ClientEvent, ServerEvent, User};
use std::time::{Duration, Instant};
use tokio::{sync::mpsc, time::timeout};

pub async fn handler(State(state): State<AppState>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(64 * 1024)
        .max_frame_size(64 * 1024)
        .on_upgrade(move |socket| run(state, socket))
}

fn parse(frame: Message) -> Option<ClientEvent> {
    match frame {
        Message::Text(text) => serde_json::from_str(&text).ok(),
        _ => None,
    }
}

async fn push(socket: &mut WebSocket, event: &ServerEvent) -> bool {
    let Ok(text) = serde_json::to_string(event) else {
        return false;
    };
    let sent = timeout(Duration::from_secs(15), socket.send(Message::Text(text.into()))).await;
    matches!(sent, Ok(Ok(())))
}

const SILENCE: Duration = Duration::from_secs(90);

async fn run(state: AppState, mut socket: WebSocket) {
    // The token travels in the first frame so it never appears in proxy logs.
    let Ok(Some(Ok(first))) = timeout(Duration::from_secs(10), socket.recv()).await else {
        return;
    };
    let Some(ClientEvent::Auth { token }) = parse(first) else {
        return;
    };
    let Ok(user) = user_for_token(&state, &token).await else {
        return;
    };

    let (tx, mut rx) = mpsc::channel::<ServerEvent>(64);
    {
        let mut sockets = state.sockets.lock().unwrap();
        let list = sockets.entry(user.id).or_default();
        list.retain(|t| !t.is_closed());
        // Bound the number of devices per account.
        if list.len() >= 8 {
            list.remove(0);
        }
        list.push(tx.clone());
    }
    if !push(&mut socket, &ServerEvent::Ready).await {
        return;
    }
    let ringing: Vec<ServerEvent> = state
        .calls
        .lock()
        .unwrap()
        .values()
        .filter(|c| c.to == user.id && c.at.elapsed() < RING_TIMEOUT)
        .map(|c| ServerEvent::Call { from: c.from.clone(), signal: c.signal.clone() })
        .collect();
    for event in ringing {
        let _ = tx.try_send(event);
    }
    // The map now holds the only sender: removing it there closes this socket.
    drop(tx);

    // Neither deadline is reset by outgoing events.
    let mut deadline = tokio::time::Instant::now() + SILENCE;
    let mut recheck = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            outgoing = rx.recv() => {
                let Some(event) = outgoing else { break };
                if !push(&mut socket, &event).await { break }
            }
            _ = recheck.tick() => {
                // A deleted or expired session closes the socket.
                if user_for_token(&state, &token).await.is_err() { break }
            }
            _ = tokio::time::sleep_until(deadline) => break,
            incoming = socket.recv() => {
                let Some(Ok(frame)) = incoming else { break };
                deadline = tokio::time::Instant::now() + SILENCE;
                if matches!(frame, Message::Close(_)) { break }
                match parse(frame) {
                    Some(ClientEvent::Ping) => {
                        if !push(&mut socket, &ServerEvent::Pong).await { break }
                    }
                    Some(ClientEvent::Call { signal }) => relay(&state, &user, signal).await,
                    _ => {}
                }
            }
        }
    }
    drop(rx);
    if let Some(list) = state.sockets.lock().unwrap().get_mut(&user.id) {
        list.retain(|t| !t.is_closed());
    }
}

/// Forwards call signalling to the other member of a one-to-one conversation.
async fn relay(state: &AppState, from: &User, signal: CallSignal) {
    if signal.call_id.len() > 64
        || !state.allow(from.id.to_string(), "signal", 120, Duration::from_secs(10))
    {
        return;
    }
    let Ok(members) = member_ids(state, signal.conversation_id).await else {
        return;
    };
    if members.len() != 2 || !members.contains(&from.id) {
        return;
    }
    let Some(&to) = members.iter().find(|&&id| id != from.id) else {
        return;
    };
    {
        let mut calls = state.calls.lock().unwrap();
        calls.retain(|_, c| c.at.elapsed() < RING_TIMEOUT);
        match signal.action {
            CallAction::Invite => {
                // One ringing call per caller.
                calls.retain(|_, c| c.from.id != from.id);
                calls.insert(
                    signal.call_id.clone(),
                    PendingCall { from: from.clone(), to, signal: signal.clone(), at: Instant::now() },
                );
            }
            CallAction::Accept | CallAction::Reject | CallAction::Hangup => {
                if calls
                    .get(&signal.call_id)
                    .is_some_and(|c| c.from.id == from.id || c.to == from.id)
                {
                    calls.remove(&signal.call_id);
                }
            }
            _ => {}
        }
    }
    let answered = matches!(signal.action, CallAction::Accept | CallAction::Reject);
    let event = ServerEvent::Call { from: from.clone(), signal };
    state.send(&[to], &event);
    // Tell the user's other devices to stop ringing.
    if answered {
        state.send(&[from.id], &event);
    }
}
