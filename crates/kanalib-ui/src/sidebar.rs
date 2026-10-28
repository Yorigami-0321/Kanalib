use crate::*;
use crate::app::{KanalibApp, push_toast};
use crate::platform::*;
use crate::theme::*;
use kanalib_core::model::{Collection, DuplicateNameError};

pub(crate) const SIDEBAR_MIN_WIDTH: f32 = 200.;
pub(crate) const SIDEBAR_MAX_WIDTH: f32 = 420.;
pub(crate) const SIDEBAR_DEFAULT_WIDTH: f32 = 260.;

/// 侧栏分区 id。注意：`collections` 是「智能集合」（`saved_searches` 表），
/// 手动相册用独立的 `albums`（`collections`/`collection_assets` 表），两者不可混用。
pub(crate) const SIDEBAR_SECTIONS: [&str; 8] = [
    "sources",
    "filters",
    "collections",
    "albums",
    "organize",
    "ratings",
    "groups",
    "tags",
];

/// 组织对象的新建/重命名/删除弹窗；命名冲突由核心 `DuplicateNameError` 判定。
#[derive(Clone, Debug)]
pub(crate) enum OrgDialog {
    RenameSavedSearch { id: i64, name: String },
    NewAlbum,
    RenameAlbum { id: i64, name: String },
    DeleteAlbum {
        id: i64,
        name: String,
        count: usize,
    },
}

/// 相册列表与「资产属于哪些相册」的只读缓存。
///
/// 侧栏/检查器每帧渲染都读它，只在 `shared.revision` 变化（增删资产、增删相册关系、
/// 重命名等）时才重新查库，避免每帧打开数据库。
pub(crate) struct AlbumCache {
    revision: u64,
    albums: Vec<Collection>,
    membership_revision: u64,
    membership: HashMap<PathBuf, Vec<Collection>>,
}

impl Default for AlbumCache {
    fn default() -> Self {
        Self {
            // `u64::MAX` 保证首帧就会加载一次。
            revision: u64::MAX,
            albums: Vec::new(),
            membership_revision: u64::MAX,
            membership: HashMap::new(),
        }
    }
}

pub(crate) fn canonicalize_org_name(name: &str) -> String {
    name.trim().to_string()
}

/// 同名冲突提示统一文案；判定始终来自核心 `DuplicateNameError`，不看错误字符串。
pub(crate) fn duplicate_name_message(name: &str) -> String {
    format!("名称「{}」已被占用；未覆盖已有条目，请换一个名称", name.trim())
}

pub(crate) fn source_label(root: &Path, aliases: &HashMap<PathBuf, String>) -> String {
    aliases
        .get(root)
        .filter(|alias| !alias.trim().is_empty())
        .cloned()
        .or_else(|| {
            root.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| root.display().to_string())
}

pub(crate) fn activity_item(
    target: ActivityTarget,
    icon: IconName,
    label: &'static str,
    active: bool,
    badge: usize,
    this: Entity<KanalibApp>,
    colors: UiColors,
) -> AnyElement {
    let this_click = this.clone();
    let item = div()
        .id(SharedString::from(format!("activity-{label}")))
        .role(Role::Button)
        .aria_label(label)
        .relative()
        .w_full()
        .h(px(40.))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(if active { colors.fg } else { colors.muted })
        .hover(move |style| style.bg(colors.hover))
        .on_click(move |_, window, cx| {
            let search = if target == ActivityTarget::Search {
                this_click.read(cx).search_input.clone()
            } else {
                None
            };
            let _ = this_click.update(cx, |app, cx| match target {
                ActivityTarget::Library => {
                    app.screen = Screen::Library;
                    app.dedup_confirm = None;
                    cx.notify();
                }
                ActivityTarget::Search => {
                    app.screen = Screen::Library;
                    app.dedup_confirm = None;
                    cx.notify();
                }
                ActivityTarget::Duplicates => {
                    app.open_dedup_mode(GroupKind::Exact, cx);
                }
                ActivityTarget::Tags => app.open_tag_manager(cx),
                ActivityTarget::Settings => app.open_settings(cx),
                ActivityTarget::Theme => app.toggle_theme(cx),
            });
            if let Some(search) = search {
                let handle = search.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
        })
        .child(Icon::new(icon).size(IconSize::Lg).color(if active {
            colors.accent
        } else {
            colors.muted
        }))
        .children(active.then(|| {
            div()
                .absolute()
                .left_0()
                .top(px(8.))
                .bottom(px(8.))
                .w(px(2.))
                .rounded_full()
                .bg(colors.accent)
        }))
        .children((badge > 0).then(|| {
            div()
                .absolute()
                .top(px(3.))
                .right(px(3.))
                .min_w(px(15.))
                .h(px(15.))
                .px(px(3.))
                .rounded_full()
                .bg(colors.danger)
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(white())
                .child(if badge > 99 {
                    "99+".to_string()
                } else {
                    badge.to_string()
                })
        }));
    item.into_any_element()
}

pub(crate) fn source_row(
    id: SharedString,
    root: PathBuf,
    label: String,
    count: usize,
    active: bool,
    status: Option<SourceScanStatus>,
    this: Entity<KanalibApp>,
    colors: UiColors,
) -> impl IntoElement {
    let filter = Filter::Source(root.clone());
    let this_click = this.clone();
    let this_right = this.clone();
    let root_right = root.clone();
    let (status_text, status_color) = match &status {
        Some(status) if !status.online => ("离线", colors.danger),
        Some(status) if status.error.is_some() => ("错误", colors.warning),
        _ => ("", colors.subtle),
    };
    let aria = match (status_text, status.as_ref().and_then(|status| status.error.as_deref())) {
        ("", _) => root.display().to_string(),
        (text, Some(error)) => format!("{}（{text}：{error}）", root.display()),
        (text, None) => format!("{}（{text}）", root.display()),
    };
    let mut row = div()
        .id(id)
        .role(Role::Button)
        .aria_label(aria)
        .flex()
        .items_center()
        .justify_between()
        .px(px(8.))
        .py(px(5.))
        .rounded(px(5.))
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover));
    if active {
        row = row.bg(colors.selection);
    }
    row.on_click(move |_, _, cx| {
        let _ = this_click.update(cx, |app, cx| {
            app.set_filter(filter.clone(), cx);
            app.dedup_confirm = None;
        });
    })
    .on_mouse_down(MouseButton::Right, move |event, window, cx| {
        window.prevent_default();
        let _ = this_right.update(cx, |app, cx| {
            app.source_menu_epoch = app.source_menu_epoch.wrapping_add(1);
            app.source_menu = Some(SourceMenuState {
                request: app.source_menu_epoch,
                root: root_right.clone(),
                position: event.position,
            });
            cx.notify();
        });
    })
    .child(
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .items_center()
            .gap(px(5.))
            .children((!status_text.is_empty()).then(|| {
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(3.))
                    .child(div().size(px(6.)).rounded_full().bg(status_color))
                    .child(
                        div()
                            .text_xs()
                            .text_color(status_color)
                            .child(status_text),
                    )
            }))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_xs()
                    .text_color(colors.fg)
                    .child(label),
            ),
    )
    .child(
        div()
            .flex_none()
            .text_xs()
            .text_color(colors.subtle)
            .child(count.to_string()),
    )
}

