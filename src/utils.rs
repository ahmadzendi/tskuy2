use std::time::{SystemTime, UNIX_EPOCH};

pub fn current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn current_wib_time() -> String {
    let secs = current_timestamp() + 7 * 3600;
    let d = secs % 86400;
    format!("{:02}:{:02}:{:02}", d / 3600, (d % 3600) / 60, d % 60)
}

pub fn format_rupiah(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let b = s.as_bytes();
    let len = b.len();

    if len <= 3 {
        return if n < 0 { format!("-{}", s) } else { s };
    }

    let mut r = String::with_capacity(len + len / 3);
    let first = len % 3;

    if first > 0 {
        r.push_str(&s[..first]);
    }
    for i in (first..len).step_by(3) {
        if !r.is_empty() {
            r.push('.');
        }
        r.push_str(&s[i..i + 3]);
    }

    if n < 0 {
        format!("-{}", r)
    } else {
        r
    }
}

pub fn format_diff_display(diff: i64, status: &str) -> String {
    match status {
        "🚀" => format!("🚀+{}", format_rupiah(diff)),
        "🔻" => format!("🔻-{}", format_rupiah(diff.abs())),
        _ => "➖tetap".into(),
    }
}

pub fn format_waktu_only(created_at: &str, status: &str) -> String {
    let time = if created_at.len() >= 19 {
        &created_at[11..19]
    } else {
        created_at
    };
    format!("{}{}", time, status)
}

pub fn calc_profit(buy_rate: i64, sell_rate: i64, modal: i64, pokok: i64) -> String {
    if buy_rate == 0 {
        return "-".into();
    }

    let gram = modal as f64 / buy_rate as f64;
    let val = (gram * sell_rate as f64 - pokok as f64) as i64;
    let gram_str = format!("{:.4}", gram).replace('.', ",");

    if val > 0 {
        format!("+{}🟢{}gr", format_rupiah(val), gram_str)
    } else if val < 0 {
        format!("-{}🔴{}gr", format_rupiah(val.abs()), gram_str)
    } else {
        format!("{}➖{}gr", format_rupiah(0), gram_str)
    }
}