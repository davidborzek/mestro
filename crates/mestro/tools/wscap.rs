#![allow(clippy::unwrap_used, clippy::expect_used)]
// developer tools: a panic is an acceptable way for a one-off
// CLI tool to fail

//! Debug capture: replicate the app's MA session (auth -> players/all
//! -> player_queues/get_active_queue) and dump every WS frame with
//! timestamps so protocol assumptions can be verified against reality.
//!
//! Usage: wscap [seconds]  (env: MESTRO_MA_URL, MESTRO_TOKEN, MESTRO_PLAYER)

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::MaybeTlsStream;
use tokio_tungstenite::WebSocketStream;

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

static START: OnceLock<Instant> = OnceLock::new();

fn ts() -> f64 {
    START
        .get()
        .map(|t| t.elapsed().as_secs_f64())
        .unwrap_or(0.0)
}

async fn tx(msg: Value, ws: &mut Ws) {
    // never log the auth token
    let mut loggable = msg.clone();
    if loggable.get("command").and_then(|c| c.as_str()) == Some("auth") {
        if let Some(args) = loggable.get_mut("args").and_then(|a| a.as_object_mut()) {
            args.insert("token".into(), serde_json::json!("<redacted>"));
        }
    }
    eprintln!(
        "[{:>8.3}] TX {}",
        ts(),
        serde_json::to_string_pretty(&loggable).unwrap()
    );
    let _ = ws.send(Message::Text(msg.to_string().into())).await;
}

fn rx(v: &Value) {
    eprintln!(
        "[{:>8.3}] RX {}",
        ts(),
        serde_json::to_string_pretty(v).unwrap()
    );
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = START.set(Instant::now());
    let secs: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);
    let ws_url = std::env::var("MESTRO_MA_URL")
        .unwrap_or_else(|_| "ws://ma.example.com:8095/ws".to_string());
    let token = std::env::var("MESTRO_TOKEN").expect("MESTRO_TOKEN required");
    let want_player = std::env::var("MESTRO_PLAYER").unwrap_or_default();

    let (mut ws, _resp) = tokio_tungstenite::connect_async(&ws_url).await?;
    eprintln!("# connected {ws_url}");

    // first message must be auth (message_id like the app sends it)
    tx(
        json!({ "message_id": "1", "command": "auth", "args": { "token": token } }),
        &mut ws,
    )
    .await;
    tx(
        json!({ "message_id": "2", "command": "players/all", "args": {} }),
        &mut ws,
    )
    .await;

    let mut msg_id = 3u64;
    let mut active_queue_sent = false;
    let mut played = false;
    let play_at = tokio::time::Instant::now() + Duration::from_secs(1);
    let next_at = tokio::time::Instant::now() + Duration::from_secs(10);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(secs);

    loop {
        // scripted actions: two browse rounds after the queue
        // reply — MESTRO_BROWSE and MESTRO_BROWSE2 carry the paths
        // ("root" = the root level). Maps the browse tree one
        // level per run.
        if active_queue_sent && !played && tokio::time::Instant::now() >= play_at {
            let path = std::env::var("MESTRO_BROWSE").unwrap_or_else(|_| "root".into());
            tx(
                json!({
                    "message_id": msg_id.to_string(),
                    "command": "music/browse",
                    "args": { "path": path }
                }),
                &mut ws,
            )
            .await;
            msg_id += 1;
            played = true;
        }
        if played && tokio::time::Instant::now() >= next_at && msg_id == 5 {
            if let Ok(path) = std::env::var("MESTRO_BROWSE2") {
                tx(
                    json!({
                        "message_id": msg_id.to_string(),
                        "command": "music/browse",
                        "args": { "path": path }
                    }),
                    &mut ws,
                )
                .await;
                msg_id += 1;
            }
        }
        // next wake-up: the earliest pending action or the deadline
        let mut wake = deadline;
        if !played && play_at < wake {
            wake = play_at;
        }
        if played && msg_id == 5 && next_at < wake {
            wake = next_at;
        }
        let msg = tokio::time::timeout_at(wake, ws.next()).await;
        let msg = match msg {
            Ok(Some(m)) => m?,
            Ok(None) => break,
            Err(_) => {
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
                continue;
            }
        };
        let Message::Text(txt) = msg else { continue };
        let Ok(v) = serde_json::from_str::<Value>(&txt) else {
            eprintln!("[{:>8.3}] RX <non-json> {txt}", ts());
            continue;
        };
        rx(&v);

        // pick a player from the players/all reply and ask for its
        // queue, mirroring the app flow
        if !active_queue_sent {
            if let Some(result) = v.get("result").and_then(|r| r.as_array()) {
                let pick = result
                    .iter()
                    .find(|p| {
                        let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("");
                        !want_player.is_empty() && name == want_player
                    })
                    .or_else(|| {
                        result.iter().find(|p| {
                            p.get("playback_state").and_then(|s| s.as_str()) == Some("playing")
                        })
                    })
                    .or_else(|| result.first());
                if let Some(pick) = pick {
                    let pid = pick
                        .get("player_id")
                        .and_then(|x| x.as_str())
                        .unwrap_or_default();
                    eprintln!("# picked player {pid:?}");
                    tx(
                        json!({
                            "message_id": msg_id.to_string(),
                            "command": "player_queues/get_active_queue",
                            "args": { "player_id": pid }
                        }),
                        &mut ws,
                    )
                    .await;
                    msg_id += 1;
                    active_queue_sent = true;
                }
            }
        }
    }
    eprintln!("# capture done");
    Ok(())
}
