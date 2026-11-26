//! Localhost WebSocket bridge for the Spicetify extension (`spicetify-ext/`).
//! The extension connects to ws://127.0.0.1:5724 and:
//!   - pushes {type:"position", spotify_id, position_ms, is_playing, title, ...} —
//!     becomes the clock's freshest source AND the capture metadata source;
//!   - asks {type:"request_marks", spotify_id} — we reply {type:"marks", ...};
//!   - sends {type:"capture"} from the in-Spotify button — triggers a capture.
//! The app pushes BACK to the extension via `broadcast()`:
//!   - {type:"pause"} / {type:"resume"} — control Spotify around a capture;
//!   - {type:"refresh_marks"} — tells the extension to re-fetch marks (after a save).

use crate::state::AppState;
use futures_util::{SinkExt, StreamExt};
use std::sync::OnceLock;
use tauri::{AppHandle, Manager};
use tokio::sync::broadcast;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::{header::ORIGIN, StatusCode};

pub const BRIDGE_ADDR: &str = "127.0.0.1:5724";

// Outbound channel: app → all connected extensions.
static OUTBOUND: OnceLock<broadcast::Sender<String>> = OnceLock::new();

/// Push a message to every connected extension (no-op if none / bridge not up).
pub fn broadcast(msg: serde_json::Value) {
    if let Some(tx) = OUTBOUND.get() {
        let _ = tx.send(msg.to_string());
    }
}

pub fn start(app: AppHandle) {
    let (tx, _rx) = broadcast::channel::<String>(32);
    let _ = OUTBOUND.set(tx);
    tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::bind(BRIDGE_ADDR).await {
            Ok(l) => l,
            Err(e) => {
                log::error!("Samplify bridge failed to bind {BRIDGE_ADDR}: {e}");
                return;
            }
        };
        log::info!("Samplify bridge listening on ws://{BRIDGE_ADDR}");
        let mut heartbeat = tokio::time::interval(std::time::Duration::from_secs(1));
        heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            // Spotify's embedded browser throttles JavaScript intervals while
            // its window is in the background. A native tick keeps the clock
            // advancing even when the user is working in another app.
            let connection = tokio::select! {
                _ = heartbeat.tick() => {
                    broadcast(serde_json::json!({"type": "request_position"}));
                    continue;
                }
                connection = listener.accept() => connection,
            };
            match connection {
                Ok((stream, _)) => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(handle_conn(app, stream));
                }
                Err(e) => log::error!("bridge accept error: {e}"),
            }
        }
    });
}

fn check_client(req: &Request, response: Response) -> Result<Response, ErrorResponse> {
    // Native local clients may omit Origin. Browsers supply it on every upgrade.
    let mut origins = req.headers().get_all(ORIGIN).iter();
    if let Some(origin) = origins.next() {
        if origin != "https://xpui.app.spotify.com" || origins.next().is_some() {
            let mut denied = ErrorResponse::new(Some("Unsupported browser origin".into()));
            *denied.status_mut() = StatusCode::FORBIDDEN;
            return Err(denied);
        }
    }
    Ok(response)
}

