use scraper::{Html, Selector};
use std::sync::Arc;

use crate::config::*;
use crate::state::{AppState, UsdIdrEntry};
use crate::utils;

async fn fetch_price(client: &reqwest::Client) -> Option<String> {
    let resp = client
        .get("https://www.google.com/finance/quote/USD-IDR")
        .header("Accept", "text/html,application/xhtml+xml")
        .header("Cookie", "CONSENT=YES+cb.20231208-04-p0.en+FX+410")
        .send()
        .await
        .ok()?;

    if resp.status() != 200 {
        return None;
    }

    let text = resp.text().await.ok()?;
    let doc = Html::parse_document(&text);
    let sel = Selector::parse("div.YMlKec.fxKbKc").ok()?;

    doc.select(&sel)
        .next()
        .map(|el| el.text().collect::<String>().trim().to_string())
}

pub async fn usd_idr_loop(state: Arc<AppState>) {
    let client = reqwest::Client::builder()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .gzip(true)
        .pool_max_idle_per_host(5)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    loop {
        if let Some(price) = fetch_price(&client).await {
            let should_update = {
                let h = state.usd_idr_history.read();
                h.is_empty() || h.back().map(|e| &e.price) != Some(&price)
            };

            if should_update {
                let mut h = state.usd_idr_history.write();
                if h.len() >= MAX_USD_HISTORY {
                    h.pop_front();
                }
                h.push_back(UsdIdrEntry {
                    price,
                    time: utils::current_wib_time(),
                });
                drop(h);

                state.invalidate_cache();
                state.ws_manager.broadcast(state.get_cached_state());
            }
        }

        tokio::time::sleep(tokio::time::Duration::from_millis(USD_POLL_INTERVAL_MS)).await;
    }
}