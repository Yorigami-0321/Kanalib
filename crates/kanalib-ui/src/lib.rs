use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ely_gpui_component::{
    buttons::{Button, ButtonVariant, IconButton},
    feedback::{Toast, ToastViewport, Toaster},
    forms::{Choice, Input, Rating, Slider, TagInput, TextInput},
    media::ImageViewer,
    menus::{ContextMenu, DropdownMenu, Menu, MenuItem, OverflowMenu},
    navigation::{Command, CommandPalette},
    overlays::{ConfirmDialog, PromptDialog},
    primitives::{Icon, IconName, Severity},
    theme::{ActiveTheme, ControlSize, IconSize, Mode, Theme},
};
use gpui::prelude::*;
use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, DispatchPhase, Entity, FocusHandle,
    Focusable, FontWeight, Hsla, ImageCache, ImageCacheError, ImageCacheItem, IntoElement,
    KeyBinding, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit,
    ParentElement,
    Pixels, Render, RenderImage, Resource, Role, ScrollStrategy, SharedString, Styled, Task,
    UniformListScrollHandle, WeakEntity, Window, canvas, div, image_cache, img, px, uniform_list,
    white,
};
#[cfg(feature = "frame-profiler")]
use hdrhistogram::Histogram;

use kanalib_core::dedup::{
    DedupSummary, DuplicateGroup, GroupKind, ReviewState, is_protected,
    similarity_pct_from_distance,
};
use kanalib_core::model::{
    Asset, Counts, DiagnosticSnapshot, Filter, RetagSummary, SavedSearch, ScanFilterConfig,
    SearchQuery, SourceScanStatus, TagCandidateReview, TagHit, TagJobError, TagQueueStatus,
    TagRecomputeSummary, TagReviewState, TagSummary, TagThresholds, TAG_CANDIDATE_MIN_SCORE,
    default_category_thresholds, is_ffmpeg_only, normalize_tag_name,
};
use kanalib_core::backup;
use kanalib_core::db::{MAX_SEARCH_PAGE, SearchPageRequest, SearchSort};
use kanalib_core::diagnostics::{self, DiagnosticsContext};
use kanalib_core::export;
use kanalib_core::state::{AppState, CacheStats};

pub use app::KanalibApp;
pub use thumb_cache::{CacheCleanup, ThumbCache};

mod app;
mod compare;
mod dedup;
mod format;
mod inspector;
mod platform;
mod search;
mod settings;
mod sidebar;
mod status_bar;
mod tags;
mod theme;
mod thumb_cache;
mod viewer;
mod views;

gpui::actions!(
    kanalib,
    [
        OpenPalette,
        FocusSearch,
        ClearSelection,
        SelectAllVisible,
        SelectPrev,
        SelectNext,
        SelectPrevRow,
        SelectNextRow,
        SelectFirst,
        SelectLast,
        CloseSimilar,
        OpenQuickView,
        ToggleFavorite,
        Rate0,
        Rate1,
        Rate2,
        Rate3,
        Rate4,
        Rate5,
        ToggleSidebar,
        ViewIconsLarge,
        ViewIconsMedium,
        ViewIconsSmall,
        ViewList,
        ViewDetails,
        ViewContent,
        ViewerClose,
        ViewerPrev,
        ViewerNext,
        ViewerFullscreen,
        CompareClose,
        CompareZoomIn,
        CompareZoomOut,
        CompareFit,
        ComparePanLeft,
        ComparePanRight,
        ComparePanUp,
        ComparePanDown,
    ]
);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum SortMode {
    #[default]
    Name,
    Modified,
    Captured,
    Size,
    Resolution,
    Star,
    Favorite,
    Rating,
}