pub(crate) fn source_menu(root: &Path, this: Entity<KanalibApp>) -> Menu {
    let rename_this = this.clone();
    let rename_root = root.to_path_buf();
    let remove_this = this.clone();
    let remove_root = root.to_path_buf();
    let reveal_root = root.to_path_buf();
    let rescan_this = this.clone();
    let rescan_root = root.to_path_buf();
    Menu::new()
        .item(
            MenuItem::new("重命名…")
                .icon(IconName::Pencil)
                .on_click(move |_, cx| {
                    let _ = rename_this.update(cx, |app, cx| {
                        app.open_source_rename(rename_root.clone(), cx);
                    });
                }),
        )
        .item(
            MenuItem::new("重新扫描此来源")
                .icon(IconName::RefreshCw)
                .on_click(move |_, cx| {
                    let _ = rescan_this.update(cx, |app, cx| {
                        app.rescan_source(rescan_root.clone(), cx);
                    });
                }),
        )
        .item(
            MenuItem::new("在资源管理器中打开")
                .icon(IconName::FolderOpen)
                .on_click(move |_, _| reveal_path(&reveal_root)),
        )
        .separator()
        .item(
            MenuItem::new("移除来源…")
                .icon(IconName::Trash2)
                .on_click(move |_, cx| {
                    let _ = remove_this.update(cx, |app, cx| {
                        app.source_remove_confirm = Some(remove_root.clone());
                        cx.notify();
                    });
                }),
        )
}

pub(crate) fn filter_row(
    id: SharedString,
    label: String,
    count: usize,
    active: bool,
    filter: Filter,
    _state: Arc<AppState>,
    this: Entity<KanalibApp>,
    colors: UiColors,
) -> impl IntoElement {
    let mut row = div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .flex()
        .items_center()
        .justify_between()
        .px(px(8.))
        .py(px(5.))
        .rounded(px(5.))
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover));
    if active {
        row = row.bg(colors.selection);
    }
    row.on_click(move |_, _, cx| {
        let _ = this.update(cx, |app, cx| {
            app.set_filter(filter.clone(), cx);
            app.dedup_confirm = None;
        });
    })
    .child(
        div()
            .flex_1()
            .min_w(px(0.))
            .truncate()
            .text_xs()
            .text_color(colors.fg)
            .child(label),
    )
    .child(
        div()
            .flex_none()
            .text_xs()
            .text_color(colors.subtle)
            .child(count.to_string()),
    )
}

pub(crate) fn dedup_row(
    id: SharedString,
    label: &'static str,
    count: String,
    active: bool,
    mode: GroupKind,
    this: Entity<KanalibApp>,
    colors: UiColors,
) -> impl IntoElement {
    let mut row = div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .flex()
        .items_center()
        .justify_between()
        .px(px(8.))
        .py(px(5.))
        .rounded(px(5.))
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover));
    if active {
        row = row.bg(colors.selection);
    }
    row.on_click(move |_, _, cx| {
        let _ = this.update(cx, |app, cx| {
            if app.screen != Screen::Duplicates {
                app.open_dedup_mode(mode, cx);
            }
        });
    })
    .child(div().text_xs().text_color(colors.fg).child(label))
    .child(
        div()
            .text_xs()
            .text_color(if active { colors.accent } else { colors.subtle })
            .child(count),
    )
}

pub(crate) fn tag_manager_row(
    id: SharedString,
    label: &'static str,
    active: bool,
    this: Entity<KanalibApp>,
    colors: UiColors,
) -> impl IntoElement {
    let mut row = div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .flex()
        .items_center()
        .justify_between()
        .px(px(8.))
        .py(px(5.))
        .rounded(px(5.))
        .cursor_pointer()
        .hover(move |style| style.bg(colors.hover));
    if active {
        row = row.bg(colors.selection);
    }
    row.on_click(move |_, _, cx| {
        let _ = this.update(cx, |app, cx| app.open_tag_manager(cx));
    })
    .child(div().text_xs().text_color(colors.fg).child(label))
    .child(
        div()
            .text_xs()
            .text_color(if active { colors.accent } else { colors.subtle })
            .child("›"),
    )
}

impl KanalibApp {
    /// 单来源重扫入口：右键菜单与设置页“重扫”按钮共用。
    pub(crate) fn rescan_source(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        match self.state.rescan_root(root.clone()) {
            Ok(()) => {
                self.settings_notice = Some("已开始重新扫描…".to_string());
            }
            Err(error) => {
                let message = format!("重新扫描失败：{error:#}");
                self.settings_notice = Some(message.clone());
                push_toast(&self.toaster, cx, message, Severity::Danger);
            }
        }
        self.source_menu = None;
        cx.notify();
    }

