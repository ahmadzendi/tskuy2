use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::config::*;
use crate::state::{AppState, GoldEntry};

#[derive(serde::Deserialize)]
struct PusherMessage {
    event: Option<String>,
    data: Option<serde_json::Value>,
    #[allow(dead_code)]
    channel: Option<String>,
}

#[derive(serde::Deserialize)]
struct GoldRateData {
    buying_rate: Option<serde_json::Value>,
    selling_rate: Option<serde_json::Value>,
    created_at: Option<String>,
}

fn parse_number(v: &serde_json::Value) -> Option<i64> {
    match v {
        serde_json::Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)),
        serde_json::Value::String(s) => s.replace('.', "").replace(',', "").parse().ok(),
        _ => None,
    }
}

async fn process_data(state: &Arc<AppState>, data: GoldRateData) {
    let buy = match data.buying_rate.as_ref().and_then(parse_number) {
        Some(v) => v,
        None => return,
    };
    let sell = match data.selling_rate.as_ref().and_then(parse_number) {
        Some(v) => v,
        None => return,
    };
    let created_at = match data.created_at {
        Some(ref s) if !s.is_empty() => s.clone(),
        _ => return,
    };

    {
        let mut shown = state.shown_updates.lock();
        if shown.contains(&created_at) {
            return;
        }
        shown.insert(created_at.clone());
        if shown.len() > 5000 {
            let keep = created_at.clone();
            shown.clear();
            shown.insert(keep);
        }
    }

    let has_last = state.has_last_buy.load(Ordering::Relaxed);
    let last = state.last_buy.load(Ordering::Relaxed);

    let (status, diff) = if !has_last {
        ("➖".into(), 0i64)
    } else if buy > last {
        ("🚀".into(), buy - last)
    } else if buy < last {
        ("🔻".into(), buy - last)
    } else {
        ("➖".into(), 0i64)
    };

    {
        let mut history = state.history.write();
        if history.len() >= MAX_HISTORY {
            history.pop_front();
        }
        history.push_back(GoldEntry {
            buying_rate: buy,
            selling_rate: sell,
            status,
            diff,
            created_at,
        });
    }

    state.last_buy.store(buy, Ordering::Relaxed);
    state.has_last_buy.store(true, Ordering::Relaxed);
    state.invalidate_cache();
    state.ws_manager.broadcast(state.get_cached_state());
}

pub async fn treasury_ws_loop(state: Arc<AppState>) {
    let mut errors: u32 = 0;

    loop {
        match connect_async(TREASURY_WS_URL).await {
            Ok((ws, _)) => {
                errors = 0;
                let (mut write, mut read) = ws.split();

                let sub = serde_json::json!({
                    "event": "pusher:subscribe",
                    "data": {"channel": TREASURY_CHANNEL}
                });
                if write
                    .send(Message::Text(sub.to_string().into()))
                    .await
                    .is_err()
                {
                    continue;
                }

                while let Some(Ok(msg)) = read.next().await {
                    match msg {
                        Message::Text(text) => {
                            if let Ok(pm) = serde_json::from_str::<PusherMessage>(&text) {
                                if pm.event.as_deref() == Some(TREASURY_EVENT) {
                                    if let Some(dv) = pm.data {
                                        let gd: Option<GoldRateData> = match dv {
                                            serde_json::Value::String(s) => {
                                                serde_json::from_str(&s).ok()
                                            }
                                            other => serde_json::from_value(other).ok(),
                                        };
                                        if let Some(g) = gd {
                                            process_data(&state, g).await;
                                        }
                                    }
                                }
                            }
                        }
                        Message::Ping(d) => {
                            let _ = write.send(Message::Pong(d)).await;
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            }
            Err(_) => {
                errors += 1;
            }
        }

        let wait = std::cmp::min(errors as u64, 15);
        tokio::time::sleep(tokio::time::Duration::from_secs(wait)).await;
    }
}