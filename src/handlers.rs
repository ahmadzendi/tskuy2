use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path, Query, State, WebSocketUpgrade,
    },
    http::{HeaderMap, StatusCode, Uri},
    response::{Html, IntoResponse, Response},
    routing::{any, get},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use subtle::ConstantTimeEq;

use crate::config::*;
use crate::state::AppState;
use crate::template::HTML_TEMPLATE;
use crate::utils;

#[derive(serde::Deserialize)]
pub struct LimitQuery {
    key: Option<String>,
}

fn ip_from_headers(h: &HeaderMap) -> String {
    if let Some(v) = h.get("x-forwarded-for") {
        if let Ok(s) = v.to_str() {
            if let Some(f) = s.split(',').next() {
                return f.trim().to_string();
            }
        }
    }
    if let Some(v) = h.get("x-real-ip") {
        if let Ok(s) = v.to_str() {
            return s.trim().to_string();
        }
    }
    "unknown".to_string()
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(index))
        .route("/health", get(health))
        .route("/api/state", get(get_state))
        .route("/ws", get(ws_handler))
        .route("/aturTS/:value", get(set_limit))
        .fallback(any(catch_all))
}

async fn index() -> Html<&'static str> {
    Html(HTML_TEMPLATE)
}

async fn health() -> &'static str {
    "ok"
}

async fn get_state(State(state): State<Arc<AppState>>) -> Response {
    (
        StatusCode::OK,
        [("content-type", "application/json")],
        state.get_cached_state(),
    )
        .into_response()
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| handle_ws(socket, state))
}

async fn handle_ws(socket: WebSocket, state: Arc<AppState>) {
    let mut rx = match state.ws_manager.subscribe() {
        Some(rx) => rx,
        None => return,
    };

    let (mut sender, mut receiver) = socket.split();

    if sender
        .send(Message::Binary(state.get_cached_state().to_vec().into()))
        .await
        .is_err()
    {
        state.ws_manager.unsubscribe();
        return;
    }

    let send_task = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(data) => {
                    if sender
                        .send(Message::Binary(data.to_vec().into()))
                        .await
                        .is_err()
                    {
                        break;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });

    let recv_task = tokio::spawn(async move {
        loop {
            match tokio::time::timeout(
                tokio::time::Duration::from_secs(WS_TIMEOUT_SECS),
                receiver.next(),
            )
            .await
            {
                Ok(Some(Ok(Message::Text(_) | Message::Binary(_)))) => {}
                _ => break,
            }
        }
    });

    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }

    state.ws_manager.unsubscribe();
}

async fn set_limit(
    State(state): State<Arc<AppState>>,
    Path(value): Path<String>,
    Query(query): Query<LimitQuery>,
    headers: HeaderMap,
) -> Response {
    let ip = ip_from_headers(&headers);

    if state.is_ip_blocked(&ip) {
        return (StatusCode::TOO_MANY_REQUESTS, "IP diblokir sementara").into_response();
    }

    let key = match query.key {
        Some(k) if !k.is_empty() => k,
        _ => {
            state.record_failed_attempt(&ip, 2);
            return (StatusCode::BAD_REQUEST, "Parameter key diperlukan").into_response();
        }
    };

    let kb = key.as_bytes();
    let sb = SECRET_KEY.as_bytes();
    if kb.len() != sb.len() || kb.ct_eq(sb).unwrap_u8() != 1 {
        state.record_failed_attempt(&ip, 1);
        return (StatusCode::FORBIDDEN, "Akses ditolak").into_response();
    }

    let int_value: i64 = match value.parse() {
        Ok(v) => v,
        Err(_) => {
            state.record_failed_attempt(&ip, 1);
            return (StatusCode::BAD_REQUEST, "Nilai harus angka").into_response();
        }
    };

    let now = utils::current_timestamp();
    let last = state.last_successful_call.load(Ordering::Relaxed);
    if now - last < RATE_LIMIT_SECONDS {
        return (StatusCode::TOO_MANY_REQUESTS, "Terlalu cepat").into_response();
    }

    if int_value < MIN_LIMIT || int_value > MAX_LIMIT {
        return (
            StatusCode::BAD_REQUEST,
            format!("Nilai harus {}-{}", MIN_LIMIT, MAX_LIMIT),
        )
            .into_response();
    }

    state.limit_bulan.store(int_value, Ordering::Relaxed);
    state.last_successful_call.store(now, Ordering::Relaxed);
    state.invalidate_cache();

    let cached = state.get_cached_state();
    state.ws_manager.broadcast(cached);

    (
        StatusCode::OK,
        axum::Json(serde_json::json!({"status":"ok","limit_bulan":int_value})),
    )
        .into_response()
}

async fn catch_all(State(state): State<Arc<AppState>>, headers: HeaderMap, uri: Uri) -> Response {
    let ip = ip_from_headers(&headers);
    let path = uri.path().to_lowercase();

    if state.is_ip_blocked(&ip) {
        return (StatusCode::TOO_MANY_REQUESTS, "IP diblokir sementara").into_response();
    }

    if !path.starts_with("/aturt")
        && (path.contains("admin") || path.contains("config"))
    {
        state.record_failed_attempt(&ip, 2);
        return (StatusCode::FORBIDDEN, "Akses ditolak").into_response();
    }

    state.record_failed_attempt(&ip, 1);
    (StatusCode::NOT_FOUND, "Halaman tidak ditemukan").into_response()
}