impl SortMode {
    fn label(self) -> &'static str {
        match self {
            Self::Name => "文件名",
            Self::Modified => "修改时间",
            Self::Captured => "拍摄时间",
            Self::Size => "文件大小",
            Self::Resolution => "分辨率",
            Self::Star => "星级",
            Self::Favorite => "收藏优先",
            Self::Rating => "模型评级",
        }
    }

    fn default_ascending(self) -> bool {
        matches!(self, Self::Name)
    }

    fn key(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Modified => "modified",
            Self::Captured => "captured",
            Self::Size => "size",
            Self::Resolution => "resolution",
            Self::Star => "star",
            Self::Favorite => "favorite",
            Self::Rating => "rating",
        }
    }

    fn from_key(value: &str) -> Option<Self> {
        Some(match value {
            "name" => Self::Name,
            "modified" => Self::Modified,
            "captured" => Self::Captured,
            "size" => Self::Size,
            "resolution" => Self::Resolution,
            "star" => Self::Star,
            "favorite" => Self::Favorite,
            "rating" => Self::Rating,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum ViewMode {
    IconsLarge,
    #[default]
    IconsMedium,
    IconsSmall,
    List,
    Details,
    Content,
}

impl ViewMode {
    const ALL: [ViewMode; 6] = [
        Self::IconsLarge,
        Self::IconsMedium,
        Self::IconsSmall,
        Self::List,
        Self::Details,
        Self::Content,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::IconsLarge => "icons-large",
            Self::IconsMedium => "icons-medium",
            Self::IconsSmall => "icons-small",
            Self::List => "list",
            Self::Details => "details",
            Self::Content => "content",
        }
    }

    fn from_key(value: &str) -> Option<Self> {
        Some(match value {
            "icons-large" | "icons-xl" | "icons-lg" => Self::IconsLarge,
            "icons-medium" | "icons-md" | "tiles" => Self::IconsMedium,
            "icons-small" | "icons-sm" => Self::IconsSmall,
            "list" => Self::List,
            "details" => Self::Details,
            "content" => Self::Content,
            _ => return None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            Self::IconsLarge => "大图标",
            Self::IconsMedium => "中图标",
            Self::IconsSmall => "小图标",
            Self::List => "列表",
            Self::Details => "详细信息",
            Self::Content => "内容",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::IconsLarge => IconName::Image,
            Self::IconsMedium => IconName::LayoutGrid,
            Self::IconsSmall => IconName::Grid3x3,
            Self::List => IconName::List,
            Self::Details => IconName::Table,
            Self::Content => IconName::AlignJustify,
        }
    }

    fn shortcut(self) -> &'static str {
        match self {
            Self::IconsLarge => "ctrl-shift-1",
            Self::IconsMedium => "ctrl-shift-3",
            Self::IconsSmall => "ctrl-shift-4",
            Self::List => "ctrl-shift-5",
            Self::Details => "ctrl-shift-6",
            Self::Content => "ctrl-shift-8",
        }
    }

    fn is_grid(self) -> bool {
        matches!(
            self,
            Self::IconsLarge | Self::IconsMedium | Self::IconsSmall
        )
    }

    fn min_width(self) -> f32 {
        match self {
            Self::IconsLarge => 288.,
            Self::IconsMedium => 160.,
            Self::IconsSmall => 128.,
            Self::List | Self::Details => 1.,
            Self::Content => 1.,
        }
    }

    fn image_height(self) -> f32 {
        match self {
            Self::IconsLarge => 268.,
            Self::IconsMedium => 150.,
            Self::IconsSmall => 112.,
            Self::List | Self::Details | Self::Content => 0.,
        }
    }

    fn row_height(self) -> f32 {
        match self {
            Self::IconsLarge | Self::IconsMedium | Self::IconsSmall => self.image_height() + 68.,
            Self::List | Self::Details => 32.,
            Self::Content => 84.,
        }
    }

    fn prefers_large_thumb(self) -> bool {
        matches!(self, Self::IconsLarge)
    }
}

/// F-005 分组浏览的维度；`None` 表示平铺（默认，旧行为）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum GroupMode {
    #[default]
    None,
    Modified,
    Captured,
    Source,
    Rating,
}

impl GroupMode {
    const ALL: [GroupMode; 5] = [
        Self::None,
        Self::Modified,
        Self::Captured,
        Self::Source,
        Self::Rating,
    ];

    fn key(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Modified => "modified",
            Self::Captured => "captured",
            Self::Source => "source",
            Self::Rating => "rating",
        }
    }

    fn from_key(value: &str) -> Option<Self> {
        Some(match value {
            "none" => Self::None,
            "modified" => Self::Modified,
            "captured" => Self::Captured,
            "source" => Self::Source,
            "rating" => Self::Rating,
            _ => return None,
        })
    }

    fn label(self) -> &'static str {
        match self {
            Self::None => "不分组",
            Self::Modified => "修改时间",
            Self::Captured => "拍摄时间",
            Self::Source => "来源",
            Self::Rating => "评级",
        }
    }
}

