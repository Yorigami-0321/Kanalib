use crate::*;
use crate::app::{KanalibApp, prompt_for, push_toast};
use crate::format::*;
use crate::platform::*;
use crate::sidebar::source_label;
use crate::theme::*;

pub(crate) fn friendly_media_error(message: &str) -> String {
    let text = message.to_lowercase();
    if text.contains("avif")
        && (text.contains("unsupported")
            || text.contains("unknown format")
            || text.contains("not supported")
            || text.contains("decoder")
            || text.contains("cannot identify"))
    {
        "AVIF 需要 FFmpeg 解码；未检测到 FFmpeg 时无法生成缩略图".to_string()
    } else if text.contains("ffmpeg") || text.contains("dav1d") {
        "FFmpeg 解码失败，缩略图与打标可能不可用".to_string()
    } else if text.contains("truncated") || text.contains("unexpected end of file") {
        "文件不完整（图片数据被截断）".to_string()
    } else if text.contains("cannot identify image file")
        || text.contains("unidentifiedimageerror")
    {
        "无法识别的图片，文件可能已损坏".to_string()
    } else if text.contains("decompression bomb") {
        "图片尺寸超出安全上限".to_string()
    } else if text.contains("unsupported") || text.contains("unknown format") {
        "不支持的图片编码".to_string()
    } else {
        message.to_string()
    }
}

/// 扩展名对应的友好格式名；未知扩展名回退为大写原名。
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
        "" => "未知格式".to_string(),
        other => other.to_uppercase(),
    }
}

/// 解码能力提示：只有 AVIF 需要 FFmpeg，其余格式走内置解码。
pub(crate) fn format_capability_note(path: &Path) -> String {
    if is_ffmpeg_only(path) {
        if kanalib_core::ffmpeg::available() {
            "需要 FFmpeg 解码".to_string()
        } else {
            "需要 FFmpeg 解码 · 当前未检测到 FFmpeg".to_string()
        }
    } else {
        "内置解码".to_string()
    }
}

