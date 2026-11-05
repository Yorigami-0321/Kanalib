use crate::app::{KanalibApp, prompt_for, push_toast};
use crate::format::*;
use crate::localization::{localize_group_name, t, t_args};
use crate::sidebar::source_label;
use crate::theme::*;
use crate::*;

/// 媒体错误的中文说明；views/content 与单元测试共用这一签名，暂不走目录。
pub(crate) fn friendly_media_error(message: &str) -> String {
    let text = message.to_lowercase();
    if (text.contains("psd") || text.contains("photoshop"))
        && (text.contains("ffmpeg")
            || text.contains("unsupported")
            || text.contains("unknown format")
            || text.contains("decoder")
            || text.contains("not supported"))
    {
        if crate::localization::is_english() { "PSD requires FFmpeg; without it only the hash and dimensions are available, so no thumbnail can be generated".to_string() } else { "PSD 需要 FFmpeg 解码；未检测到 FFmpeg 时只有哈希与尺寸，无法生成缩略图".to_string() }
    } else if text.contains("heic") || text.contains("heif") || text.contains("libheif") {
        if crate::localization::is_english() { "HEIC/HEIF is not supported in this version".to_string() } else { "HEIC/HEIF 未接入，当前版本不支持该格式".to_string() }
    } else if text.contains("avif")
        && (text.contains("unsupported")
            || text.contains("unknown format")
            || text.contains("not supported")
            || text.contains("decoder")
            || text.contains("cannot identify"))
    {
        if crate::localization::is_english() { "AVIF requires FFmpeg; without it no thumbnail can be generated".to_string() } else { "AVIF 需要 FFmpeg 解码；未检测到 FFmpeg 时无法生成缩略图".to_string() }
    } else if text.contains("ffmpeg") || text.contains("dav1d") {
        if crate::localization::is_english() { "FFmpeg decoding failed; thumbnails and tagging may be unavailable".to_string() } else { "FFmpeg 解码失败，缩略图与打标可能不可用".to_string() }
    } else if text.contains("truncated") || text.contains("unexpected end of file") {
        if crate::localization::is_english() { "The file is incomplete (image data is truncated)".to_string() } else { "文件不完整（图片数据被截断）".to_string() }
    } else if text.contains("cannot identify image file") || text.contains("unidentifiedimageerror")
    {
        if crate::localization::is_english() { "Unrecognized image; the file may be damaged".to_string() } else { "无法识别的图片，文件可能已损坏".to_string() }
    } else if text.contains("decompression bomb") {
        if crate::localization::is_english() { "The image exceeds the safe size limit".to_string() } else { "图片尺寸超出安全上限".to_string() }
    } else if text.contains("unsupported") || text.contains("unknown format") {
        if crate::localization::is_english() { "Unsupported image codec".to_string() } else { "不支持的图片编码".to_string() }
    } else {
        message.to_string()
    }
}

/// 扩展名对应的友好格式名；未知扩展名回退为大写原名。format 详情与测试共用这一签名。
pub(crate) fn friendly_format_label(path: &Path) -> String {
    let ext = path
        .extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "jpg" | "jpeg" => "JPEG".to_string(),
        "png" => "PNG".to_string(),
        "webp" => "WebP".to_string(),
        "gif" => "GIF".to_string(),
        "bmp" => "BMP".to_string(),
        "tif" | "tiff" => "TIFF".to_string(),
        "ico" => "ICO".to_string(),
        "qoi" => "QOI".to_string(),
        "exr" => "EXR".to_string(),
        "avif" => "AVIF".to_string(),
        "psd" => "PSD".to_string(),
        "mp4" | "m4v" => "MP4".to_string(),
        "mov" => "MOV".to_string(),
        "mkv" => "MKV".to_string(),
        "webm" => "WebM".to_string(),
        "avi" => "AVI".to_string(),
        "wmv" => "WMV".to_string(),
        "flv" => "FLV".to_string(),
        "mpg" | "mpeg" => "MPEG".to_string(),
        "3gp" => "3GP".to_string(),
        "3g2" => "3G2".to_string(),
        "ts" => "TS".to_string(),
        "m2ts" => "M2TS".to_string(),
        "mts" => "MTS".to_string(),
        "ogv" => "OGV".to_string(),
        "vob" => "VOB".to_string(),
        "heic" | "heif" => "HEIC/HEIF".to_string(),
        "dng" | "cr2" | "cr3" | "nef" | "arw" | "raf" | "orf" | "rw2" | "pef" | "srw" | "x3f" => {
            "RAW".to_string()
        }
        "" => if crate::localization::is_english() { "Unknown format".to_string() } else { "未知格式".to_string() },
        other => other.to_uppercase(),
    }
}