    pub(crate) fn remove_source(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        match self.state.remove_root(&root) {
            Ok(removed) => {
                self.source_remove_confirm = None;
                self.clear_multi_selection();
                self.auto_select_pending = true;
                push_toast(
                    &self.toaster,
                    cx,
                    format!("已移除来源，磁盘文件未删除（{removed} 张资产移出视图）"),
                    Severity::Success,
                );
            }
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("移除来源失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }

    pub(crate) fn open_source_rename(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        self.source_rename = Some(root);
        self.source_rename_epoch = self.source_rename_epoch.wrapping_add(1);
        cx.notify();
    }

    pub(crate) fn apply_source_rename(&mut self, alias: &str, cx: &mut Context<Self>) {
        let Some(root) = self.source_rename.clone() else {
            return;
        };
        match self.state.set_source_alias(&root, alias) {
            Ok(()) => {
                self.source_rename = None;
                push_toast(
                    &self.toaster,
                    cx,
                    if alias.trim().is_empty() {
                        "已恢复来源默认名称".to_string()
                    } else {
                        format!("已将来源重命名为 {}", alias.trim())
                    },
                    Severity::Success,
                );
            }
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("重命名来源失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }

    pub(crate) fn set_sidebar_width(&mut self, width: f32, cx: &mut Context<Self>) {
        let width = width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
        if (width - self.sidebar_width).abs() < 0.5 {
            return;
        }
        self.sidebar_width = width;
        self.anchor_scroll();
        let weak = cx.entity().downgrade();
        self.sidebar_persist_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let _ = weak.update(cx, |app, _| {
                app.persist_setting("ui.sidebar_width", format!("{:.0}", app.sidebar_width));
            });
        }));
        cx.notify();
    }

    pub(crate) fn sidebar_section_open(&self, key: &str) -> bool {
        !self.sidebar_collapsed.contains(key)
    }

    pub(crate) fn toggle_sidebar_section(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.sidebar_collapsed.remove(key) {
            self.sidebar_collapsed.insert(key.to_string());
        }
        let mut keys = self.sidebar_collapsed.iter().cloned().collect::<Vec<_>>();
        keys.sort();
        self.persist_setting("ui.sidebar_collapsed", keys.join(","));
        cx.notify();
    }

    pub(crate) fn collapse_sidebar_sections(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = SIDEBAR_SECTIONS
            .iter()
            .map(|key| key.to_string())
            .collect();
        let mut keys = self.sidebar_collapsed.iter().cloned().collect::<Vec<_>>();
        keys.sort();
        self.persist_setting("ui.sidebar_collapsed", keys.join(","));
        cx.notify();
    }

    pub(crate) fn set_show_sidebar(&mut self, show: bool, cx: &mut Context<Self>) {
        self.anchor_scroll();
        self.show_sidebar = show;
        self.persist_setting("ui.show_sidebar", show.to_string());
        self.focus_grid_pending = true;
        cx.notify();
    }

    pub(crate) fn sidebar_section_header(
        &self,
        key: &'static str,
        title: &'static str,
        count: Option<usize>,
        this: &Entity<Self>,
        colors: UiColors,
    ) -> AnyElement {
        let open = self.sidebar_section_open(key);
        let this = this.clone();
        div()
            .id(SharedString::from(format!("sidebar-section-{key}")))
            .role(Role::Button)
            .aria_label(title)
            .flex_1()
            .min_w(px(0.))
            .h(px(28.))
            .px(px(6.))
            .flex()
            .items_center()
            .gap(px(4.))
            .rounded(px(4.))
            .cursor_pointer()
            .hover(move |style| style.bg(colors.hover))
            .on_click(move |_, _, cx| {
                let _ = this.update(cx, |app, cx| app.toggle_sidebar_section(key, cx));
            })
            .child(
                Icon::new(if open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(IconSize::Xs)
                .color(colors.subtle),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .truncate()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(colors.subtle)
                    .child(title),
            )
            .children(count.map(|count| {
                div()
                    .flex_none()
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(count.to_string())
            }))
            .into_any_element()
    }

    pub(crate) fn render_sidebar(
        &self,
        this: &Entity<Self>,
        roots: &[PathBuf],
        source_aliases: &HashMap<PathBuf, String>,
        scanning: bool,
        filter: &Filter,
        counts: &Counts,
        screen: Screen,
        dedup_pending: usize,
        saved_searches: &[SavedSearch],
        blocked_tags: &HashSet<String>,
        tag_vocab: &[TagSummary],
        tag_search: &Entity<TextInput>,
        tag_query: &str,
        search: &Entity<TextInput>,
        panel_width: f32,
        colors: UiColors,
    ) -> impl IntoElement + use<> {
        let state = self.state.clone();
        let this_dedup = this.clone();
        let library = screen == Screen::Library;
        let mut body = div()
            .id("sidebar-body")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .px(px(8.))
            .pb(px(12.))
            .flex()
            .flex_col()
            .gap(px(2.));

        body = body.child(self.sidebar_section_header(
            "sources",
            "来源",
            Some(roots.len()),
            this,
            colors,
        ));
        if self.sidebar_section_open("sources") {
            if roots.is_empty() {
                body = body.child(
                    div()
                        .px(px(8.))
                        .pb(px(4.))
                        .text_xs()
                        .text_color(colors.subtle)
                        .child("还没有图库文件夹"),
                );
            } else {
                for root in roots {
                    let count = counts
                        .sources
                        .iter()
                        .find(|(path, _)| path == root)
                        .map(|(_, count)| *count)
                        .unwrap_or(0);
                    let status = self
                        .source_statuses
                        .iter()
                        .find(|status| &status.root == root)
                        .cloned();
                    body = body.child(source_row(
                        SharedString::from(format!("filter-source-{}", root.display())),
                        root.clone(),
                        source_label(root, source_aliases),
                        count,
                        library && filter == &Filter::Source(root.clone()),
                        status,
                        this.clone(),
                        colors,
                    ));
                }
            }
        }

        body = body.child(self.sidebar_section_header(
            "filters",
            "筛选",
            Some(counts.total),
            this,
            colors,
        ));
        if self.sidebar_section_open("filters") {
            body = body.child(filter_row(
                SharedString::from("filter-all"),
                "全部图片".into(),
                counts.total,
                library && *filter == Filter::All,
                Filter::All,
                state.clone(),
                this.clone(),
                colors,
            ));
            body = body.child(filter_row(
                SharedString::from("filter-untagged"),
                "未打标".into(),
                counts.untagged,
                library && *filter == Filter::Untagged,
                Filter::Untagged,
                state.clone(),
                this.clone(),
                colors,
            ));
            body = body.child(filter_row(
                SharedString::from("filter-favorites"),
                "收藏".into(),
                counts.favorites,
                library && *filter == Filter::Favorites,
                Filter::Favorites,
                state.clone(),
                this.clone(),
                colors,
            ));
            if counts.failed > 0 {
                body = body.child(filter_row(
                    SharedString::from("filter-failed"),
                    "失败".into(),
                    counts.failed,
                    library && filter == &Filter::Failed,
                    Filter::Failed,
                    state.clone(),
                    this.clone(),
                    colors,
                ));
            }
            if counts.damaged > 0 {
                body = body.child(filter_row(
                    SharedString::from("filter-damaged"),
                    "损坏".into(),
                    counts.damaged,
                    library && filter == &Filter::Damaged,
                    Filter::Damaged,
                    state.clone(),
                    this.clone(),
                    colors,
                ));
            }
        }

        let this_save_search = this.clone();
        body = body.child(
            div()
                .flex()
                .items_center()
                .child(self.sidebar_section_header(
                    "collections",
                    "智能集合",
                    Some(saved_searches.len()),
                    this,
                    colors,
                ))
                .child(
                    IconButton::new("save-saved-search", IconName::Plus)
                        .size(ControlSize::Sm)
                        .tooltip("保存当前视图为智能集合")
                        .on_click(move |_, _, cx| {
                            let _ = this_save_search.update(cx, |app, cx| {
                                app.save_search_open = true;
                                app.focus_grid_pending = false;
                                cx.notify();
                            });
                        }),
                ),
        );
        if self.sidebar_section_open("collections") {
            if saved_searches.is_empty() {
                body = body.child(
                    div()
                        .px(px(8.))
                        .pb(px(4.))
                        .text_xs()
                        .text_color(colors.subtle)
                        .child("保存筛选、搜索与排序，随时回来"),
                );
            } else {
                for saved in saved_searches {
                    body = body.child(self.saved_search_row(saved, search, this, colors));
                }
            }
        }

        // 手动相册：独立分区 id `albums`，与上面的智能集合（`collections`）是两套概念。
        let albums = self.albums_snapshot();
        let selected_count = self.selected_count();
        let this_new_album = this.clone();
        body = body.child(
            div()
                .flex()
                .items_center()
                .child(self.sidebar_section_header(
                    "albums",
                    "相册",
                    Some(albums.len()),
                    this,
                    colors,
                ))
                .child(
                    IconButton::new("new-album", IconName::FolderPlus)
                        .size(ControlSize::Sm)
                        .tooltip("新建相册")
                        .on_click(move |_, _, cx| {
                            let _ = this_new_album.update(cx, |app, cx| {
                                app.open_org_dialog(OrgDialog::NewAlbum, cx);
                            });
                        }),
                ),
        );
        if self.sidebar_section_open("albums") {
            if albums.is_empty() {
                body = body.child(
                    div()
                        .px(px(8.))
                        .pb(px(4.))
                        .text_xs()
                        .text_color(colors.subtle)
                        .child("新建相册把选中图片归档，不移动磁盘文件"),
                );
            } else {
                for album in &albums {
                    body = body.child(self.album_row(album, selected_count, this, colors));
                }
            }
        }

        body = body.child(self.sidebar_section_header(
            "organize",
            "整理",
            Some(dedup_pending),
            this,
            colors,
        ));
        if self.sidebar_section_open("organize") {
            body = body.child(dedup_row(
                SharedString::from("dedup-entry"),
                "重复项",
                dedup_pending.to_string(),
                screen == Screen::Duplicates,
                GroupKind::Exact,
                this_dedup,
                colors,
            ));
            let this_tags = this.clone();
            body = body.child(tag_manager_row(
                SharedString::from("tag-manager-entry"),
                "标签管理",
                screen == Screen::Tags,
                this_tags,
                colors,
            ));
        }

        if !counts.ratings.is_empty() {
            body = body.child(self.sidebar_section_header(
                "ratings",
                "评级",
                Some(counts.ratings.len()),
                this,
                colors,
            ));
            if self.sidebar_section_open("ratings") {
                for (rating, count) in counts.ratings.iter() {
                    body = body.child(filter_row(
                        SharedString::from(format!("filter-rating-{rating}")),
                        format!("评级 {rating}"),
                        *count,
                        library && filter == &Filter::Rating(rating.clone()),
                        Filter::Rating(rating.clone()),
                        state.clone(),
                        this.clone(),
                        colors,
                    ));
                }
            }
        }

        if !counts.groups.is_empty() {
            body = body.child(self.sidebar_section_header(
                "groups",
                "自动归类",
                Some(counts.groups.len()),
                this,
                colors,
            ));
            if self.sidebar_section_open("groups") {
                for (group, count) in counts.groups.iter() {
                    body = body.child(filter_row(
                        SharedString::from(format!("filter-group-{group}")),
                        group.clone(),
                        *count,
                        library && filter == &Filter::Group(group.clone()),
                        Filter::Group(group.clone()),
                        state.clone(),
                        this.clone(),
                        colors,
                    ));
                }
            }
        }

        let this_tag_sort = this.clone();
        body = body.child(
            div()
                .flex()
                .items_center()
                .child(self.sidebar_section_header(
                    "tags",
                    "标签",
                    Some(tag_vocab.len()),
                    this,
                    colors,
                ))
                .child(
                    IconButton::new("tag-pane-sort", IconName::ArrowDownUp)
                        .size(ControlSize::Sm)
                        .tooltip(if self.tag_pane_sort_name {
                            "按使用次数排序"
                        } else {
                            "按名称排序"
                        })
                        .on_click(move |_, _, cx| {
                            let _ = this_tag_sort.update(cx, |app, cx| {
                                app.tag_pane_sort_name = !app.tag_pane_sort_name;
                                cx.notify();
                            });
                        }),
                ),
        );
        if self.sidebar_section_open("tags") {
        body = body.child(
            div()
                .px(px(8.))
                .pb(px(4.))
                .child(Input::new(tag_search).clearable().least("搜索标签")),
        );
        let mut category_chips = div()
            .flex()
            .flex_wrap()
            .gap(px(3.))
            .px(px(8.))
            .pb(px(4.));
        for (key, label) in [
            ("general", "通用"),
            ("character", "角色"),
            ("copyright", "作品"),
            ("style", "风格"),
            ("meta", "元数据"),
            ("manual", "手工"),
        ] {
            let this = this.clone();
            let active = self.tag_pane_category == key;
            category_chips = category_chips.child(
                div()
                    .id(SharedString::from(format!("tag-category-{key}")))
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .text_xs()
                    .cursor_pointer()
                    .text_color(if active { colors.fg } else { colors.muted })
                    .when(active, |chip| chip.bg(colors.selection))
                    .hover(move |chip| chip.bg(colors.hover))
                    .on_click(move |_, _, cx| {
                        let _ = this.update(cx, |app, cx| {
                            app.tag_pane_category = key.to_string();
                            app.tag_pane_expanded = false;
                            cx.notify();
                        });
                    })
                    .child(label),
            );
        }
        body = body.child(category_chips);

        let needle = tag_query.trim().to_lowercase();
        let mut pane_tags = tag_vocab
            .iter()
            .filter(|tag| {
                tag.count > 0
                    && tag.category == self.tag_pane_category
                    && !blocked_tags.contains(&tag.name)
                    && (needle.is_empty() || tag.name.to_lowercase().contains(&needle))
            })
            .collect::<Vec<_>>();
        if self.tag_pane_sort_name {
            pane_tags.sort_by(|a, b| a.name.cmp(&b.name));
        } else {
            pane_tags.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        }
        let total = pane_tags.len();
        let limit = if self.tag_pane_expanded { 200 } else { 10 };
        pane_tags.truncate(limit);
        if pane_tags.is_empty() {
            body = body.child(
                div()
                    .px(px(8.))
                    .pb(px(4.))
                    .text_xs()
                    .text_color(colors.subtle)
                    .child(if needle.is_empty() {
                        "这个类别还没有标签"
                    } else {
                        "没有匹配的标签"
                    }),
            );
        }
        for tag in pane_tags {
            let name = tag.name.clone();
            let display_name = name.clone();
            let count = tag.count;
            let active = library && filter == &Filter::Tag(name.clone());
            let this_click = this.clone();
            let state_click = state.clone();
            let search_click = search.clone();
            let mut row = div()
                .id(SharedString::from(format!("filter-tag-{name}")))
                .role(Role::Button)
                .aria_label(SharedString::from(format!("#{name}")))
                .flex()
                .items_center()
                .justify_between()
                .px(px(8.))
                .py(px(5.))
                .rounded(px(5.))
                .cursor_pointer()
                .hover(move |row| row.bg(colors.hover));
            if active {
                row = row.bg(colors.selection);
            }
            body = body.child(
                row.on_click(move |event, _, cx| {
                    let modifiers = match event {
                        ClickEvent::Mouse(event) => event.down.modifiers,
                        ClickEvent::Keyboard(_) | ClickEvent::Touch(_) => Modifiers::default(),
                    };
                    if modifiers.secondary() || modifiers.shift {
                        let current = search_click.read(cx).text().to_string();
                        let next = if current.trim().is_empty() {
                            format!("tag:{name}")
                        } else {
                            format!("{current} tag:{name}")
                        };
                        search_click.update(cx, |input, cx| input.set_text(next, cx));
                    } else {
                        state_click.set_filter(Filter::Tag(name.clone()));
                    }
                    let _ = this_click.update(cx, |app, cx| {
                        app.screen = Screen::Library;
                        app.dedup_confirm = None;
                        cx.notify();
                    });
                })
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .truncate()
                        .text_xs()
                        .text_color(colors.fg)
                        .child(format!("#{display_name}")),
                )
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(colors.subtle)
                        .child(count.to_string()),
                ),
            );
        }
        if total > limit {
            let this_more = this.clone();
            body = body.child(
                div().px(px(8.)).pb(px(4.)).child(
                    Button::new(
                        "tag-pane-more",
                        if self.tag_pane_expanded {
                            "收起".to_string()
                        } else {
                            format!("查看更多（{total}）")
                        },
                    )
                    .size(ControlSize::Sm)
                    .on_click(move |_, _, cx| {
                        let _ = this_more.update(cx, |app, cx| {
                            app.tag_pane_expanded = !app.tag_pane_expanded;
                            cx.notify();
                        });
                    }),
                ),
            );
        }

        }

        let screen_title = match screen {
            Screen::Library => "资源管理器",
            Screen::Duplicates => "重复项",
            Screen::Tags => "标签管理",
            Screen::Settings => "设置",
        };

        let this_choose = this.clone();
        let this_rescan = this.clone();
        let this_stop = this.clone();
        let this_retry = this.clone();
        let this_collapse = this.clone();
        let mut more_menu = Menu::new()
            .item(
                MenuItem::new("添加文件夹")
                    .icon(IconName::FolderOpen)
                    .on_click(move |_, cx| {
                        let _ = this_choose.update(cx, |app, cx| app.choose_folder(cx));
                    }),
            )
            .item(
                MenuItem::new("重新扫描图库")
                    .icon(IconName::RefreshCw)
                    .on_click(move |_, cx| {
                        let _ = this_rescan.update(cx, |app, cx| app.rescan(cx));
                    }),
            );
        if scanning {
            more_menu = more_menu.item(
                MenuItem::new("停止扫描")
                    .icon(IconName::Square)
                    .on_click(move |_, cx| {
                        let _ = this_stop.update(cx, |app, cx| {
                            app.state.stop();
                            cx.notify();
                        });
                    }),
            );
        }
        if counts.failed > 0 {
            more_menu = more_menu.item(
                MenuItem::new("重试失败")
                    .icon(IconName::RotateCcw)
                    .on_click(move |_, cx| {
                        let _ = this_retry.update(cx, |app, cx| app.retry_failed(cx));
                    }),
            );
        }
        more_menu = more_menu.separator().item(
            MenuItem::new("全部折叠")
                .icon(IconName::ChevronsUpDown)
                .on_click(move |_, cx| {
                    let _ = this_collapse.update(cx, |app, cx| app.collapse_sidebar_sections(cx));
                }),
        );

        let panel = div()
            .relative()
            .w(px(panel_width))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(colors.surface)
            .border_r_1()
            .border_color(colors.border)
            .child(
                div()
                    .h(px(36.))
                    .px(px(10.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(
                        div()
                            .truncate()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(screen_title),
                    )
                    .child(OverflowMenu::new("sidebar-more", more_menu).tooltip("侧栏操作")),
            )
            .child(body)
            .child(
                div()
                    .id("sidebar-resize")
                    .aria_label("调整侧栏宽度")
                    .absolute()
                    .right(px(-3.))
                    .top_0()
                    .bottom_0()
                    .w(px(6.))
                    .cursor_ew_resize()
                    .hover(move |style| style.bg(colors.accent)),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    {
                        let this = this.clone();
                        move |_, _, window, _| {
                            let pressed = this.clone();
                            let panel_right = ACTIVITY_BAR_WIDTH + panel_width;
                            window.on_mouse_event(
                                move |event: &MouseDownEvent, phase, _, cx| {
                                    if phase != DispatchPhase::Capture
                                        || event.button != MouseButton::Left
                                        || event.position.y.as_f32() < TITLEBAR_HEIGHT
                                        || (event.position.x.as_f32() - panel_right).abs() > 4.
                                    {
                                        return;
                                    }
                                    cx.stop_propagation();
                                    let _ = pressed.update(cx, |app, cx| {
                                        app.sidebar_resizing = true;
                                        cx.notify();
                                    });
                                },
                            );
                            let moved = this.clone();
                            window.on_mouse_event(
                                move |event: &MouseMoveEvent, phase, _, cx| {
                                    if phase != DispatchPhase::Bubble {
                                        return;
                                    }
                                    if !moved.read(cx).sidebar_resizing {
                                        return;
                                    }
                                    let width =
                                        event.position.x.as_f32() - ACTIVITY_BAR_WIDTH;
                                    let _ = moved.update(cx, |app, cx| {
                                        app.set_sidebar_width(width, cx)
                                    });
                                },
                            );
                            let lifted = this.clone();
                            window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                                if phase != DispatchPhase::Bubble {
                                    return;
                                }
                                let _ = lifted.update(cx, |app, cx| {
                                    if app.sidebar_resizing {
                                        app.sidebar_resizing = false;
                                        app.persist_setting(
                                            "ui.sidebar_width",
                                            format!("{:.0}", app.sidebar_width),
                                        );
                                        cx.notify();
                                    }
                                });
                            });
                        }
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .w(px(1.))
                .h(px(1.)),
            );

        let this_library = this.clone();
        let this_search = this.clone();
        let this_duplicates = this.clone();
        let this_tags = this.clone();
        let this_settings = this.clone();
        let this_theme = this.clone();
        let library_badge = counts.failed;
        let activity = div()
            .id("activity-bar")
            .role(Role::Toolbar)
            .aria_label("活动栏")
            .w(px(ACTIVITY_BAR_WIDTH))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_between()
            .py(px(6.))
            .bg(colors.sunken)
            .border_r_1()
            .border_color(colors.border)
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(2.))
                    .child(activity_item(
                        ActivityTarget::Library,
                        IconName::Library,
                        "图库",
                        screen == Screen::Library,
                        library_badge,
                        this_library,
                        colors,
                    ))
                    .child(activity_item(
                        ActivityTarget::Search,
                        IconName::Search,
                        "搜索",
                        false,
                        0,
                        this_search,
                        colors,
                    ))
                    .child(activity_item(
                        ActivityTarget::Duplicates,
                        IconName::Copy,
                        "重复项",
                        screen == Screen::Duplicates,
                        dedup_pending,
                        this_duplicates,
                        colors,
                    ))
                    .child(activity_item(
                        ActivityTarget::Tags,
                        IconName::Tag,
                        "标签",
                        screen == Screen::Tags,
                        0,
                        this_tags,
                        colors,
                    ))
                    .child(activity_item(
                        ActivityTarget::Settings,
                        IconName::Settings,
                        "设置",
                        screen == Screen::Settings,
                        0,
                        this_settings,
                        colors,
                    )),
            )
            .child(activity_item(
                ActivityTarget::Theme,
                if self.theme_dark {
                    IconName::Sun
                } else {
                    IconName::Moon
                },
                "主题",
                false,
                0,
                this_theme,
                colors,
            ));

        div()
            .flex_none()
            .h_full()
            .w(px(ACTIVITY_BAR_WIDTH + panel_width))
            .flex()
            .child(activity)
            .child(panel)
    }

    /// 当前选中数量；智能集合/相册的批量菜单用来显示将要作用的张数。
    pub(crate) fn selected_count(&self) -> usize {
        if !self.selection.is_empty() {
            return self.selection.len();
        }
        usize::from(self.state.shared.lock().unwrap().selected.is_some())
    }

    /// 相册列表快照：只在 `shared.revision` 变化时重查，避免每帧打开数据库。
    pub(crate) fn albums_snapshot(&self) -> Vec<Collection> {
        let revision = self.state.shared.lock().unwrap().revision;
        let mut cache = self.album_cache.borrow_mut();
        if cache.revision != revision {
            cache.revision = revision;
            match self.state.collections() {
                Ok(albums) => cache.albums = albums,
                Err(error) => log::warn!("load albums: {error:#}"),
            }
        }
        cache.albums.clone()
    }

    /// 某资产所属的相册（含计数）；同样按 revision 失效。
    pub(crate) fn asset_album_names(&self, path: &Path) -> Vec<Collection> {
        let revision = self.state.shared.lock().unwrap().revision;
        let mut cache = self.album_cache.borrow_mut();
        if cache.membership_revision != revision {
            cache.membership_revision = revision;
            cache.membership.clear();
        }
        if let Some(found) = cache.membership.get(path) {
            return found.clone();
        }
        let found = match self.state.asset_collections(path) {
            Ok(found) => found,
            Err(error) => {
                log::warn!("load asset albums: {error:#}");
                Vec::new()
            }
        };
        cache.membership.insert(path.to_path_buf(), found.clone());
        found
    }

    pub(crate) fn open_org_dialog(&mut self, dialog: OrgDialog, cx: &mut Context<Self>) {
        self.org_dialog = Some(dialog);
        self.org_dialog_epoch = self.org_dialog_epoch.wrapping_add(1);
        self.focus_grid_pending = false;
        cx.notify();
    }

    /// 智能集合行：点击名称应用集合（恢复筛选/搜索/排序/视图/窗格）；菜单提供更新、
    /// 重命名、置顶、上下移动与删除。
    fn saved_search_row(
        &self,
        saved: &SavedSearch,
        search: &Entity<TextInput>,
        this: &Entity<Self>,
        colors: UiColors,
    ) -> AnyElement {
        let id = saved.id;
        let name = saved.name.clone();
        let pinned = saved.pinned;
        let detail = if saved.query.is_empty() {
            saved.filter.label()
        } else {
            format!("{} · {}", saved.filter.label(), saved.query)
        };
        let saved_for_apply = saved.clone();
        let search_for_apply = search.clone();
        let this_apply = this.clone();

        let menu_update = this.clone();
        let menu_rename = this.clone();
        let menu_pin = this.clone();
        let menu_up = this.clone();
        let menu_down = this.clone();
        let menu_delete = this.clone();
        let rename_name = name.clone();
        let menu = Menu::new()
            .item(
                MenuItem::new("用当前视图更新")
                    .icon(IconName::RefreshCw)
                    .on_click(move |_, cx| {
                        let _ = menu_update.update(cx, |app, cx| {
                            app.update_saved_search_from_current(id, cx);
                        });
                    }),
            )
            .item(
                MenuItem::new("重命名…")
                    .icon(IconName::Pencil)
                    .on_click(move |_, cx| {
                        let dialog = OrgDialog::RenameSavedSearch {
                            id,
                            name: rename_name.clone(),
                        };
                        let _ = menu_rename
                            .update(cx, |app, cx| app.open_org_dialog(dialog, cx));
                    }),
            )
            .item(
                MenuItem::new(if pinned { "取消置顶" } else { "置顶" })
                    .icon(IconName::Pin)
                    .on_click(move |_, cx| {
                        let _ = menu_pin.update(cx, |app, cx| {
                            app.toggle_saved_search_pin(id, cx);
                        });
                    }),
            )
            .separator()
            .item(
                MenuItem::new("上移")
                    .icon(IconName::ArrowUp)
                    .on_click(move |_, cx| {
                        let _ = menu_up.update(cx, |app, cx| {
                            app.move_saved_search(id, -1, cx);
                        });
                    }),
            )
            .item(
                MenuItem::new("下移")
                    .icon(IconName::ArrowDown)
                    .on_click(move |_, cx| {
                        let _ = menu_down.update(cx, |app, cx| {
                            app.move_saved_search(id, 1, cx);
                        });
                    }),
            )
            .separator()
            .item(
                MenuItem::new("删除")
                    .icon(IconName::Trash2)
                    .on_click(move |_, cx| {
                        let _ = menu_delete.update(cx, |app, cx| {
                            app.delete_saved_search(id, cx);
                        });
                    }),
            );

        div()
            .id(SharedString::from(format!("saved-search-row-{id}")))
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(4.))
            .py(px(2.))
            .rounded(px(6.))
            .hover(move |style| style.bg(colors.hover))
            .child(
                div()
                    .id(SharedString::from(format!("saved-search-{id}")))
                    .role(Role::Button)
                    .aria_label(name.clone())
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(4.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        let _ = this_apply.update(cx, |app, cx| {
                            app.apply_saved_search(&saved_for_apply, &search_for_apply, cx)
                        });
                    })
                    .children(pinned.then(|| {
                        Icon::new(IconName::Pin)
                            .size(IconSize::Xs)
                            .color(colors.accent)
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(div().truncate().text_xs().text_color(colors.fg).child(name))
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(colors.subtle)
                                    .child(detail),
                            ),
                    ),
            )
            .child(
                OverflowMenu::new(
                    SharedString::from(format!("saved-search-menu-{id}")),
                    menu,
                )
                .tooltip("管理智能集合"),
            )
            .into_any_element()
    }

