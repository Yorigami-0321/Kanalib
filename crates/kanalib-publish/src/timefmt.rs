//! UTC RFC3339 时间戳（`2026-11-08T23:12:00Z`）。
//!
//! 发布器只用途这一个时间格式，为它引入 date 库不值得；这里用
//! Howard Hinnant 的 days-from-civil 反算，覆盖 1970 年以后的时间。

use std::time::{SystemTime, UNIX_EPOCH};

/// 当前时间，秒级 UTC，形如 `2026-11-08T23:12:00Z`。
pub fn rfc3339_utc_now() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0);
    rfc3339_utc(secs)
}

/// 把 UTC 秒格式化成 RFC3339（秒级，`Z` 后缀）。
pub fn rfc3339_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::rfc3339_utc;

    #[test]
    fn formats_known_instants() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_utc(1_600_000_000), "2020-09-13T12:26:40Z");
        assert_eq!(rfc3339_utc(1_792_843_111), "2026-10-24T11:58:31Z");
        // 闰日
        assert_eq!(rfc3339_utc(1_582_934_400), "2020-02-29T00:00:00Z");
    }
}
