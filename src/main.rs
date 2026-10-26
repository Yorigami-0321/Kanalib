#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "frame-profiler")]
#[global_allocator]
static ALLOC: kanalib_core::alloc_stats::CountingAlloc = kanalib_core::alloc_stats::CountingAlloc;

use std::path::PathBuf;

use ely_gpui_component::{
    Assets,
    i18n::I18n,
    theme::{Mode, Theme},
};
use gpui::{App, AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use kanalib_core::{sidecar, state};
use kanalib_ui::KanalibApp;

fn init_logging() {
    let log_path = std::env::var_os("KANALIB_LOG_FILE")
        .map(PathBuf::from)
        .or_else(|| {
            if std::env::var_os("KANALIB_HEADLESS").is_some() {
                let dir = sidecar::project_root().join(".kanalib");
                std::fs::create_dir_all(&dir).ok()?;
                Some(dir.join("headless.log"))
            } else {
                None
            }
        });
    let mut builder =
        env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"));
    if let Some(path) = log_path {
        match std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            Ok(file) => {
                builder.target(env_logger::Target::Pipe(Box::new(file)));
            }
            Err(error) => eprintln!("open log file {}: {error}", path.display()),
        }
    }
    builder.init();
}

fn main() {
    init_logging();
    #[cfg(feature = "frame-profiler")]
    kanalib_core::alloc_stats::spawn_logger();
    let folder = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("KANALIB_FOLDER").map(PathBuf::from));

    if std::env::var_os("KANALIB_HEADLESS").is_some() {
        let state = state::AppState::new(folder.clone());
        if std::env::var_os("KANALIB_SEARCH_ONLY").is_none() {
            let skip_tag = std::env::var_os("KANALIB_SKIP_TAG").is_some();
            match state.scan_blocking(skip_tag) {
                Ok(()) => log::info!("headless scan finished"),
                Err(error) => {
                    log::error!("headless scan failed: {error:#}");
                    std::process::exit(1);
                }
            }
        }
        {
            let shared = state.shared.lock().unwrap();
            let summary = shared.dedup.summary(kanalib_core::dedup::GroupKind::Exact);
            log::info!(
                "dedup summary: groups={} pending={} pending_assets={} ignored={} done={} pending_bytes={}",
                summary.total,
                summary.pending,
                summary.pending_assets,
                summary.ignored,
                summary.done,
                summary.pending_bytes
            );
            for group in shared.dedup.groups.iter().take(5) {
                log::info!(
                    "dedup group: key={} assets={} keeper={} reason={} bytes={}",
                    &group.key[..12.min(group.key.len())],
                    group.assets.len(),
                    group
                        .keeper
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_default(),
                    group.suggested_reason,
                    group.reclaimable()
                );
            }
        }
        let runs = std::env::var("KANALIB_SEARCH_RUNS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(100)
            .max(1);
        for query in ["synthetic", "合成", "sky", "1girl"] {
            let mut durations = Vec::with_capacity(runs);
            let mut hits = 0usize;
            for _ in 0..runs {
                let start = std::time::Instant::now();
                match state.search_paths(query) {
                    Ok(paths) => hits = paths.len(),
                    Err(error) => log::warn!("search {query}: {error:#}"),
                }
                durations.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            durations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let p50 = durations[(runs - 1) / 2];
            let p95 = durations[((runs * 95) / 100).min(runs - 1)];
            let max = durations[runs - 1];
            log::info!(
                "search metrics: query={query} hits={hits} runs={runs} p50_ms={p50:.2} p95_ms={p95:.2} max_ms={max:.2}"
            );
        }
        if std::env::var_os("KANALIB_SIMILAR_SCAN").is_some() {
            let assets = state.shared.lock().unwrap().assets.read().unwrap().clone();
            let reviews = std::collections::HashMap::new();
            for threshold in [6u8, 10, 14] {
                let started = std::time::Instant::now();
                let groups = kanalib_core::dedup::compute_similar_groups(
                    &assets,
                    threshold,
                    &reviews,
                    &[],
                    &[],
                );
                let members: usize = groups.iter().map(|group| group.assets.len()).sum();
                log::info!(
                    "similar metrics: threshold={threshold} assets={} groups={} members={} ms={:.1}",
                    assets.len(),
                    groups.len(),
                    members,
                    started.elapsed().as_secs_f64() * 1000.0
                );
            }
        }
        return;
    }

    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            ely_gpui_component::init(cx).expect("Ely failed to start");
            cx.set_global(I18n::new("zh-CN"));
            Theme::set_mode_now(Mode::Dark, cx);
            Theme::update(cx, |theme| {
                theme.font_family = "Microsoft YaHei UI".into();
                theme.mono_family = "Cascadia Mono".into();
            });

            let work_area = cx
                .primary_display()
                .map(|display| display.visible_bounds().size);
            let window_size = work_area
                .map(|work| {
                    size(
                        px((work.width.as_f32() - 80.).min(1280.).max(960.)),
                        px((work.height.as_f32() - 80.).min(820.).max(640.)),
                    )
                })
                .unwrap_or_else(|| size(px(1280.), px(820.)));
            let bounds = Bounds::centered(None, window_size, cx);
            let view = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        titlebar: Some(TitlebarOptions {
                            title: Some("Kanalib".into()),
                            appears_transparent: true,
                            traffic_light_position: None,
                        }),
                        window_min_size: Some(size(px(900.), px(660.))),
                        ..Default::default()
                    },
                    move |_window, cx| {
                        let folder = folder.clone();
                        cx.new(|cx| KanalibApp::new(folder, cx))
                    },
                )
                .expect("open Kanalib window");
            let _ = view.update(cx, |app, _window, cx| app.start(cx));
            cx.activate(true);
        });
}