/// 解码能力提示：AVIF/PSD 需要 FFmpeg；GIF/WebP 内置解码但动画只取首帧；
/// HEIC/RAW 明确未接入，不写成“内置解码”。测试固定了这一签名与中文断言。
pub(crate) fn format_capability_note(path: &Path) -> String {
    if crate::format::unsupported_format_note(path).is_some() {
        return if crate::localization::is_english() { "Not supported (this version cannot scan/decode it)".to_string() } else { "未接入（当前版本不支持扫描/解码）".to_string() };
    }
    if kanalib_core::model::is_video(path) {
        return if crate::localization::is_english() { "First-frame thumbnail · system player".to_string() } else { "首帧缩略图 · 系统播放器".to_string() };
    }
    let ext = path
        .extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if is_ffmpeg_only(path) {
        let available = kanalib_core::ffmpeg::available();
        let mut note = if available {
            if crate::localization::is_english() { "Requires FFmpeg".to_string() } else { "需要 FFmpeg 解码".to_string() }
        } else {
            if crate::localization::is_english() { "Requires FFmpeg · FFmpeg not detected".to_string() } else { "需要 FFmpeg 解码 · 当前未检测到 FFmpeg".to_string() }
        };
        if ext == "psd" {
            note.push_str(if crate::localization::is_english() { " · composite only" } else { " · 仅合成图" });
        }
        note
    } else if crate::format::is_animation_container(path) {
        if crate::localization::is_english() { "Built-in decoder · first frame only".to_string() } else { "内置解码 · 动画仅首帧".to_string() }
    } else {
        if crate::localization::is_english() { "Built-in decoder".to_string() } else { "内置解码".to_string() }
    }
}

/// 标签分类的界面名；目录未登记的分类原样显示其数据键。
fn tag_category_label(cx: &App, category: &str) -> SharedString {
    match category {
        "general" => t(cx, "inspector.tags.category.general"),
        "character" => t(cx, "inspector.tags.category.character"),
        "copyright" => t(cx, "inspector.tags.category.copyright"),
        "style" => t(cx, "inspector.tags.category.style"),
        "meta" => t(cx, "inspector.tags.category.meta"),
        "rating" => t(cx, "inspector.tags.category.rating"),
        other => SharedString::from(other.to_string()),
    }
}

pub(crate) fn exif_panel(asset: &Asset, colors: UiColors, cx: &App) -> Option<AnyElement> {
    let meta = serde_json::from_str::<kanalib_core::media::ExifData>(&asset.metadata_json)
        .unwrap_or_default();
    let video = asset.is_video();
    // 视频即使 ffprobe 缺失也要显示分辨率行，因此不能按 ExifData 为空直接隐藏。
    if !video && meta.is_empty() && asset.captured_at.is_none() {
        return None;
    }
    let video_facts = video
        .then(|| video_fact_rows(&video_meta(&asset.metadata_json), asset.width, asset.height));
    let mut rows: Vec<(SharedString, String)> = Vec::new();
    if let Some(captured) = asset.captured_at {
        rows.push((t(cx, "inspector.exif.captured_at"), format_date(captured)));
    }
    rows.extend(
        video_facts
            .unwrap_or_default()
            .into_iter()
            .map(|(label, value)| (SharedString::from(label), value)),
    );
    if !meta.camera.is_empty() {
        rows.push((t(cx, "inspector.exif.camera"), meta.camera));
    }
    if !meta.lens.is_empty() {
        rows.push((t(cx, "inspector.exif.lens"), meta.lens));
    }
    let exposure = [
        (!meta.iso.is_empty()).then(|| format!("ISO {}", meta.iso.trim())),
        (!meta.aperture.is_empty()).then(|| meta.aperture.trim().to_string()),
        (!meta.shutter.is_empty()).then(|| meta.shutter.trim().to_string()),
        (!meta.focal_length.is_empty()).then(|| meta.focal_length.trim().to_string()),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    if !exposure.is_empty() {
        rows.push((t(cx, "inspector.exif.exposure"), exposure));
    }
    Some(
        div()
            .p(px(8.))
            .rounded(px(6.))
            .bg(colors.sunken)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.subtle)
                    .child(if video {
                        t(cx, "inspector.exif.video_title")
                    } else {
                        t(cx, "inspector.exif.photo_title")
                    }),
            )
            .children(rows.into_iter().map(|(label, value)| {
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .child(
                        div()
                            .w(px(48.))
                            .flex_none()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child(label),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .text_xs()
                            .text_color(colors.fg)
                            .child(value),
                    )
            }))
            .into_any_element(),
    )
}