pub(crate) fn exif_panel(asset: &Asset, colors: UiColors) -> Option<AnyElement> {
    let meta = serde_json::from_str::<kanalib_core::media::ExifData>(&asset.metadata_json).ok()?;
    if meta.is_empty() && asset.captured_at.is_none() {
        return None;
    }
    let mut rows: Vec<(&'static str, String)> = Vec::new();
    if let Some(captured) = asset.captured_at {
        rows.push(("拍摄时间", format_date(captured)));
    }
    if !meta.camera.is_empty() {
        rows.push(("相机", meta.camera));
    }
    if !meta.lens.is_empty() {
        rows.push(("镜头", meta.lens));
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
        rows.push(("曝光", exposure));
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
                    .child("拍摄信息"),
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn friendly_format_label_maps_known_extensions() {
        assert_eq!(super::friendly_format_label(Path::new("a.TIFF")), "TIFF");
        assert_eq!(super::friendly_format_label(Path::new("b.avif")), "AVIF");
        assert_eq!(super::friendly_format_label(Path::new("c.qoi")), "QOI");
        assert_eq!(super::friendly_format_label(Path::new("d.xyz")), "XYZ");
        assert_eq!(super::friendly_format_label(Path::new("noext")), "未知格式");
    }

    #[test]
    fn friendly_media_error_points_avif_at_ffmpeg() {
        assert!(
            super::friendly_media_error("The image format Avif is not supported")
                .contains("FFmpeg")
        );
        assert!(
            super::friendly_media_error("ffmpeg exited with code 1")
                .contains("FFmpeg")
        );
        assert!(super::friendly_media_error("unexpected end of file").contains("截断"));
        assert!(super::friendly_media_error("unsupported color type").contains("不支持"));
    }

    #[test]
    fn format_capability_note_marks_ffmpeg_only_formats() {
        assert_eq!(
            super::format_capability_note(Path::new("a.png")),
            "内置解码".to_string()
        );
        assert!(
            super::format_capability_note(Path::new("a.avif")).contains("需要 FFmpeg 解码")
        );
    }

}

impl KanalibApp {
    pub(crate) fn render_preview_inspector(
        &self,
        this: &Entity<Self>,
        selected: Option<&Asset>,
        source_aliases: &HashMap<PathBuf, String>,
        width: Pixels,
        colors: UiColors,
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
                    .child("无预览")
                    .into_any_element(),
            };
            let this_quick = this.clone();
            let path_for_open = asset.path.clone();
            let similar_this = this.clone();
            let similar_path = asset.path.clone();
            let compare_this = this.clone();
            let favorite = asset.favorite;
            let star = asset.star;
            let rating = asset.rating.to_uppercase();
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
                        .child(format!(
                            "来源 · {}",
                            source_label(&asset.library_root, source_aliases)
                        )),
                )
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
                                .child("收藏")
                        }))
                        .child(if star > 0 {
                            format!("★ {star}")
                        } else {
                            "未设星级".to_string()
                        })
                        .child(if rating.is_empty() {
                            "未评级".to_string()
                        } else {
                            format!("评级 {rating}")
                        }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            Button::new("preview-quick-view", "快速查看")
                                .size(ControlSize::Sm)
                                .icon(IconName::Maximize2)
                                .on_click(move |_, _, cx| {
                                    let _ =
                                        this_quick.update(cx, |app, cx| app.open_quick_view(cx));
                                }),
                        )
                        .child(
                            Button::new("preview-open-file", "打开文件")
                                .size(ControlSize::Sm)
                                .on_click(move |_, _, _| open_path(&path_for_open)),
                        )
                        .child(
                            Button::new("preview-find-similar", "查找相似")
                                .size(ControlSize::Sm)
                                .icon(IconName::Search)
                                .on_click(move |_, _, cx| {
                                    let _ = similar_this.update(cx, |app, cx| {
                                        app.open_similar(similar_path.clone(), cx);
                                    });
                                }),
                        )
                        .child(
                            Button::new("preview-compare", "对比选中")
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
                .child("选择一张图片")
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child("预览窗格只显示大图与基础信息。"),
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
                    .child("预览"),
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
                    .child("无预览")
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
            .placeholder("添加标签，输入以补全全部词表")
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
                let added = next_set.difference(&manual_initial).cloned().collect::<Vec<_>>();
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
                    manual_state.set_error(format!("标签操作失败：{error}"));
                    push_toast(
                        &manual_toaster,
                        cx,
                        format!("标签操作失败：{error}"),
                        Severity::Danger,
                    );
                } else if changed > 0 {
                    push_toast(
                        &manual_toaster,
                        cx,
                        "标签已更新",
                        Severity::Success,
                    );
                } else if !added.is_empty() || !removed.is_empty() {
                    push_toast(
                        &manual_toaster,
                        cx,
                        "标签已存在或无需变更",
                        Severity::Warning,
                    );
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
                        .child(category.to_string()),
                );
                for hit in hits {
                    let filter = Filter::Tag(hit.tag.clone());
                    let filter_state = state.clone();
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
                                filter_state.set_filter(filter.clone());
                                let _ = filter_this.update(cx, |app, cx| {
                                    app.screen = Screen::Library;
                                    cx.notify();
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
                                        .tooltip("移除标签")
                                        .on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            match remove_state.remove_tags(
                                                std::slice::from_ref(&remove_path),
                                                &remove_tag,
                                            ) {
                                                Ok(updated) => push_toast(
                                                    &remove_toaster,
                                                    cx,
                                                    if updated > 0 {
                                                        format!("已移除标签 #{remove_tag}")
                                                    } else {
                                                        "没有可移除的标签".to_string()
                                                    },
                                                    if updated > 0 {
                                                        Severity::Success
                                                    } else {
                                                        Severity::Warning
                                                    },
                                                ),
                                                Err(error) => push_toast(
                                                    &remove_toaster,
                                                    cx,
                                                    format!("移除标签失败：{error:#}"),
                                                    Severity::Danger,
                                                ),
                                            }
                                            let _ = remove_this.update(cx, |_, cx| cx.notify());
                                        }),
                                    ),
                            ),
                    );
                }
            }

            let path_for_open = path.clone();
            let open = Button::new("open-file", "打开文件")
                .size(ControlSize::Sm)
                .on_click(move |_, _, _| {
                    open_path(&path_for_open);
                });
            let this_quick = this.clone();
            let quick = Button::new("quick-view", "快速查看")
                .size(ControlSize::Sm)
                .icon(IconName::Maximize2)
                .on_click(move |_, _, cx| {
                    let _ = this_quick.update(cx, |app, cx| app.open_quick_view(cx));
                });
            let similar_this = this.clone();
            let similar_path = path.clone();
            let find_similar = Button::new("inspector-find-similar", "查找相似")
                .size(ControlSize::Sm)
                .icon(IconName::Search)
                .on_click(move |_, _, cx| {
                    let _ = similar_this.update(cx, |app, cx| {
                        app.open_similar(similar_path.clone(), cx);
                    });
                });
            let compare_this = this.clone();
            let compare = Button::new("inspector-compare", "对比选中")
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
                if favorite { "已收藏" } else { "收藏" },
            )
            .size(ControlSize::Sm)
            .icon(IconName::Heart)
            .on_click(move |_, _, cx| {
                match favorite_state.set_favorite(std::slice::from_ref(&favorite_path), !favorite) {
                    Ok(updated) => push_toast(
                        &favorite_toaster,
                        cx,
                        if favorite {
                            format!("已取消收藏 {updated} 张")
                        } else {
                            format!("已收藏 {updated} 张")
                        },
                        Severity::Success,
                    ),
                    Err(error) => {
                        favorite_state.set_error(format!("收藏失败：{error:#}"));
                        push_toast(
                            &favorite_toaster,
                            cx,
                            format!("收藏失败：{error:#}"),
                            Severity::Danger,
                        );
                    }
                }
                let _ = favorite_this.update(cx, |_, cx| cx.notify());
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
                    Ok(updated) => push_toast(
                        &star_toaster,
                        cx,
                        if value == 0 {
                            format!("已清除 {updated} 张的星级")
                        } else {
                            format!("已设置 {updated} 张为 {value} 星")
                        },
                        Severity::Success,
                    ),
                    Err(error) => {
                        star_state.set_error(format!("设置星级失败：{error:#}"));
                        push_toast(
                            &star_toaster,
                            cx,
                            format!("设置星级失败：{error:#}"),
                            Severity::Danger,
                        );
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
            let copy_prompt = Button::new("copy-prompt", "复制 Prompt")
                .size(ControlSize::Sm)
                .disabled(prompt_empty)
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(prompt.clone()));
                    push_toast(
                        &prompt_toaster,
                        cx,
                        format!("已复制 {prompt_count} 个标签"),
                        Severity::Success,
                    );
                });
            let copy_path_text = path.display().to_string();
            let copy_path_toaster = self.toaster.clone();
            let copy_path = Button::new("copy-path", "复制路径")
                .size(ControlSize::Sm)
                .icon(IconName::Copy)
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(copy_path_text.clone()));
                    push_toast(&copy_path_toaster, cx, "已复制文件路径", Severity::Success);
                });

            let has_issue = asset.error.is_some() || asset.damage.is_some();
            let issue = has_issue.then(|| {
                let repair = Button::new("repair-asset", "容错重解")
                    .size(ControlSize::Sm)
                    .on_click(move |_, _, cx| {
                        let path = repair_path.clone();
                        let _ = repair_this.update(cx, |app, cx| {
                            if let Err(error) = app.state.repair_asset(path) {
                                app.state.set_error(format!("修复失败：{error:#}"));
                            }
                            cx.notify();
                        });
                    });
                let export = Button::new("export-repair", "导出修复副本…")
                    .size(ControlSize::Sm)
                    .on_click(move |_, _, cx| {
                        let state = export_state.clone();
                        let path = export_path.clone();
                        let weak = export_this.downgrade();
                        cx.spawn(async move |cx| {
                            let stem = path
                                .file_stem()
                                .map(|value| value.to_string_lossy().to_string())
                                .unwrap_or_else(|| "image".to_string());
                            let mut dialog = rfd::AsyncFileDialog::new()
                                .set_title("导出修复副本")
                                .set_file_name(format!("{stem}-repaired.png"))
                                .add_filter("PNG 图片", &["png"])
                                .add_filter("JPEG 图片", &["jpg", "jpeg"]);
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
                                    app.state
                                        .set_error(format!("导出修复副本失败：{error:#}"));
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
                            .child("文件异常"),
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
                    panel = panel.child(
                        div()
                            .text_xs()
                            .text_color(colors.danger)
                            .child(format!("打标失败：{}", friendly_media_error(error))),
                    );
                }
                panel
                    .child(
                        div()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child("修复只重建预览并重新打标，不改动原文件；导出会另存为新文件。"),
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
            let mut album_section = div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(colors.subtle)
                                .child("所属相册"),
                        )
                        .children((!albums.is_empty()).then(|| {
                            let mut menu = Menu::new();
                            let mut entries = self.albums_snapshot();
                            entries
                                .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
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
                            .tooltip("加入相册")
                        })),
                );
            if albums.is_empty() {
                album_section = album_section.child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child("未加入相册；用「加入相册」菜单归档，不移动文件"),
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
                                .tooltip("从该相册移出")
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
                    asset.width,
                    asset.height,
                    if asset.rating.is_empty() {
                        "未评级"
                    } else {
                        asset.rating.as_str()
                    },
                    asset.group
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
                        .child(format!(
                            "来源 · {}",
                            source_label(&asset.library_root, source_aliases)
                        )),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(if is_ffmpeg_only(&asset.path) {
                            colors.warning
                        } else {
                            colors.subtle
                        })
                        .child(format!(
                            "格式 · {} · {}",
                            friendly_format_label(&asset.path),
                            format_capability_note(&asset.path)
                        )),
                )
                .child(album_section)
                .children(exif_panel(asset, colors))
                .child(div().text_xs().text_color(colors.subtle).child(
                    match (&asset.sha1, &asset.blake3) {
                        (Some(sha1), Some(blake3)) => format!(
                            "SHA1 {} · B3 {}{}",
                            &sha1[..12.min(sha1.len())],
                            &blake3[..8.min(blake3.len())],
                            match asset.phash {
                                Some(phash) => format!(" · pHash {phash:016x}"),
                                None => " · pHash 未计算（解码失败）".to_string(),
                            }
                        ),
                        _ => "内容哈希未计算".to_string(),
                    },
                ))
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
                        .child(open),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_xs().text_color(colors.subtle).child("个人星级"))
                        .child(rating_control),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(copy_prompt)
                        .child(copy_path),
                )
                .child(
                    div()
                        .mt(px(4.))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.subtle)
                        .child("手工标签（回车添加，自动保存）"),
                )
                .child(manual_input)
                .child(
                    div()
                        .mt(px(4.))
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(colors.subtle)
                        .child("自动标签（点击筛选，× 移除）"),
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
                .child("选择一张图片")
                .child(
                    div()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child("自动标签与分类来自本地 PixAI Tagger。"),
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
                    .child("检查器"),
            )
            .child(content)
    }

}
