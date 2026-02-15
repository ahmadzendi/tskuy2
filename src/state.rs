use arc_swap::ArcSwap;
use bytes::Bytes;
use dashmap::DashMap;
use parking_lot::{Mutex, RwLock};
use std::collections::{HashSet, VecDeque};
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::config::*;
use crate::utils;
use crate::ws_manager::WsManager;

// ─── Data Structures ───

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct GoldEntry {
    pub buying_rate: i64,
    pub selling_rate: i64,
    pub status: String,
    pub diff: i64,
    pub created_at: String,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct UsdIdrEntry {
    pub price: String,
    pub time: String,
}

#[derive(serde::Serialize)]
struct HistoryItem<'a> {
    buying_rate: &'a str,
    selling_rate: &'a str,
    buying_rate_raw: i64,
    selling_rate_raw: i64,
    waktu_display: &'a str,
    diff_display: &'a str,
    transaction_display: &'a str,
    created_at: &'a str,
    jt10: &'a str,
    jt20: &'a str,
    jt30: &'a str,
    jt40: &'a str,
    jt50: &'a str,
}

// Owned version for building
struct HistoryItemOwned {
    buying_rate: String,
    selling_rate: String,
    buying_rate_raw: i64,
    selling_rate_raw: i64,
    waktu_display: String,
    diff_display: String,
    transaction_display: String,
    created_at: String,
    jt10: String,
    jt20: String,
    jt30: String,
    jt40: String,
    jt50: String,
}

impl HistoryItemOwned {
    fn as_ref(&self) -> HistoryItem<'_> {
        HistoryItem {
            buying_rate: &self.buying_rate,
            selling_rate: &self.selling_rate,
            buying_rate_raw: self.buying_rate_raw,
            selling_rate_raw: self.selling_rate_raw,
            waktu_display: &self.waktu_display,
            diff_display: &self.diff_display,
            transaction_display: &self.transaction_display,
            created_at: &self.created_at,
            jt10: &self.jt10,
            jt20: &self.jt20,
            jt30: &self.jt30,
            jt40: &self.jt40,
            jt50: &self.jt50,
        }
    }
}

// ─── Serialization Helper (manual JSON, zero-copy) ───

struct JsonWriter {
    buf: Vec<u8>,
}

impl JsonWriter {
    fn with_capacity(cap: usize) -> Self {
        Self { buf: Vec::with_capacity(cap) }
    }

    #[inline]
    fn write_raw(&mut self, s: &[u8]) {
        self.buf.extend_from_slice(s);
    }

    #[inline]
    fn write_str_value(&mut self, s: &str) {
        self.buf.push(b'"');
        // Escape JSON string — fast path for ASCII
        for &b in s.as_bytes() {
            match b {
                b'"' => self.buf.extend_from_slice(b"\\\""),
                b'\\' => self.buf.extend_from_slice(b"\\\\"),
                b'\n' => self.buf.extend_from_slice(b"\\n"),
                b'\r' => self.buf.extend_from_slice(b"\\r"),
                b'\t' => self.buf.extend_from_slice(b"\\t"),
                _ => self.buf.push(b),
            }
        }
        self.buf.push(b'"');
    }

    #[inline]
    fn write_i64(&mut self, v: i64) {
        let mut buf = itoa::Buffer::new();
        self.buf.extend_from_slice(buf.format(v).as_bytes());
    }

    fn into_bytes(self) -> Bytes {
        Bytes::from(self.buf)
    }
}

// ─── Cached State ───

pub struct CachedState {
    pub data: Bytes,
    pub version: u64,
    pub created_at: Instant,
}

// ─── App State ───

pub struct AppState {
    pub history: RwLock<VecDeque<GoldEntry>>,
    pub usd_idr_history: RwLock<VecDeque<UsdIdrEntry>>,
    pub last_buy: AtomicI64,
    pub has_last_buy: AtomicBool,
    pub shown_updates: Mutex<HashSet<String>>,
    pub limit_bulan: AtomicI64,
    pub ws_manager: WsManager,
    pub rate_limiter: crate::rate_limiter::RateLimiter,
    pub blocked_ips: DashMap<String, u64>,
    pub failed_attempts: DashMap<String, Vec<u64>>,
    pub last_successful_call: AtomicU64,
    state_cache: ArcSwap<CachedState>,
    cache_version: AtomicU64,
}

impl AppState {
    pub fn new() -> Self {
        // Pre-build empty state
        let empty_data = Bytes::from_static(
            br#"{"history":[],"usd_idr_history":[],"limit_bulan":8}"#
        );

        Self {
            history: RwLock::new(VecDeque::with_capacity(MAX_HISTORY)),
            usd_idr_history: RwLock::new(VecDeque::with_capacity(MAX_USD_HISTORY)),
            last_buy: AtomicI64::new(0),
            has_last_buy: AtomicBool::new(false),
            shown_updates: Mutex::new(HashSet::with_capacity(64)),
            limit_bulan: AtomicI64::new(8),
            ws_manager: WsManager::new(),
            rate_limiter: crate::rate_limiter::RateLimiter::new(),
            blocked_ips: DashMap::with_capacity(32),
            failed_attempts: DashMap::with_capacity(32),
            last_successful_call: AtomicU64::new(0),
            state_cache: ArcSwap::new(Arc::new(CachedState {
                data: empty_data,
                version: 0,
                created_at: Instant::now(),
            })),
            cache_version: AtomicU64::new(0),
        }
    }

    #[inline]
    pub fn invalidate_cache(&self) {
        self.cache_version.fetch_add(1, Ordering::Release);
    }

