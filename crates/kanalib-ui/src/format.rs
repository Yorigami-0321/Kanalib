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
/// 统一引用核心的 `UNSUPPORTED_FORMAT_NOTE`，避免 UI 与扫描口径漂移。
pub(crate) fn unsupported_format_note(path: &Path) -> Option<String> {
    kanalib_core::model::is_unsupported_format(path).then(|| {
        format!(
            "{} 当前版本不支持。{}",
            crate::inspector::friendly_format_label(path),
            kanalib_core::model::UNSUPPORTED_FORMAT_NOTE
        )
    })
}

/// 格式能力边界提示；只描述本轮实测过的行为，不承诺未接入能力。
pub(crate) fn format_limits_note(path: &Path) -> Option<String> {
    if is_animation_container(path) {
        return Some(
            "GIF/WebP 动画当前只显示首帧，不提供播放器；可用「用系统应用播放」查看完整动画。"
                .to_string(),
        );
    }
    if let Some(note) = unsupported_format_note(path) {
        return Some(note);
    }
    let ext = path
        .extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    (ext == "psd").then(|| {
        "PSD 只显示合成图：图层/智能对象不展开、Alpha 不保留、不计算 pHash（不参与相似去重），不支持 .psb。"
            .to_string()
    })
}

/// F-040：`metadata_json` 中的视频时长与编码。
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct VideoMeta {
    pub(crate) duration_secs: Option<f64>,
    pub(crate) codec: String,
}

/// 从资产元数据 JSON 读取视频信息；图片或损坏 JSON 返回空值。
pub(crate) fn video_meta(metadata_json: &str) -> VideoMeta {
    let Ok(meta) = serde_json::from_str::<kanalib_core::media::ExifData>(metadata_json) else {
        return VideoMeta::default();
    };
    VideoMeta {
        duration_secs: meta.duration_secs,
        codec: meta.codec,
    }
}

