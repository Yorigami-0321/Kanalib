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

/// 扩展名是否属于动画容器（GIF/WebP）。是否真的含多帧由文件内容决定，
/// 但 UI 不解析帧，只按容器格式提示“仅首帧”，避免假装播放。
pub(crate) fn is_animation_container(path: &Path) -> bool {
    path.extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .is_some_and(|ext| matches!(ext.as_str(), "gif" | "webp"))
}

/// 已知但本轮未接入扫描/解码的格式（用于明确边界，不做泛化支持）。
pub(crate) fn unsupported_format_note(path: &Path) -> Option<&'static str> {
    let ext = path
        .extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "heic" | "heif" => Some(
            "HEIC/HEIF 未接入：不在扫描白名单，也没有可分发解码链与实测样本；当前版本不支持。",
        ),
        "dng" | "cr2" | "cr3" | "nef" | "arw" | "raf" | "orf" | "rw2" | "pef" | "srw" | "x3f" => {
            Some("相机 RAW 未接入：不在扫描白名单，无合法解码链与实测样本；当前版本不支持。")
        }
        _ => None,
    }
}

/// 格式能力边界提示；只描述本轮实测过的行为，不承诺未接入能力。
pub(crate) fn format_limits_note(path: &Path) -> Option<&'static str> {
    if is_animation_container(path) {
        return Some(
            "GIF/WebP 动画当前只显示首帧，不提供播放器；可用「用系统应用播放」查看完整动画。",
        );
    }
    if unsupported_format_note(path).is_some() {
        return unsupported_format_note(path);
    }
    let ext = path
        .extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    (ext == "psd").then_some(
        "PSD 只显示合成图：图层/智能对象不展开、Alpha 不保留、不计算 pHash（不参与相似去重），不支持 .psb。",
    )
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
    use std::path::Path;
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

    #[test]
    fn animation_and_unsupported_format_notes_are_explicit() {
        assert!(super::is_animation_container(Path::new("a.GIF")));
        assert!(super::is_animation_container(Path::new("a.webp")));
        assert!(!super::is_animation_container(Path::new("a.png")));
        let gif = super::format_limits_note(Path::new("anim.gif")).unwrap();
        assert!(gif.contains("首帧"), "{gif}");
        assert!(gif.contains("系统应用"), "{gif}");
        let psd = super::format_limits_note(Path::new("layer.psd")).unwrap();
        assert!(psd.contains("合成图"), "{psd}");
        assert!(psd.contains("pHash"), "{psd}");
        let heic = super::unsupported_format_note(Path::new("photo.HEIC")).unwrap();
        assert!(heic.contains("未接入") && heic.contains("不支持"), "{heic}");
        let raw = super::unsupported_format_note(Path::new("photo.cr2")).unwrap();
        assert!(raw.contains("RAW") && raw.contains("不支持"), "{raw}");
        assert!(super::format_limits_note(Path::new("normal.png")).is_none());
    }

}