    /// 相册行：显示名称与计数；菜单提供加入/移出选中、重命名与删除。
    fn album_row(
        &self,
        album: &Collection,
        selected_count: usize,
        this: &Entity<Self>,
        colors: UiColors,
    ) -> AnyElement {
        let id = album.id;
        let name = album.name.clone();
        let count = album.asset_count;
        let add = this.clone();
        let remove = this.clone();
        let rename = this.clone();
        let delete = this.clone();
        let rename_name = name.clone();
        let delete_name = name.clone();
        let menu = Menu::new()
            .item(
                MenuItem::new(format!("把选中加入（{selected_count} 张）"))
                    .icon(IconName::FolderPlus)
                    .disabled(selected_count == 0)
                    .on_click(move |_, cx| {
                        let _ = add.update(cx, |app, cx| {
                            app.add_selection_to_album(id, cx);
                        });
                    }),
            )
            .item(
                MenuItem::new(format!("从相册移出选中（{selected_count} 张）"))
                    .icon(IconName::FolderOpen)
                    .disabled(selected_count == 0)
                    .on_click(move |_, cx| {
                        let _ = remove.update(cx, |app, cx| {
                            app.remove_selection_from_album(id, cx);
                        });
                    }),
            )
            .separator()
            .item(
                MenuItem::new("重命名…")
                    .icon(IconName::Pencil)
                    .on_click(move |_, cx| {
                        let dialog = OrgDialog::RenameAlbum {
                            id,
                            name: rename_name.clone(),
                        };
                        let _ = rename.update(cx, |app, cx| app.open_org_dialog(dialog, cx));
                    }),
            )
            .item(
                MenuItem::new("删除相册…")
                    .icon(IconName::Trash2)
                    .on_click(move |_, cx| {
                        let dialog = OrgDialog::DeleteAlbum {
                            id,
                            name: delete_name.clone(),
                            count,
                        };
                        let _ = delete.update(cx, |app, cx| app.open_org_dialog(dialog, cx));
                    }),
            );

        div()
            .id(SharedString::from(format!("album-row-{id}")))
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(4.))
            .py(px(2.))
            .rounded(px(6.))
            .hover(move |style| style.bg(colors.hover))
            .child(
                div()
                    .id(SharedString::from(format!("album-{id}")))
                    .role(Role::Button)
                    .aria_label(format!("相册 {name}"))
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(4.))
                    .py(px(2.))
                    .child(Icon::new(IconName::Folder).size(IconSize::Xs).color(colors.subtle))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_xs()
                            .text_color(colors.fg)
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(colors.subtle)
                            .child(count.to_string()),
                    ),
            )
            .child(
                OverflowMenu::new(
                    SharedString::from(format!("album-menu-{id}")),
                    menu,
                )
                .tooltip("管理相册"),
            )
            .into_any_element()
    }

    /// 组织弹窗统一在 `app.rs` 的根节点挂载，避免被侧栏/检查器的滚动容器裁切。
    pub(crate) fn render_org_dialog(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
        this: &Entity<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.org_dialog.clone()?;
        let epoch = self.org_dialog_epoch;
        let mut prompt = |title: &'static str,
                      placeholder: &'static str,
                      initial: String,
                      label: String,
                      submit_label: &'static str,
                      target: OrgDialog,
                      close: Entity<Self>,
                      submit: Entity<Self>| {
            let holder = window.use_keyed_state(
                SharedString::from(format!("kanalib-org-field-{epoch}")),
                cx,
                move |window, cx| {
                    let input = cx.new(|cx| TextInput::new(window, cx).placeholder(placeholder));
                    if !initial.is_empty() {
                        input.update(cx, |input, cx| input.set_text(initial, cx));
                    }
                    input
                },
            );
            let field = holder.read(cx).clone();
            PromptDialog::new(
                SharedString::from(format!("kanalib-org-dialog-{epoch}")),
                title,
                &field,
                move |_, cx| {
                    let _ = close.update(cx, |app, cx| {
                        app.org_dialog = None;
                        cx.notify();
                    });
                },
            )
            .label(label)
            .submit(submit_label)
            .check(|text| {
                if canonicalize_org_name(text).is_empty() {
                    Err("名称不能为空".into())
                } else {
                    Ok(())
                }
            })
            .on_submit(move |text, _, cx| {
                // PromptDialog 会先执行关闭回调，不能依赖 `self.org_dialog`，目标随闭包传入。
                let target = target.clone();
                let _ = submit.update(cx, |app, cx| app.submit_org_dialog(target, text, cx));
            })
            .into_any_element()
        };
        Some(match dialog.clone() {
            OrgDialog::RenameSavedSearch { name, .. } => prompt(
                "重命名智能集合",
                "智能集合名称",
                name,
                "重命名不改集合 ID，图标与排序保持不变".into(),
                "保存",
                dialog,
                this.clone(),
                this.clone(),
            ),
            OrgDialog::NewAlbum => prompt(
                "新建相册",
                "相册名称",
                String::new(),
                "相册只记录图片归属，不移动、不复制磁盘文件".into(),
                "新建",
                dialog,
                this.clone(),
                this.clone(),
            ),
            OrgDialog::RenameAlbum { name, .. } => prompt(
                "重命名相册",
                "相册名称",
                name,
                "重命名不改相册 ID，已归入的图片保持不变".into(),
                "保存",
                dialog,
                this.clone(),
                this.clone(),
            ),
            OrgDialog::DeleteAlbum { id, name, count } => {
                let close = this.clone();
                let confirm = this.clone();
                ConfirmDialog::new(
                    SharedString::from(format!("kanalib-delete-album-{epoch}")),
                    "删除相册？",
                    format!(
                        "将删除相册「{name}」与它的归属关系（{count} 张）。\n图片仍保留在图库和磁盘上，不会被删除。"
                    ),
                    move |_, cx| {
                        let _ = close.update(cx, |app, cx| {
                            app.org_dialog = None;
                            cx.notify();
                        });
                    },
                )
                .confirm("删除相册")
                .destructive()
                .on_confirm(move |_, cx| {
                    let _ = confirm.update(cx, |app, cx| app.delete_album(id, &name, cx));
                })
                .into_any_element()
            }
        })
    }

    pub(crate) fn submit_org_dialog(
        &mut self,
        dialog: OrgDialog,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let name = canonicalize_org_name(text);
        if name.is_empty() {
            push_toast(&self.toaster, cx, "名称不能为空", Severity::Warning);
            return;
        }
        match dialog {
            OrgDialog::RenameSavedSearch { id, .. } => self.rename_saved_search_checked(id, &name, cx),
            OrgDialog::NewAlbum => self.create_album(&name, cx),
            OrgDialog::RenameAlbum { id, .. } => self.rename_album(id, &name, cx),
            OrgDialog::DeleteAlbum { .. } => {}
        }
    }

    pub(crate) fn rename_saved_search_checked(
        &mut self,
        id: i64,
        name: &str,
        cx: &mut Context<Self>,
    ) {
        match self.state.rename_saved_search(id, name) {
            Ok(saved) => {
                self.org_dialog = None;
                push_toast(
                    &self.toaster,
                    cx,
                    format!("已将智能集合重命名为「{}」（ID 不变）", saved.name),
                    Severity::Success,
                );
            }
            Err(error) => {
                if error.downcast_ref::<DuplicateNameError>().is_some() {
                    push_toast(
                        &self.toaster,
                        cx,
                        duplicate_name_message(name),
                        Severity::Warning,
                    );
                } else {
                    push_toast(
                        &self.toaster,
                        cx,
                        format!("重命名智能集合失败：{error:#}"),
                        Severity::Danger,
                    );
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn create_album(&mut self, name: &str, cx: &mut Context<Self>) {
        match self.state.create_collection(name) {
            Ok(album) => {
                self.org_dialog = None;
                push_toast(
                    &self.toaster,
                    cx,
                    format!("已新建相册「{}」", album.name),
                    Severity::Success,
                );
            }
            Err(error) => {
                if error.downcast_ref::<DuplicateNameError>().is_some() {
                    push_toast(
                        &self.toaster,
                        cx,
                        duplicate_name_message(name),
                        Severity::Warning,
                    );
                } else {
                    push_toast(
                        &self.toaster,
                        cx,
                        format!("新建相册失败：{error:#}"),
                        Severity::Danger,
                    );
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn rename_album(&mut self, id: i64, name: &str, cx: &mut Context<Self>) {
        match self.state.rename_collection(id, name) {
            Ok(album) => {
                self.org_dialog = None;
                push_toast(
                    &self.toaster,
                    cx,
                    format!("已将相册重命名为「{}」（ID 不变）", album.name),
                    Severity::Success,
                );
            }
            Err(error) => {
                if error.downcast_ref::<DuplicateNameError>().is_some() {
                    push_toast(
                        &self.toaster,
                        cx,
                        duplicate_name_message(name),
                        Severity::Warning,
                    );
                } else {
                    push_toast(
                        &self.toaster,
                        cx,
                        format!("重命名相册失败：{error:#}"),
                        Severity::Danger,
                    );
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn delete_album(&mut self, id: i64, name: &str, cx: &mut Context<Self>) {
        match self.state.delete_collection(id) {
            Ok(()) => {
                self.org_dialog = None;
                push_toast(
                    &self.toaster,
                    cx,
                    format!("已删除相册「{name}」；图片仍在图库，仅移除了归属关系"),
                    Severity::Success,
                );
            }
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("删除相册失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }

    pub(crate) fn add_selection_to_album(&mut self, id: i64, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            push_toast(&self.toaster, cx, "先在网格里选择图片", Severity::Warning);
            return;
        }
        self.add_paths_to_album(id, paths, cx);
    }

    pub(crate) fn add_paths_to_album(
        &mut self,
        id: i64,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let wanted = paths.len();
        match self.state.add_assets_to_collection(id, &paths) {
            Ok(added) => push_toast(
                &self.toaster,
                cx,
                if added == 0 {
                    format!("没有新增：{wanted} 张已在相册中或不在图库")
                } else {
                    format!("已加入相册 {added} 张（选中 {wanted} 张）")
                },
                if added == 0 {
                    Severity::Warning
                } else {
                    Severity::Success
                },
            ),
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("加入相册失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }

    pub(crate) fn remove_selection_from_album(&mut self, id: i64, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            push_toast(&self.toaster, cx, "先在网格里选择图片", Severity::Warning);
            return;
        }
        self.remove_paths_from_album(id, paths, cx);
    }

    pub(crate) fn remove_paths_from_album(
        &mut self,
        id: i64,
        paths: Vec<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let wanted = paths.len();
        match self.state.remove_assets_from_collection(id, &paths) {
            Ok(removed) => push_toast(
                &self.toaster,
                cx,
                if removed == 0 {
                    format!("没有移出：{wanted} 张不在该相册中")
                } else {
                    format!("已从相册移出 {removed} 张（选中 {wanted} 张）")
                },
                if removed == 0 {
                    Severity::Warning
                } else {
                    Severity::Success
                },
            ),
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("移出相册失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }

    pub(crate) fn remove_path_from_album(
        &mut self,
        id: i64,
        path: PathBuf,
        cx: &mut Context<Self>,
    ) {
        match self.state.remove_assets_from_collection(id, &[path]) {
            Ok(removed) => push_toast(
                &self.toaster,
                cx,
                if removed == 0 {
                    "这张图片不在该相册中".to_string()
                } else {
                    "已从相册移出 1 张".to_string()
                },
                if removed == 0 {
                    Severity::Warning
                } else {
                    Severity::Success
                },
            ),
            Err(error) => push_toast(
                &self.toaster,
                cx,
                format!("移出相册失败：{error:#}"),
                Severity::Danger,
            ),
        }
        cx.notify();
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn organization_sections_use_distinct_ids() {
        assert!(SIDEBAR_SECTIONS.contains(&"collections"));
        assert!(SIDEBAR_SECTIONS.contains(&"albums"));
        let unique = SIDEBAR_SECTIONS.iter().collect::<HashSet<_>>();
        assert_eq!(unique.len(), SIDEBAR_SECTIONS.len());
    }

    #[test]
    fn org_names_are_trimmed_for_conflict_checks() {
        assert_eq!(canonicalize_org_name("  旅行  "), "旅行");
        assert!(canonicalize_org_name(" \t ").is_empty());
        assert!(duplicate_name_message("旅行").contains("旅行"));
    }
}