/// 时长显示：不足 1 小时为 `mm:ss`，否则 `h:mm:ss`；未知/非法值显式写“未知”。
pub(crate) fn format_duration(secs: f64) -> String {
    if !secs.is_finite() || secs < 0.0 {
        return "未知".to_string();
    }
    let total = secs.round() as u64;
    let hours = total / 3_600;
    let minutes = (total % 3_600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// 常见视频编码的展示名；未知编码保留原名大写，空值写“未知”。
pub(crate) fn friendly_codec_label(codec: &str) -> String {
    match codec.trim().to_ascii_lowercase().as_str() {
        "" => "未知".to_string(),
        "h264" | "avc" => "H.264".to_string(),
        "hevc" | "h265" => "HEVC".to_string(),
        "av1" => "AV1".to_string(),
        "vp8" => "VP8".to_string(),
        "vp9" => "VP9".to_string(),
        "mpeg4" => "MPEG-4".to_string(),
        "mpeg2video" => "MPEG-2".to_string(),
        "mpeg1video" => "MPEG-1".to_string(),
        "mjpeg" => "MJPEG".to_string(),
        "prores" => "ProRes".to_string(),
        "dnxhd" => "DNxHD".to_string(),
        "theora" => "Theora".to_string(),
        other => other.to_uppercase(),
    }
}

/// 视频详情行：时长、编码、分辨率；缺数据时写“未知”，不伪造值。
pub(crate) fn video_fact_rows(
    meta: &VideoMeta,
    width: u32,
    height: u32,
) -> Vec<(&'static str, String)> {
    vec![
        (
            "时长",
            meta.duration_secs
                .map(format_duration)
                .unwrap_or_else(|| "未知".to_string()),
        ),
        ("编码", friendly_codec_label(&meta.codec)),
        (
            "分辨率",
            if width > 0 && height > 0 {
                format!("{width} × {height}")
            } else {
                "未知".to_string()
            },
        ),
    ]
}

/// 远端只读提示：虚拟路径没有本地原图。
pub(crate) const REMOTE_READONLY_NOTE: &str =
    "远端只读来源：未下载原图，不能交给系统打开、在资源管理器中显示或移入回收站。";

/// 是否为远端虚拟路径（`immich://` / `webdav://`）。
pub(crate) fn is_remote_asset(path: &Path) -> bool {
    kanalib_core::sources::is_remote_path(path)
}

/// 打开/显示/回收站入口的统一只读守卫；远端一律拒绝。
pub(crate) fn can_touch_local_file(path: &Path) -> bool {
    !is_remote_asset(path)
}

/// 一组资产中是否含远端只读项（去重组按钮同步禁用）。
pub(crate) fn any_remote_asset<'a>(paths: impl IntoIterator<Item = &'a Path>) -> bool {
    paths.into_iter().any(is_remote_asset)
}

/// 把本地资产交给系统默认程序：视频走 `open_video`，其余媒体走默认关联程序。
/// 远端路径在这里直接拒绝，绝不调用 explorer/xdg-open。
pub(crate) fn open_asset_with_system(path: &Path) -> Result<(), String> {
    if is_remote_asset(path) {
        return Err(format!("{REMOTE_READONLY_NOTE}（{}）", path.display()));
    }
    let result = if kanalib_core::model::is_video(path) {
        kanalib_core::open::open_video(path)
    } else {
        kanalib_core::open::open_with_default_app(path)
    };
    result.map_err(|error| format!("打开失败：{error:#}"))
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
        assert!(
            heic.contains("不支持") && heic.contains("HEIC/HEIF"),
            "{heic}"
        );
        assert!(
            heic.contains(kanalib_core::model::UNSUPPORTED_FORMAT_NOTE),
            "{heic}"
        );
        let raw = super::unsupported_format_note(Path::new("photo.cr2")).unwrap();
        assert!(raw.contains("RAW") && raw.contains("不支持"), "{raw}");
        assert!(super::format_limits_note(Path::new("normal.png")).is_none());
    }

    #[test]
    fn duration_and_codec_labels_cover_video_edges() {
        assert_eq!(super::format_duration(0.0), "00:00");
        assert_eq!(super::format_duration(12.4), "00:12");
        assert_eq!(super::format_duration(65.0), "01:05");
        assert_eq!(super::format_duration(3_661.0), "1:01:01");
        assert_eq!(super::format_duration(-1.0), "未知");
        assert_eq!(super::format_duration(f64::NAN), "未知");
        assert_eq!(super::friendly_codec_label("h264"), "H.264");
        assert_eq!(super::friendly_codec_label("HEVC"), "HEVC");
        assert_eq!(super::friendly_codec_label("av1"), "AV1");
        assert_eq!(super::friendly_codec_label(""), "未知");
        assert_eq!(super::friendly_codec_label("mystery"), "MYSTERY");
    }

    #[test]
    fn video_meta_reads_duration_and_codec_without_touching_images() {
        let json = r#"{"duration_secs":12.5,"codec":"h264"}"#;
        let meta = super::video_meta(json);
        assert_eq!(meta.duration_secs, Some(12.5));
        assert_eq!(meta.codec, "h264");
        assert_eq!(
            super::video_fact_rows(&meta, 1920, 1080),
            vec![
                ("时长", "00:13".to_string()),
                ("编码", "H.264".to_string()),
                ("分辨率", "1920 × 1080".to_string()),
            ]
        );
        assert_eq!(super::video_meta("not json"), super::VideoMeta::default());
        assert_eq!(super::video_meta("{}"), super::VideoMeta::default());
        let missing = super::video_fact_rows(&super::VideoMeta::default(), 0, 0);
        assert_eq!(missing[0], ("时长", "未知".to_string()));
        assert_eq!(missing[1], ("编码", "未知".to_string()));
        assert_eq!(missing[2], ("分辨率", "未知".to_string()));
    }

    #[test]
    fn remote_assets_are_read_only_for_open_and_trash() {
        let immich = Path::new("immich://asset-1");
        let webdav = Path::new("webdav://source-1/photos/a.jpg");
        let local = Path::new(r"C:\pics\a.jpg");
        assert!(super::is_remote_asset(immich));
        assert!(super::is_remote_asset(webdav));
        assert!(!super::is_remote_asset(local));
        assert!(!super::can_touch_local_file(immich));
        assert!(!super::can_touch_local_file(webdav));
        assert!(super::can_touch_local_file(local));
        assert!(super::any_remote_asset([local, webdav]));
        assert!(!super::any_remote_asset([local]));
        // 守卫在启动任何进程前拒绝远端路径。
        let error = super::open_asset_with_system(immich).unwrap_err();
        assert!(error.contains("远端只读"), "{error}");
        assert!(error.contains("未下载原图"), "{error}");
    }

}