fn resolve_view_mode(saved: Option<&str>, legacy_density: Option<&str>) -> ViewMode {
    saved
        .and_then(ViewMode::from_key)
        .unwrap_or_else(|| match legacy_density {
            Some("small") => ViewMode::IconsSmall,
            Some("large") => ViewMode::IconsLarge,
            _ => ViewMode::IconsMedium,
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum PaneMode {
    #[default]
    Details,
    Preview,
    Hidden,
}

impl PaneMode {
    fn key(self) -> &'static str {
        match self {
            Self::Details => "details",
            Self::Preview => "preview",
            Self::Hidden => "hidden",
        }
    }

    fn from_key(value: &str) -> Self {
        match value {
            "preview" => Self::Preview,
            "hidden" => Self::Hidden,
            _ => Self::Details,
        }
    }

    fn width(self) -> f32 {
        match self {
            Self::Hidden => 0.,
            Self::Details | Self::Preview => 310.,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Screen {
    Library,
    /// F-004：单锚点相似查询的临时结果视图；只保留当前一次查询，不写库。
    Similar,
    /// F-003：图片对比；只加载当前 2–4 张，保留标记仅存内存。
    Compare,
    Duplicates,
    Tags,
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActivityTarget {
    Library,
    Search,
    Duplicates,
    Tags,
    Settings,
    Theme,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DedupFilter {
    Pending,
    Ignored,
    Related,
    Done,
    All,
}

impl DedupFilter {
    fn label(self) -> &'static str {
        match self {
            Self::Pending => "未审",
            Self::Ignored => "已忽略",
            Self::Related => "相关变体",
            Self::Done => "已保留",
            Self::All => "全部",
        }
    }

    fn matches(self, state: ReviewState) -> bool {
        match self {
            Self::Pending => state == ReviewState::Pending,
            Self::Ignored => state == ReviewState::Ignored,
            Self::Related => state == ReviewState::Related,
            Self::Done => state == ReviewState::Done,
            Self::All => true,
        }
    }
}

#[derive(Clone, Debug)]
enum DedupConfirm {
    Group(String),
    Batch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RescanOrigin {
    Library,
    Dedup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TagDialogMode {
    Add,
    Remove,
}

/// 大范围重打标的确认信息；`scope` 只用于展示来源（选中/当前视图）。
#[derive(Clone)]
struct TagRetagConfirm {
    paths: Vec<PathBuf>,
    scope: &'static str,
}

#[derive(Clone)]
struct SourceMenuState {
    request: u64,
    root: PathBuf,
    position: gpui::Point<Pixels>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportFormat {
    Csv,
    Json,
}

impl ExportFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Json => "json",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExportTarget {
    Selected,
    View,
}

impl ExportTarget {
    fn slug(self) -> &'static str {
        match self {
            Self::Selected => "selected",
            Self::View => "view",
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn view_mode_keys_round_trip() {
        for mode in super::ViewMode::ALL {
            assert_eq!(super::ViewMode::from_key(mode.key()), Some(mode));
        }
        assert_eq!(super::ViewMode::from_key("unknown"), None);
    }

    #[test]
    fn view_mode_migrates_legacy_density() {
        use super::{ViewMode, resolve_view_mode};
        assert_eq!(resolve_view_mode(None, Some("small")), ViewMode::IconsSmall);
        assert_eq!(resolve_view_mode(None, Some("medium")), ViewMode::IconsMedium);
        assert_eq!(resolve_view_mode(None, Some("large")), ViewMode::IconsLarge);
        assert_eq!(resolve_view_mode(None, None), ViewMode::IconsMedium);
        assert_eq!(
            resolve_view_mode(Some("details"), Some("large")),
            ViewMode::Details
        );
    }

    #[test]
    fn view_modes_are_converged_to_six() {
        use super::ViewMode;
        assert_eq!(ViewMode::ALL.len(), 6);
        assert_eq!(ViewMode::from_key("tiles"), Some(ViewMode::IconsMedium));
        assert_eq!(ViewMode::from_key("icons-lg"), Some(ViewMode::IconsLarge));
        assert_eq!(ViewMode::from_key("icons-xl"), Some(ViewMode::IconsLarge));
        assert_eq!(ViewMode::IconsSmall.icon().name(), "grid-3x3");
        assert_eq!(ViewMode::IconsMedium.icon().name(), "layout-grid");
    }

    #[test]
    fn group_mode_keys_round_trip_and_default_off() {
        use super::GroupMode;
        for mode in GroupMode::ALL {
            assert_eq!(GroupMode::from_key(mode.key()), Some(mode));
        }
        assert_eq!(GroupMode::from_key("unknown"), None);
        assert_eq!(GroupMode::default(), GroupMode::None);
        assert_eq!(GroupMode::None.label(), "不分组");
    }
}