impl KanalibApp {
    pub(crate) fn render_preview_inspector(
        &self,
        this: &Entity<Self>,
        selected: Option<&Asset>,
        source_aliases: &HashMap<PathBuf, String>,
        width: Pixels,
        colors: UiColors,
        cx: &App,
    ) -> impl IntoElement {
        let content: AnyElement = if let Some(asset) = selected {
            let preview: AnyElement = match asset.thumb.as_ref().or(Some(&asset.path)) {
                Some(path) => img(path.clone())
                    .image_cache(&self.thumb_cache)
                    .w_full()
                    .max_h(px(420.))
                    .object_fit(ObjectFit::Contain)
                    .into_any_element(),
                None => div()
                    .h(px(240.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(t(cx, "inspector.no_preview"))
                    .into_any_element(),
            };
            let this_quick = this.clone();
            let path_for_open = asset.path.clone();
            let open_toaster = self.toaster.clone();
            let open_state = self.state.clone();
            let open_remote = is_remote_asset(&asset.path);
            let open_label: SharedString = if open_remote {
                t(cx, "inspector.open.remote")
            } else if asset.is_video() {
                t(cx, "inspector.open.video")
            } else {
                t(cx, "inspector.open.file")
            };
            let similar_this = this.clone();
            let similar_path = asset.path.clone();
            let compare_this = this.clone();
            let favorite = asset.favorite;
            let star = asset.star;
            let rating = asset.rating.to_uppercase();
            let source_text = source_label(&asset.library_root, source_aliases);
            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(preview)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(asset.title()),
                )
                .child(div().text_xs().text_color(colors.muted).child(format!(
                    "{} × {} · {}",
                    asset.width,
                    asset.height,
                    format_bytes(asset.bytes)
                )))
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(t_args(
                            cx,
                            "inspector.info.source",
                            &[("source", source_text.as_str())],
                        )),
                )
                .children(open_remote.then(|| {
                    div()
                        .text_xs()
                        .text_color(colors.warning)
                        .child(t(cx, "inspector.remote_readonly_note"))
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .text_xs()
                        .text_color(colors.subtle)
                        .children(favorite.then(|| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(3.))
                                .child(
                                    Icon::new(IconName::Heart)
                                        .size(IconSize::Xs)
                                        .color(colors.danger),
                                )
                                .child(t(cx, "inspector.favorite.badge"))
                        }))
                        .child(if star > 0 {
                            format!("★ {star}")
                        } else {
                            t(cx, "inspector.info.stars_none").to_string()
                        })
                        .child(if rating.is_empty() {
                            t(cx, "inspector.info.rating_none").to_string()
                        } else {
                            t_args(cx, "inspector.info.rating", &[("rating", rating.as_str())])
                                .to_string()
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            Button::new("preview-quick-view", t(cx, "inspector.action.quick_view"))
                                .size(ControlSize::Sm)
                                .icon(IconName::Maximize2)
                                .on_click(move |_, _, cx| {
                                    let _ =
                                        this_quick.update(cx, |app, cx| app.open_quick_view(cx));
                                }),
                        )
                        .child(
                            Button::new("preview-open-file", open_label)
                                .size(ControlSize::Sm)
                                .disabled(open_remote)
                                .on_click(move |_, _, cx| {
                                    if let Err(error) = open_asset_with_system(&path_for_open) {
                                        open_state.set_ui_error(
                                            t_args(
                                                cx,
                                                "inspector.open.failed",
                                                &[("error", error.as_str())],
                                            )
                                            .to_string(),
                                        );
                                        push_toast(&open_toaster, cx, error, Severity::Danger);
                                    }
                                }),
                        )
                        .child(
                            Button::new(
                                "preview-find-similar",
                                t(cx, "inspector.action.find_similar"),
                            )
                            .size(ControlSize::Sm)
                            .icon(IconName::Search)
                            .on_click(move |_, _, cx| {
                                let _ = similar_this.update(cx, |app, cx| {
                                    app.open_similar(similar_path.clone(), cx);
                                });
                            }),
                        )
                        .child(
                            Button::new("preview-compare", t(cx, "inspector.action.compare"))
                                .size(ControlSize::Sm)
                                .icon(IconName::Columns2)
                                .on_click(move |_, _, cx| {
                                    let _ = compare_this
                                        .update(cx, |app, cx| app.open_compare_selected(cx));
                                }),
                        ),
                )
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .text_sm()
                .text_color(colors.muted)
                .child(t(cx, "inspector.empty.select"))
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(t(cx, "inspector.empty.preview_hint")),
                )
                .into_any_element()
        };

        div()
            .w(width)
            .flex_none()
            .h_full()
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border)
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .id("preview-inspector-scroll")
            .overflow_y_scroll()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(t(cx, "inspector.preview.title")),
            )
            .child(content)
    }

    pub(crate) fn render_inspector(
        &self,
        this: &Entity<Self>,
        selected: Option<&Asset>,
        blocked_tags: &HashSet<String>,
        source_aliases: &HashMap<PathBuf, String>,
        width: Pixels,
        colors: UiColors,
        cx: &App,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let this = this.clone();
        let content: AnyElement = if let Some(asset) = selected {
            let preview: AnyElement = match asset.thumb.as_ref().or(Some(&asset.path)) {
                Some(path) => img(path.clone())
                    .image_cache(&self.thumb_cache)
                    .w_full()
                    .h(px(230.))
                    .object_fit(ObjectFit::Contain)
                    .into_any_element(),
                None => div()
                    .h(px(230.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(t(cx, "inspector.no_preview"))
                    .into_any_element(),
            };

            let path = asset.path.clone();
            let repair_this = this.clone();
            let repair_path = asset.path.clone();
            let export_state = state.clone();
            let export_this = this.clone();
            let export_path = asset.path.clone();
            let manual_initial = asset
                .manual_tags
                .iter()
                .map(|tag| normalize_tag_name(&tag.tag))
                .collect::<HashSet<_>>();
            let manual_names = asset
                .manual_tags
                .iter()
                .map(|tag| SharedString::from(tag.tag.clone()))
                .collect::<Vec<_>>();
            let mut suggestion_entries = if self.tag_suggestions.is_empty() {
                let mut entries = self
                    .tag_vocab
                    .iter()
                    .filter(|tag| tag.count > 0 && !blocked_tags.contains(&tag.name))
                    .take(40)
                    .map(|tag| (tag.name.clone(), tag.count))
                    .collect::<Vec<_>>();
                for tag in asset.all_tags() {
                    if !entries
                        .iter()
                        .any(|(name, _)| name.eq_ignore_ascii_case(&tag.tag))
                    {
                        entries.push((tag.tag.clone(), 0));
                    }
                }
                entries
            } else {
                self.tag_suggestions.clone()
            };
            suggestion_entries.truncate(60);
            let suggestions = suggestion_entries
                .into_iter()
                .map(|(name, _)| Choice::new(name.clone(), name))
                .collect::<Vec<_>>();
            let manual_state = state.clone();
            let manual_this = this.clone();
            let manual_toaster = self.toaster.clone();
            let manual_path = path.clone();
            let typing_this = this.clone();
            let manual_input = TagInput::new(
                SharedString::from(format!("manual-tags-{}", path.display())),
                manual_names,
            )
            .placeholder(t(cx, "inspector.tags.placeholder"))
            .check(|value| !normalize_tag_name(value).is_empty())
            .suggestions(suggestions)
            .on_typing(move |typed, _, cx| {
                let _ = typing_this.update(cx, |app, cx| {
                    app.update_tag_suggestions(&typed);
                    cx.notify();
                });
            })
            .on_change(move |next, _, cx| {
                let next_set = next
                    .iter()
                    .map(|value| normalize_tag_name(value))
                    .filter(|value| !value.is_empty())
                    .collect::<HashSet<_>>();
                let added = next_set
                    .difference(&manual_initial)
                    .cloned()
                    .collect::<Vec<_>>();
                let removed = manual_initial
                    .difference(&next_set)
                    .cloned()
                    .collect::<Vec<_>>();
                let mut changed = 0usize;
                let mut failure = None;
                for tag in &added {
                    match manual_state.add_tags(std::slice::from_ref(&manual_path), tag) {
                        Ok(updated) => changed += updated,
                        Err(error) => {
                            failure = Some(format!("{error:#}"));
                            break;
                        }
                    }
                }
                if failure.is_none() {
                    for tag in &removed {
                        match manual_state.remove_tags(std::slice::from_ref(&manual_path), tag) {
                            Ok(updated) => changed += updated,
                            Err(error) => {
                                failure = Some(format!("{error:#}"));
                                break;
                            }
                        }
                    }
                }
                if let Some(error) = failure {
                    let message = t_args(
                        cx,
                        "inspector.tags.update_failed",
                        &[("error", error.as_str())],
                    )
                    .to_string();
                    manual_state.set_error(message.clone());
                    push_toast(&manual_toaster, cx, message, Severity::Danger);
                } else if changed > 0 {
                    let message = t(cx, "inspector.tags.updated");
                    push_toast(&manual_toaster, cx, message, Severity::Success);
                } else if !added.is_empty() || !removed.is_empty() {
                    let message = t(cx, "inspector.tags.unchanged");
                    push_toast(&manual_toaster, cx, message, Severity::Warning);
                }
                let _ = manual_this.update(cx, |_, cx| cx.notify());
            });

            let mut tags = div().id("inspector-tags").flex().flex_col().gap(px(2.));
            for category in [
                "general",
                "character",
                "copyright",
                "style",
                "meta",
                "rating",
            ] {
                let hits: Vec<&TagHit> = asset
                    .tags
                    .iter()
                    .filter(|hit| hit.category == category)
                    .take(if category == "general" { 12 } else { 6 })
                    .collect();
                if hits.is_empty() {
                    continue;
                }
                tags = tags.child(
                    div()
                        .mt(px(6.))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.subtle)
                        .child(tag_category_label(cx, category)),
                );
                for hit in hits {
                    let filter = Filter::Tag(hit.tag.clone());
                    let filter_this = this.clone();
                    let remove_state = state.clone();
                    let remove_this = this.clone();
                    let remove_toaster = self.toaster.clone();
                    let remove_path = path.clone();
                    let tag = hit.tag.clone();
                    let remove_tag = tag.clone();
                    let score = hit.score;
                    tags = tags.child(
                        div()
                            .id(SharedString::from(format!(
                                "inspector-tag-{}-{}",
                                category, tag
                            )))
                            .flex()
                            .items_center()
                            .justify_between()
                            .px(px(6.))
                            .py(px(3.))
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(move |style| style.bg(colors.hover))
                            .on_click(move |_, _, cx| {
                                let filter = filter.clone();
                                let _ = filter_this.update(cx, |app, cx| {
                                    app.set_filter(filter, cx);
                                });
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(colors.fg)
                                    .child(format!("#{tag}")),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(colors.subtle)
                                            .child(format!("{score:.3}")),
                                    )
                                    .child(
                                        IconButton::new(
                                            SharedString::from(format!(
                                                "remove-tag-{}-{}",
                                                category, tag
                                            )),
                                            IconName::X,
                                        )
                                        .size(ControlSize::Sm)
                                        .tooltip(t(cx, "inspector.tags.remove"))
                                        .on_click(
                                            move |_, _, cx| {
                                                cx.stop_propagation();
                                                match remove_state.remove_tags(
                                                    std::slice::from_ref(&remove_path),
                                                    &remove_tag,
                                                ) {
                                                    Ok(updated) => {
                                                        let message = if updated > 0 {
                                                            t_args(
                                                                cx,
                                                                "inspector.tags.removed",
                                                                &[("name", remove_tag.as_str())],
                                                            )
                                                        } else {
                                                            t(cx, "inspector.tags.none_removed")
                                                        };
                                                        push_toast(
                                                            &remove_toaster,
                                                            cx,
                                                            message,
                                                            if updated > 0 {
                                                                Severity::Success
                                                            } else {
                                                                Severity::Warning
                                                            },
                                                        )
                                                    }
                                                    Err(error) => {
                                                        let error_text = format!("{error:#}");
                                                        let message = t_args(
                                                            cx,
                                                            "inspector.tags.remove_failed",
                                                            &[("error", error_text.as_str())],
                                                        );
                                                        push_toast(
                                                            &remove_toaster,
                                                            cx,
                                                            message,
                                                            Severity::Danger,
                                                        )
                                                    }
                                                }
                                                let _ = remove_this.update(cx, |_, cx| cx.notify());
                                            },
                                        ),
                                    ),
                            ),
                    );
                }
            }

            let path_for_open = path.clone();
            let open_toaster = self.toaster.clone();
            let open_state = self.state.clone();
            let open_remote = is_remote_asset(&path);
            let open_label: SharedString = if open_remote {
                t(cx, "inspector.open.remote")
            } else if asset.is_video() {
                t(cx, "inspector.open.video")
            } else {
                t(cx, "inspector.open.file")
            };
            let open = Button::new("open-file", open_label)
                .size(ControlSize::Sm)
                .disabled(open_remote)
                .on_click(move |_, _, cx| {
                    if let Err(error) = open_asset_with_system(&path_for_open) {
                        // F-073 6.2：toast 之外还要有常驻的就地说明。
                        open_state.set_ui_error(
                            t_args(cx, "inspector.open.failed", &[("error", error.as_str())])
                                .to_string(),
                        );
                        push_toast(&open_toaster, cx, error, Severity::Danger);
                    }
                });
            // 动画不内置播放器：明确提供“系统应用播放”出口，不伪造逐帧能力。
            let animation_open = !open_remote && crate::format::is_animation_container(&path);
            let animation_path = path.clone();
            let animation_toaster = self.toaster.clone();
            let animation_state = self.state.clone();
            let animation_button = animation_open.then(|| {
                Button::new(
                    "inspector-open-animation",
                    t(cx, "inspector.action.play_system"),
                )
                .size(ControlSize::Sm)
                .icon(IconName::Play)
                .on_click(move |_, _, cx| {
                    if let Err(error) = open_asset_with_system(&animation_path) {
                        animation_state.set_ui_error(
                            t_args(
                                cx,
                                "inspector.action.play_failed",
                                &[("error", error.as_str())],
                            )
                            .to_string(),
                        );
                        push_toast(&animation_toaster, cx, error, Severity::Danger);
                    }
                })
            });
            let this_quick = this.clone();
            let quick = Button::new("quick-view", t(cx, "inspector.action.quick_view"))
                .size(ControlSize::Sm)
                .icon(IconName::Maximize2)
                .on_click(move |_, _, cx| {
                    let _ = this_quick.update(cx, |app, cx| app.open_quick_view(cx));
                });
            let similar_this = this.clone();
            let similar_path = path.clone();
            let find_similar = Button::new(
                "inspector-find-similar",
                t(cx, "inspector.action.find_similar"),
            )
            .size(ControlSize::Sm)
            .icon(IconName::Search)
            .on_click(move |_, _, cx| {
                let _ = similar_this.update(cx, |app, cx| {
                    app.open_similar(similar_path.clone(), cx);
                });
            });
            let compare_this = this.clone();
            let compare = Button::new("inspector-compare", t(cx, "inspector.action.compare"))
                .size(ControlSize::Sm)
                .icon(IconName::Columns2)
                .on_click(move |_, _, cx| {
                    let _ = compare_this.update(cx, |app, cx| app.open_compare_selected(cx));
                });
            let favorite = asset.favorite;
            let favorite_state = state.clone();
            let favorite_this = this.clone();
            let favorite_toaster = self.toaster.clone();
            let favorite_path = path.clone();
            let mut favorite_button = Button::new(
                SharedString::from(format!("inspector-favorite-{}", path.display())),
                if favorite {
                    t(cx, "inspector.favorite.marked")
                } else {
                    t(cx, "inspector.favorite.add")
                },
            )
            .size(ControlSize::Sm)
            .icon(IconName::Heart)
            .on_click(move |_, _, cx| {
                let changed = match favorite_state
                    .set_favorite(std::slice::from_ref(&favorite_path), !favorite)
                {
                    Ok(updated) => {
                        let count = updated.to_string();
                        let message = if favorite {
                            t_args(cx, "inspector.favorite.removed", &[("n", count.as_str())])
                        } else {
                            t_args(cx, "inspector.favorite.added", &[("n", count.as_str())])
                        };
                        push_toast(&favorite_toaster, cx, message, Severity::Success);
                        true
                    }
                    Err(error) => {
                        let error_text = format!("{error:#}");
                        let message = t_args(
                            cx,
                            "inspector.favorite.failed",
                            &[("error", error_text.as_str())],
                        );
                        favorite_state.set_error(message.to_string());
                        push_toast(&favorite_toaster, cx, message, Severity::Danger);
                        false
                    }
                };
                if changed {
                    let path = favorite_path.clone();
                    let became_favorite = !favorite;
                    let _ = favorite_this.update(cx, |app, cx| {
                        app.after_favorite_change(&path, became_favorite, cx)
                    });
                }
            });
            if favorite {
                favorite_button = favorite_button.primary();
            }
            let star_state = state.clone();
            let star_this = this.clone();
            let star_toaster = self.toaster.clone();
            let star_path = path.clone();
            let rating_control = Rating::new(
                SharedString::from(format!("inspector-star-{}", path.display())),
                asset.star,
            )
            .on_change(move |value, _, cx| {
                match star_state.set_star(std::slice::from_ref(&star_path), value) {
                    Ok(updated) => {
                        let count = updated.to_string();
                        let message = if value == 0 {
                            t_args(cx, "inspector.rating.cleared", &[("n", count.as_str())])
                        } else {
                            let stars = value.to_string();
                            t_args(
                                cx,
                                "inspector.rating.set",
                                &[("n", count.as_str()), ("stars", stars.as_str())],
                            )
                        };
                        push_toast(&star_toaster, cx, message, Severity::Success);
                    }
                    Err(error) => {
                        let error_text = format!("{error:#}");
                        let message = t_args(
                            cx,
                            "inspector.rating.failed",
                            &[("error", error_text.as_str())],
                        );
                        star_state.set_error(message.to_string());
                        push_toast(&star_toaster, cx, message, Severity::Danger);
                    }
                }
                let _ = star_this.update(cx, |_, cx| cx.notify());
            });
            let prompt = prompt_for(asset);
            let prompt_empty = prompt.is_empty();
            let prompt_count = asset
                .tags
                .iter()
                .filter(|tag| {
                    matches!(tag.category.as_str(), "general" | "character" | "copyright")
                })
                .count();
            let prompt_toaster = self.toaster.clone();
            let copy_prompt = Button::new("copy-prompt", t(cx, "inspector.copy.prompt"))
                .size(ControlSize::Sm)
                .disabled(prompt_empty)
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(prompt.clone()));
                    let count = prompt_count.to_string();
                    let message =
                        t_args(cx, "inspector.copy.prompt_done", &[("n", count.as_str())]);
                    push_toast(&prompt_toaster, cx, message, Severity::Success);
                });
            let copy_path_text = path.display().to_string();
            let copy_path_toaster = self.toaster.clone();
            let copy_path = Button::new("copy-path", t(cx, "inspector.copy.path"))
                .size(ControlSize::Sm)
                .icon(IconName::Copy)
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_path_text.clone()));
                    let message = t(cx, "inspector.copy.path_done");
                    push_toast(&copy_path_toaster, cx, message, Severity::Success);
                });
            let copy_image_this = this.clone();
            let copy_image_path = path.clone();
            let copy_image = Button::new("copy-image", t(cx, "inspector.copy.image"))
                .size(ControlSize::Sm)
                .icon(IconName::Image)
                .on_click(move |_, _, cx| {
                    let _ = copy_image_this.update(cx, |app, cx| {
                        app.copy_image_path(copy_image_path.clone(), cx);
                    });
                });

            let has_issue = asset.error.is_some() || asset.damage.is_some();
            let issue = has_issue.then(|| {
                let repair = Button::new("repair-asset", t(cx, "inspector.issue.repair"))
                    .size(ControlSize::Sm)
                    .on_click(move |_, _, cx| {
                        let path = repair_path.clone();
                        let _ = repair_this.update(cx, |app, cx| {
                            if let Err(error) = app.state.repair_asset(path) {
                                let error_text = format!("{error:#}");
                                app.state.set_error(
                                    t_args(
                                        cx,
                                        "inspector.issue.repair_failed",
                                        &[("error", error_text.as_str())],
                                    )
                                    .to_string(),
                                );
                            }
                            cx.notify();
                        });
                    });
                let export_title = t(cx, "inspector.issue.export_title").to_string();
                let png_filter = t(cx, "inspector.issue.export_png").to_string();
                let jpeg_filter = t(cx, "inspector.issue.export_jpeg").to_string();
                let export = Button::new("export-repair", t(cx, "inspector.issue.export"))
                    .size(ControlSize::Sm)
                    .on_click(move |_, _, cx| {
                        let state = export_state.clone();
                        let path = export_path.clone();
                        let weak = export_this.downgrade();
                        let export_title = export_title.clone();
                        let png_filter = png_filter.clone();
                        let jpeg_filter = jpeg_filter.clone();
                        cx.spawn(async move |cx| {
                            let stem = path
                                .file_stem()
                                .map(|value| value.to_string_lossy().to_string())
                                .unwrap_or_else(|| "image".to_string());
                            let mut dialog = rfd::AsyncFileDialog::new()
                                .set_title(export_title)
                                .set_file_name(format!("{stem}-repaired.png"))
                                .add_filter(png_filter, &["png"])
                                .add_filter(jpeg_filter, &["jpg", "jpeg"]);
                            if let Some(parent) = path.parent() {
                                dialog = dialog.set_directory(parent);
                            }
                            let Some(handle) = dialog.save_file().await else {
                                return;
                            };
                            let output = handle.path().to_path_buf();
                            let result = state.export_repaired_copy(path.clone(), output);
                            let _ = weak.update(cx, |app, cx| {
                                if let Err(error) = result {
                                    let error_text = format!("{error:#}");
                                    app.state.set_error(
                                        t_args(
                                            cx,
                                            "inspector.issue.export_failed",
                                            &[("error", error_text.as_str())],
                                        )
                                        .to_string(),
                                    );
                                }
                                cx.notify();
                            });
                        })
                        .detach();
                    });
                let mut panel = div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(8.))
                    .rounded(px(6.))
                    .bg(colors.sunken)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.subtle)
                            .child(t(cx, "inspector.issue.title")),
                    );
                if let Some(damage) = &asset.damage {
                    panel = panel.child(
                        div()
                            .text_xs()
                            .text_color(colors.warning)
                            .child(damage.clone()),
                    );
                }
                if let Some(error) = &asset.error {
                    let detail = friendly_media_error(error);
                    panel = panel.child(div().text_xs().text_color(colors.danger).child(t_args(
                        cx,
                        "inspector.issue.tagging_failed",
                        &[("error", detail.as_str())],
                    )));
                }
                panel
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child(t(cx, "inspector.issue.hint")),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .child(repair)
                            .child(export),
                    )
            });

            let albums = self.asset_album_names(&asset.path);
            let album_path = asset.path.clone();
            let album_this = this.clone();
            let mut album_section = div().flex().flex_col().gap(px(4.)).child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(colors.subtle)
                            .child(t(cx, "inspector.album.section")),
                    )
                    .children((!albums.is_empty()).then(|| {
                        let mut menu = Menu::new();
                        let mut entries = self.albums_snapshot();
                        entries.sort_by_key(|a| a.name.to_lowercase());
                        for album in entries {
                            let add_this = album_this.clone();
                            let add_path = album_path.clone();
                            menu = menu.item(
                                MenuItem::new(album.name.clone())
                                    .icon(IconName::Folder)
                                    .on_click(move |_, cx| {
                                        let _ = add_this.update(cx, |app, cx| {
                                            app.add_paths_to_album(
                                                album.id,
                                                vec![add_path.clone()],
                                                cx,
                                            );
                                        });
                                    }),
                            );
                        }
                        OverflowMenu::new(
                            SharedString::from(format!(
                                "inspector-album-more-{}",
                                album_path.display()
                            )),
                            menu,
                        )
                        .tooltip(t(cx, "inspector.album.add"))
                    })),
            );
            if albums.is_empty() {
                album_section = album_section.child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(t(cx, "inspector.album.empty")),
                );
            } else {
                let mut chips = div().flex().flex_wrap().gap(px(4.));
                for album in albums {
                    let remove_this = this.clone();
                    let remove_id = album.id;
                    let chip_label = album.name.clone();
                    let remove_path = album_path.clone();
                    chips = chips.child(
                        div()
                            .id(SharedString::from(format!(
                                "inspector-album-{}-{}",
                                album.id,
                                album_path.display()
                            )))
                            .flex()
                            .items_center()
                            .gap(px(2.))
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(colors.sunken)
                            .child(div().text_xs().text_color(colors.fg).child(chip_label))
                            .child(
                                IconButton::new(
                                    SharedString::from(format!(
                                        "inspector-album-remove-{}-{}",
                                        album.id,
                                        album_path.display()
                                    )),
                                    IconName::X,
                                )
                                .size(ControlSize::Sm)
                                .tooltip(t(cx, "inspector.album.remove"))
                                .on_click(move |_, _, cx| {
                                    let path = remove_path.clone();
                                    let _ = remove_this.update(cx, |app, cx| {
                                        app.remove_path_from_album(remove_id, path.clone(), cx);
                                    });
                                }),
                            ),
                    );
                }
                album_section = album_section.child(chips);
            }

            let rating_text = if asset.rating.is_empty() {
                t(cx, "inspector.info.rating_none").to_string()
            } else {
                asset.rating.clone()
            };
            let group_text = localize_group_name(cx, &asset.group);
            let source_text = source_label(&asset.library_root, source_aliases);
            let format_text = friendly_format_label(&asset.path);
            let capability_text = format_capability_note(&asset.path);
            let sha_line = match (&asset.sha1, &asset.blake3) {
                (Some(sha1), Some(blake3)) => {
                    let phash_note = match asset.phash {
                        Some(phash) => format!(" · pHash {phash:016x}"),
                        None if crate::format::unsupported_format_note(&asset.path).is_some() => {
                            format!(" · {}", t(cx, "inspector.hash.phash_unsupported"))
                        }
                        None if asset
                            .path
                            .extension()
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("psd")) =>
                        {
                            format!(" · {}", t(cx, "inspector.hash.phash_psd"))
                        }
                        None => format!(" · {}", t(cx, "inspector.hash.phash_failed")),
                    };
                    format!(
                        "SHA1 {} · B3 {}{}",
                        &sha1[..12.min(sha1.len())],
                        &blake3[..8.min(blake3.len())],
                        phash_note
                    )
                }
                _ => t(cx, "inspector.hash.none").to_string(),
            };

            div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(preview)
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(asset.title()),
                )
                .child(div().text_xs().text_color(colors.muted).child(format!(
                    "{} × {} · {} · {}",
                    asset.width, asset.height, rating_text, group_text
                )))
                .child(div().text_xs().text_color(colors.subtle).child(format!(
                    "{} · {}",
                    format_bytes(asset.bytes),
                    format_date(asset.mtime)
                )))
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(t_args(
                            cx,
                            "inspector.info.source",
                            &[("source", source_text.as_str())],
                        )),
                )
                .children(open_remote.then(|| {
                    div()
                        .text_xs()
                        .text_color(colors.warning)
                        .child(t(cx, "inspector.remote_readonly_note"))
                }))
                .child(
                    div()
                        .text_xs()
                        .text_color(
                            if is_ffmpeg_only(&asset.path)
                                || crate::format::unsupported_format_note(&asset.path).is_some()
                            {
                                colors.warning
                            } else {
                                colors.subtle
                            },
                        )
                        .child(t_args(
                            cx,
                            "inspector.info.format",
                            &[
                                ("format", format_text.as_str()),
                                ("note", capability_text.as_str()),
                            ],
                        )),
                )
                .children(crate::format::format_limits_note(&asset.path).map(|note| {
                    div()
                        .id("inspector-format-note")
                        .text_xs()
                        .text_color(colors.muted)
                        .child(note)
                }))
                .child(album_section)
                .children(exif_panel(asset, colors, cx))
                .child(div().text_xs().text_color(colors.subtle).child(sha_line))
                .children(issue)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(favorite_button)
                        .child(quick)
                        .child(find_similar)
                        .child(compare)
                        .child(open)
                        .children(animation_button),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .text_color(colors.subtle)
                                .child(t(cx, "inspector.rating.label")),
                        )
                        .child(rating_control),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(copy_prompt)
                        .child(copy_path)
                        .child(copy_image),
                )
                .child(
                    div()
                        .mt(px(4.))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.subtle)
                        .child(t(cx, "inspector.tags.manual_heading")),
                )
                .child(manual_input)
                .child(
                    div()
                        .mt(px(4.))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.subtle)
                        .child(t(cx, "inspector.tags.auto_heading")),
                )
                .child(tags)
                .into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .text_sm()
                .text_color(colors.muted)
                .child(t(cx, "inspector.empty.select"))
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(t(cx, "inspector.empty.details_hint")),
                )
                .into_any_element()
        };

        div()
            .w(width)
            .flex_none()
            .h_full()
            .bg(colors.surface)
            .border_1()
            .border_color(colors.border)
            .p(px(14.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .id("inspector-scroll")
            .overflow_y_scroll()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(t(cx, "inspector.title")),
            )
            .child(content)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn friendly_format_label_maps_known_extensions() {
        assert_eq!(super::friendly_format_label(Path::new("a.TIFF")), "TIFF");
        assert_eq!(super::friendly_format_label(Path::new("b.avif")), "AVIF");
        assert_eq!(super::friendly_format_label(Path::new("c.qoi")), "QOI");
        assert_eq!(super::friendly_format_label(Path::new("d.psd")), "PSD");
        assert_eq!(
            super::friendly_format_label(Path::new("e.HEIC")),
            "HEIC/HEIF"
        );
        assert_eq!(super::friendly_format_label(Path::new("f.cr2")), "RAW");
        assert_eq!(super::friendly_format_label(Path::new("clip.MP4")), "MP4");
        assert_eq!(super::friendly_format_label(Path::new("clip.mov")), "MOV");
        assert_eq!(super::friendly_format_label(Path::new("clip.mkv")), "MKV");
        assert_eq!(super::friendly_format_label(Path::new("clip.webm")), "WebM");
        assert_eq!(super::friendly_format_label(Path::new("clip.avi")), "AVI");
        assert_eq!(super::friendly_format_label(Path::new("d.xyz")), "XYZ");
        assert_eq!(super::friendly_format_label(Path::new("noext")), "未知格式");
    }

    #[test]
    fn friendly_media_error_points_avif_at_ffmpeg() {
        assert!(
            super::friendly_media_error("The image format Avif is not supported")
                .contains("FFmpeg")
        );
        assert!(super::friendly_media_error("ffmpeg exited with code 1").contains("FFmpeg"));
        assert!(super::friendly_media_error("unexpected end of file").contains("截断"));
        assert!(super::friendly_media_error("unsupported color type").contains("不支持"));
    }

    #[test]
    fn format_capability_note_marks_ffmpeg_only_formats() {
        assert_eq!(
            super::format_capability_note(Path::new("a.png")),
            if crate::localization::is_english() { "Built-in decoder".to_string() } else { "内置解码".to_string() }
        );
        assert!(super::format_capability_note(Path::new("a.avif")).contains("需要 FFmpeg 解码"));
        let psd = super::format_capability_note(Path::new("a.PSD"));
        assert!(psd.contains("需要 FFmpeg 解码"), "{psd}");
        assert!(psd.contains("仅合成图"), "{psd}");
        let gif = super::format_capability_note(Path::new("a.gif"));
        assert!(gif.contains("动画仅首帧"), "{gif}");
        let heic = super::format_capability_note(Path::new("a.heic"));
        assert!(heic.contains("未接入"), "{heic}");
        assert!(!heic.contains("内置解码"), "{heic}");
        let raw = super::format_capability_note(Path::new("a.nef"));
        assert!(raw.contains("未接入"), "{raw}");
        let video = super::format_capability_note(Path::new("clip.MP4"));
        assert!(video.contains("首帧"), "{video}");
        assert!(video.contains("系统播放器"), "{video}");
    }

    #[test]
    fn friendly_media_error_covers_psd_and_heic() {
        let psd = super::friendly_media_error("ffmpeg psd decoder failed: unsupported format");
        assert!(psd.contains("PSD") && psd.contains("FFmpeg"), "{psd}");
        let heic = super::friendly_media_error("libheif: no decoder for heic");
        assert!(heic.contains("HEIC") && heic.contains("不支持"), "{heic}");
    }
}
