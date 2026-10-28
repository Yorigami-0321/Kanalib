use crate::*;

pub(crate) fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.;
    const MB: f64 = KB * 1024.;
    const GB: f64 = MB * 1024.;
    let bytes = bytes as f64;
    if bytes >= GB {
        format!("{:.1} GB", bytes / GB)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes / MB)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes / KB)
    } else {
        format!("{} B", bytes as u64)
    }
}

pub(crate) fn format_date(seconds: u64) -> String {
    if seconds == 0 {
        return "未知".to_string();
    }
    let days = (seconds / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    format!("{year:04}-{month:02}-{day:02}")
}

/// 纯 std 相对时间；超过 30 天回退为日期。
pub(crate) fn relative_time(seconds: u64) -> String {
    if seconds == 0 {
        return "从未".to_string();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    if now <= seconds {
        return "刚刚".to_string();
    }
    let diff = now - seconds;
    match diff {
        0..=59 => "刚刚".to_string(),
        60..=3_599 => format!("{} 分钟前", diff / 60),
        3_600..=86_399 => format!("{} 小时前", diff / 3_600),
        86_400..=2_591_999 => format!("{} 天前", diff / 86_400),
        _ => format_date(seconds),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn relative_time_buckets_are_stable() {
        assert_eq!(super::relative_time(0), "从未");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert_eq!(super::relative_time(now), "刚刚");
        assert_eq!(super::relative_time(now - 120), "2 分钟前");
        assert_eq!(super::relative_time(now - 7_200), "2 小时前");
        assert_eq!(super::relative_time(now - 172_800), "2 天前");
    }

}