async fn handle_conn(app: AppHandle, stream: tokio::net::TcpStream) {
    let ws = match tokio_tungstenite::accept_hdr_async(stream, check_client).await {
        Ok(ws) => ws,
        Err(_) => return,
    };
    let (mut write, mut read) = ws.split();
    let mut rx = OUTBOUND.get().unwrap().subscribe();

    loop {
        tokio::select! {
            incoming = read.next() => {
                match incoming {
                    Some(Ok(msg)) => {
                        if handle_incoming(&app, &mut write, msg).await.is_err() { break; }
                    }
                    _ => break,
                }
            }
            outbound = rx.recv() => {
                match outbound {
                    Ok(txt) => {
                        if write.send(Message::Text(txt.into())).await.is_err() { break; }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(_) => break,
                }
            }
        }
    }
}

type WsWrite = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    Message,
>;

#[cfg(test)]
mod tests {
    use super::*;

    fn accepted(origins: &[&str]) -> bool {
        let mut req = Request::new(());
        for origin in origins {
            req.headers_mut().append("origin", origin.parse().unwrap());
        }
        check_client(&req, Response::new(())).is_ok()
    }

    #[test]
    fn spotify_desktop_and_native_clients_can_connect() {
        assert!(accepted(&["https://xpui.app.spotify.com"]));
        assert!(accepted(&[]));
    }

    #[test]
    fn reject_other_browser_origins_and_ambiguous_headers() {
        assert!(!accepted(&["https://example.org"]));
        assert!(!accepted(&["null"]));
        assert!(!accepted(&["https://xpui.app.spotify.com", "https://example.org"]));
    }
}

async fn handle_incoming(app: &AppHandle, write: &mut WsWrite, msg: Message) -> Result<(), ()> {
    let txt: String = match msg {
        Message::Text(t) => t.to_string(),
        Message::Close(_) => return Err(()),
        _ => return Ok(()),
    };
    let v: serde_json::Value = match serde_json::from_str(&txt) {
        Ok(v) => v,
        Err(_) => return Ok(()),
    };

    match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
        "position" => {
            let sid = v.get("spotify_id").and_then(|s| s.as_str()).unwrap_or("").to_string();
            let pos = v.get("position_ms").and_then(|p| p.as_i64()).unwrap_or(0);
            let playing = v.get("is_playing").and_then(|b| b.as_bool()).unwrap_or(false);
            if !sid.is_empty() {
                let state = app.state::<AppState>();
                let now = chrono::Utc::now().timestamp_millis();
                #[cfg(windows)]
                state.desktop_audio.spotify.lock().unwrap().update(&sid, pos, playing, std::time::Instant::now());
                state.clock.lock().unwrap().update(crate::clock::Sample {
                    track_ms: pos,
                    is_playing: playing,
                    received_at_ms: now,
                    spotify_id: sid.clone(),
                });
                let title = v.get("title").and_then(|s| s.as_str()).unwrap_or("").to_string();
                if !title.is_empty() {
                    *state.bridge_track.lock().unwrap() = Some((std::time::Instant::now(), crate::spotify::NowPlaying {
                        is_track: true,
                        spotify_id: sid,
                        title,
                        artist: v.get("artist").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                        album: v.get("album").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                        album_art_url: v.get("album_art_url").and_then(|s| s.as_str()).map(|s| s.to_string()),
                        duration_ms: v.get("duration_ms").and_then(|d| d.as_i64()).unwrap_or(0),
                        progress_ms: pos,
                        is_playing: playing,
                    }));
                }
            } else {
                let state = app.state::<AppState>();
                *state.bridge_track.lock().unwrap() = None;
                #[cfg(windows)]
                state.desktop_audio.spotify.lock().unwrap().forget_track();
            }
        }
        "request_marks" => {
            let sid = v.get("spotify_id").and_then(|s| s.as_str()).unwrap_or("").to_string();
            let marks = {
                let state = app.state::<AppState>();
                crate::store::marks_for_spotify(&state.store, &sid).unwrap_or_default()
            };
            let reply = serde_json::json!({ "type": "marks", "spotify_id": sid, "marks": marks });
            if write.send(Message::Text(reply.to_string().into())).await.is_err() {
                return Err(());
            }
        }
        "capture" => {
            let result = crate::commands::trigger_capture(app.clone(), true).await;
            if let Err(error) = &result {
                use tauri_plugin_notification::NotificationExt;
                let _ = app.notification().builder().title("Spotify capture").body(error).show();
            }
            let reply = serde_json::json!({"type":"capture_result", "error":result.err()});
            write.send(Message::Text(reply.to_string().into())).await.map_err(|_| ())?;
        }
        "ping" => {
            if write.send(Message::Text("{\"type\":\"pong\"}".to_string().into())).await.is_err() {
                return Err(());
            }
        }
        _ => {}
    }
    Ok(())
}