    pub fn get_cached_state(&self) -> Bytes {
        let current = self.state_cache.load();
        let ver = self.cache_version.load(Ordering::Acquire);

        if current.version == ver
            && current.created_at.elapsed().as_millis() < STATE_CACHE_TTL_MS as u128
        {
            return current.data.clone();
        }

        let data = self.build_full_state_fast();

        self.state_cache.store(Arc::new(CachedState {
            data: data.clone(),
            version: ver,
            created_at: Instant::now(),
        }));

        data
    }

    /// Fast manual JSON serialization — avoids serde overhead
    fn build_full_state_fast(&self) -> Bytes {
        let history = self.history.read();
        let usd = self.usd_idr_history.read();
        let limit = self.limit_bulan.load(Ordering::Relaxed);

        // Pre-build history items
        let items: Vec<HistoryItemOwned> = history
            .iter()
            .map(|h| Self::build_item(h))
            .collect();

        // Estimate capacity: ~500 bytes per history item + ~100 per usd entry
        let estimated = items.len() * 500 + usd.len() * 100 + 64;
        let mut w = JsonWriter::with_capacity(estimated);

        // Start object
        w.write_raw(b"{\"history\":[");

        for (i, item) in items.iter().enumerate() {
            if i > 0 { w.write_raw(b","); }
            w.write_raw(b"{\"buying_rate\":");
            w.write_str_value(&item.buying_rate);
            w.write_raw(b",\"selling_rate\":");
            w.write_str_value(&item.selling_rate);
            w.write_raw(b",\"buying_rate_raw\":");
            w.write_i64(item.buying_rate_raw);
            w.write_raw(b",\"selling_rate_raw\":");
            w.write_i64(item.selling_rate_raw);
            w.write_raw(b",\"waktu_display\":");
            w.write_str_value(&item.waktu_display);
            w.write_raw(b",\"diff_display\":");
            w.write_str_value(&item.diff_display);
            w.write_raw(b",\"transaction_display\":");
            w.write_str_value(&item.transaction_display);
            w.write_raw(b",\"created_at\":");
            w.write_str_value(&item.created_at);
            w.write_raw(b",\"jt10\":");
            w.write_str_value(&item.jt10);
            w.write_raw(b",\"jt20\":");
            w.write_str_value(&item.jt20);
            w.write_raw(b",\"jt30\":");
            w.write_str_value(&item.jt30);
            w.write_raw(b",\"jt40\":");
            w.write_str_value(&item.jt40);
            w.write_raw(b",\"jt50\":");
            w.write_str_value(&item.jt50);
            w.write_raw(b"}");
        }

        w.write_raw(b"],\"usd_idr_history\":[");

        for (i, entry) in usd.iter().enumerate() {
            if i > 0 { w.write_raw(b","); }
            w.write_raw(b"{\"price\":");
            w.write_str_value(&entry.price);
            w.write_raw(b",\"time\":");
            w.write_str_value(&entry.time);
            w.write_raw(b"}");
        }

        w.write_raw(b"],\"limit_bulan\":");
        w.write_i64(limit);
        w.write_raw(b"}");

        w.into_bytes()
    }

    fn build_item(h: &GoldEntry) -> HistoryItemOwned {
        let buy_fmt = utils::format_rupiah(h.buying_rate);
        let sell_fmt = utils::format_rupiah(h.selling_rate);
        let diff_display = utils::format_diff_display(h.diff, &h.status);
        let waktu_display = utils::format_waktu_only(&h.created_at, &h.status);
        let transaction_display =
            format!("Beli: {}<br>Jual: {}<br>{}", buy_fmt, sell_fmt, diff_display);

        HistoryItemOwned {
            buying_rate: buy_fmt,
            selling_rate: sell_fmt,
            buying_rate_raw: h.buying_rate,
            selling_rate_raw: h.selling_rate,
            waktu_display,
            diff_display,
            transaction_display,
            created_at: h.created_at.clone(),
            jt10: utils::calc_profit(h.buying_rate, h.selling_rate, 10_000_000, 9_669_000),
            jt20: utils::calc_profit(h.buying_rate, h.selling_rate, 20_000_000, 19_330_000),
            jt30: utils::calc_profit(h.buying_rate, h.selling_rate, 30_000_000, 28_995_000),
            jt40: utils::calc_profit(h.buying_rate, h.selling_rate, 40_000_000, 38_660_000),
            jt50: utils::calc_profit(h.buying_rate, h.selling_rate, 50_000_000, 48_325_000),
        }
    }

    #[inline]
    pub fn is_ip_blocked(&self, ip: &str) -> bool {
        if let Some(entry) = self.blocked_ips.get(ip) {
            let now = utils::current_timestamp();
            if now < *entry {
                return true;
            }
            drop(entry);
            self.blocked_ips.remove(ip);
            self.failed_attempts.remove(ip);
        }
        false
    }

    #[inline]
    pub fn block_ip(&self, ip: &str, duration: u64) {
        self.blocked_ips
            .insert(ip.to_string(), utils::current_timestamp() + duration);
    }

    pub fn record_failed_attempt(&self, ip: &str, weight: usize) {
        let now = utils::current_timestamp();
        let mut entry = self
            .failed_attempts
            .entry(ip.to_string())
            .or_insert_with(|| Vec::with_capacity(MAX_FAILED_ATTEMPTS));

        for _ in 0..weight {
            entry.push(now);
        }
        entry.retain(|&t| now - t < 60);

        if entry.len() >= MAX_FAILED_ATTEMPTS {
            drop(entry);
            self.block_ip(ip, BLOCK_DURATION_SECS);
        }
    }
}
