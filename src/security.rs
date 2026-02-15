use axum::{
    body::Body,
    extract::{Request, State},
    http::{Response, StatusCode},
    middleware::Next,
    response::IntoResponse,
};
use std::sync::Arc;

use crate::config::*;
use crate::rate_limiter::RateLimitStatus;
use crate::state::AppState;

const HTML_429: &str = "<!DOCTYPE html><html><head><title>429</title></head><body><h1>Too Many Requests</h1></body></html>";

pub fn get_client_ip(req: &Request) -> String {
    if let Some(v) = req.headers().get("x-forwarded-for") {
        if let Ok(s) = v.to_str() {
            if let Some(first) = s.split(',').next() {
                return first.trim().to_string();
            }
        }
    }
    if let Some(v) = req.headers().get("x-real-ip") {
        if let Ok(s) = v.to_str() {
            return s.trim().to_string();
        }
    }
    "unknown".to_string()
}

fn is_suspicious(path: &str) -> bool {
    let p = path.to_lowercase();
    if p.starts_with("/aturt") {
        return false;
    }
    SUSPICIOUS_PATHS.iter().any(|&s| p.contains(s))
}

fn response_429() -> Response<Body> {
    Response::builder()
        .status(StatusCode::TOO_MANY_REQUESTS)
        .header("Content-Type", "text/html")
        .header("Retry-After", "60")
        .body(Body::from(HTML_429))
        .unwrap()
}

pub async fn security_middleware(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> impl IntoResponse {
    let ip = get_client_ip(&req);
    let path = req.uri().path().to_string();
    let pl = path.to_lowercase();

    if state.is_ip_blocked(&ip) {
        return response_429();
    }

    let whitelisted =
        path == "/ws" || path == "/api/state" || path == "/health" || path == "/" || pl.starts_with("/aturt");

    if !whitelisted {
        let (_ok, _count, status) = state.rate_limiter.check(&ip);
        match status {
            RateLimitStatus::Blocked => {
                state.block_ip(&ip, 600);
                return response_429();
            }
            RateLimitStatus::Limited => return response_429(),
            RateLimitStatus::Ok => {}
        }
    }

    if is_suspicious(&path) {
        state.record_failed_attempt(&ip, 3);
        return Response::builder()
            .status(StatusCode::FORBIDDEN)
            .body(Body::from(r#"{"error":"forbidden"}"#))
            .unwrap();
    }

    next.run(req).await.into_response()
}