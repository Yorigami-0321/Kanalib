#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(feature = "frame-profiler")]
#[global_allocator]
static ALLOC: kanalib_core::alloc_stats::CountingAlloc = kanalib_core::alloc_stats::CountingAlloc;

use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use ely_gpui_component::{
    Assets,
    i18n::I18n,
    theme::{Mode, Theme},
};
use gpui::{App, AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};
use kanalib_core::state;
use kanalib_ui::{
    KanalibApp, MIN_WINDOW_HEIGHT, MIN_WINDOW_WIDTH, WindowState, WorkArea,
};

mod logging;

/// F-039：单实例锁必须活到进程退出；`Application::run` 的启动闭包返回后会被
/// 释放，所以放在静态槽里而不是闭包捕获。
static INSTANCE_GUARD: Mutex<Option<single_instance::SingleInstance>> = Mutex::new(None);

#[cfg(windows)]
mod single_instance {
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::JoinHandle;
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, OpenEventW, ReleaseMutex, SetEvent,
        WaitForSingleObject,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        ASFW_ANY, AllowSetForegroundWindow,
    };

    pub enum Startup {
        /// 本进程持有命名 mutex，并在后台等待激活事件。
        Primary(SingleInstance),
        /// 已有实例；激活事件已 SetEvent，调用方应立即 exit(0)。
        Secondary,
        /// 锁/事件创建失败；按普通启动继续，不影响可用性。
        Degraded,
    }

    pub struct SingleInstance {
        activation: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        _handles: Arc<SharedHandles>,
        _thread: Option<JoinHandle<()>>,
    }

    impl SingleInstance {
        pub fn activation(&self) -> Arc<AtomicBool> {
            Arc::clone(&self.activation)
        }
    }

    impl Drop for SingleInstance {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
        }
    }

    struct SharedHandles {
        mutex: HANDLE,
        event: HANDLE,
    }

    // 命名内核对象句柄本身可跨线程使用；等待线程只读 event。
    unsafe impl Send for SharedHandles {}
    unsafe impl Sync for SharedHandles {}

    impl Drop for SharedHandles {
        fn drop(&mut self) {
            unsafe {
                if !self.event.is_null() {
                    CloseHandle(self.event);
                }
                if !self.mutex.is_null() {
                    ReleaseMutex(self.mutex);
                    CloseHandle(self.mutex);
                }
            }
        }
    }

    /// 数据根规范化后稳定哈希；`KANALIB_ROOT`、便携版 exe 目录与不同副本天然隔离。
    pub fn normalized_root(root: &Path) -> String {
        let absolute = std::path::absolute(root).unwrap_or_else(|_| root.to_path_buf());
        let mut text = absolute.to_string_lossy().replace('/', "\\");
        while text.len() > 3 && text.ends_with('\\') {
            text.pop();
        }
        text.to_lowercase()
    }

    fn stable_hash(text: &str) -> u64 {
        // FNV-1a 64：无第三方依赖，跨进程/跨启动稳定。
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in text.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash
    }

    pub fn instance_key(root: &Path) -> String {
        format!("{:016x}", stable_hash(&normalized_root(root)))
    }

    pub fn mutex_name(root: &Path) -> String {
        format!("Local\\Kanalib.SingleInstance.{}", instance_key(root))
    }

    pub fn event_name(root: &Path) -> String {
        format!("Local\\Kanalib.Activate.{}", instance_key(root))
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn acquire(root: &Path) -> Startup {
        let mutex = wide(&mutex_name(root));
        let event = wide(&event_name(root));
        unsafe {
            let mutex_handle = CreateMutexW(std::ptr::null(), 1, mutex.as_ptr());
            if mutex_handle.is_null() {
                log::warn!(
                    "single instance: CreateMutexW failed: {}; continuing without guard",
                    std::io::Error::last_os_error()
                );
                return Startup::Degraded;
            }
            if GetLastError() == ERROR_ALREADY_EXISTS {
                CloseHandle(mutex_handle);
                // 第二个实例刚由用户启动，借此把前台切换权限转交给已有实例，
                // 否则 Windows 前台锁可能只闪任务栏。只授权、不注入输入。
                AllowSetForegroundWindow(ASFW_ANY);
                return match signal_existing(&event) {
                    Ok(()) => {
                        log::info!("single instance: another instance owns this data root; activation sent");
                        Startup::Secondary
                    }
                    Err(error) => {
                        log::warn!(
                            "single instance: activation event unavailable ({error}); continuing without guard"
                        );
                        Startup::Degraded
                    }
                };
            }
            let event_handle = CreateEventW(std::ptr::null(), 0, 0, event.as_ptr());
            if event_handle.is_null() {
                let error = std::io::Error::last_os_error();
                log::warn!("single instance: CreateEventW failed: {error}; continuing without guard");
                ReleaseMutex(mutex_handle);
                CloseHandle(mutex_handle);
                return Startup::Degraded;
            }
            let handles = Arc::new(SharedHandles {
                mutex: mutex_handle,
                event: event_handle,
            });
            let activation = Arc::new(AtomicBool::new(false));
            let stop = Arc::new(AtomicBool::new(false));
            let thread = {
                let handles = Arc::clone(&handles);
                let activation = Arc::clone(&activation);
                let stop = Arc::clone(&stop);
                std::thread::Builder::new()
                    .name("kanalib-instance".into())
                    .spawn(move || wait_loop(&handles, &activation, &stop))
                    .map_err(|error| log::warn!("single instance: waiter thread: {error}"))
                    .ok()
            };
            log::info!(
                "single instance: lock acquired for {}",
                normalized_root(root)
            );
            Startup::Primary(SingleInstance {
                activation,
                stop,
                _handles: handles,
                _thread: thread,
            })
        }
    }

    /// 首个实例可能在创建 mutex 与 event 之间被第二个进程撞上，短暂重试。
    fn signal_existing(event_name: &[u16]) -> std::io::Result<()> {
        for attempt in 0..20 {
            unsafe {
                let event = OpenEventW(EVENT_MODIFY_STATE, 0, event_name.as_ptr());
                if !event.is_null() {
                    let signaled = SetEvent(event) != 0;
                    let error = std::io::Error::last_os_error();
                    CloseHandle(event);
                    return if signaled { Ok(()) } else { Err(error) };
                }
            }
            if attempt < 19 {
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        Err(std::io::Error::last_os_error())
    }

    fn wait_loop(handles: &SharedHandles, activation: &AtomicBool, stop: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) {
            match unsafe { WaitForSingleObject(handles.event, 200) } {
                WAIT_OBJECT_0 => activation.store(true, Ordering::SeqCst),
                WAIT_TIMEOUT => {}
                _ => {
                    log::warn!(
                        "single instance: WaitForSingleObject failed: {}",
                        std::io::Error::last_os_error()
                    );
                    break;
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod single_instance {
    use std::path::Path;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    #[allow(dead_code)]
    pub enum Startup {
        Primary(SingleInstance),
        Secondary,
        Degraded,
    }

    #[allow(dead_code)]
    pub struct SingleInstance {
        activation: Arc<AtomicBool>,
    }

    impl SingleInstance {
        pub fn activation(&self) -> Arc<AtomicBool> {
            Arc::clone(&self.activation)
        }
    }

    pub fn acquire(_root: &Path) -> Startup {
        Startup::Degraded
    }
}

fn acquire_single_instance(root: &std::path::Path) -> Option<Arc<AtomicBool>> {
    match single_instance::acquire(root) {
        single_instance::Startup::Primary(guard) => {
            let activation = guard.activation();
            match INSTANCE_GUARD.lock() {
                Ok(mut slot) => *slot = Some(guard),
                Err(_) => log::warn!("single instance: guard slot poisoned"),
            }
            Some(activation)
        }
        single_instance::Startup::Secondary => {
            log::info!("another Kanalib instance is running; activation signal sent, exiting");
            std::process::exit(0);
        }
        single_instance::Startup::Degraded => None,
    }
}

fn work_areas(cx: &App) -> Vec<WorkArea> {
    let primary_id = cx.primary_display().map(|display| display.id());
    cx.displays()
        .iter()
        .map(|display| WorkArea {
            display_id: u64::from(display.id()),
            bounds: display.visible_bounds(),
            primary: Some(display.id()) == primary_id,
        })
        .collect()
}

fn main() {
    let log_path = logging::init();
    if let Some(path) = &log_path {
        log::info!("Kanalib starting; log file: {}", path.display());
    }
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

    let data_root = kanalib_core::sidecar::project_root();
    let activation = acquire_single_instance(&data_root);

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
            // F-038：优先恢复上次窗口状态并夹取到当前显示器可见工作区；
            // 文件缺失/损坏或没有显示器时回退原居中逻辑。
            let restored = WindowState::load(&data_root)
                .and_then(|state| state.resolve(&work_areas(cx)));
            let (window_bounds, restored_display) = match restored {
                Some(resolved) => {
                    let bounds = resolved.state.to_bounds();
                    let bounds = if resolved.state.maximized {
                        WindowBounds::Maximized(bounds)
                    } else {
                        WindowBounds::Windowed(bounds)
                    };
                    log::info!(
                        "restored window state: x={} y={} width={} height={} maximized={} display={}",
                        resolved.state.x,
                        resolved.state.y,
                        resolved.state.width,
                        resolved.state.height,
                        resolved.state.maximized,
                        resolved.display_id
                    );
                    (bounds, Some(resolved.display_id))
                }
                None => (
                    WindowBounds::Windowed(Bounds::centered(None, window_size, cx)),
                    None,
                ),
            };
            let view = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(window_bounds),
                        titlebar: Some(TitlebarOptions {
                            title: Some("Kanalib".into()),
                            appears_transparent: true,
                            traffic_light_position: None,
                        }),
                        window_min_size: Some(size(
                            px(MIN_WINDOW_WIDTH),
                            px(MIN_WINDOW_HEIGHT),
                        )),
                        display_id: restored_display.map(Into::into),
                        ..Default::default()
                    },
                    move |_window, cx| {
                        let folder = folder.clone();
                        cx.new(|cx| KanalibApp::new(folder, cx))
                    },
                )
                .expect("open Kanalib window");
            let activation = activation.clone();
            let _ = view.update(cx, |app, window, cx| {
                app.start(window, cx);
                // F-039：后台线程只置位；在 GPUI 主线程轮询并置前窗口。
                if let Some(activation) = activation {
                    cx.spawn_in(window, async move |_weak, cx| loop {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(120))
                            .await;
                        if activation.swap(false, std::sync::atomic::Ordering::SeqCst) {
                            let activated = cx.update(|window, cx| {
                                window.activate_window();
                                cx.activate(true);
                            });
                            if activated.is_err() {
                                break;
                            }
                            log::info!("single instance: existing window brought to front");
                        }
                    })
                    .detach();
                }
            });
            cx.activate(true);
        });
}

#[cfg(all(test, windows))]
mod single_instance_tests {
    use super::single_instance::{event_name, instance_key, mutex_name, normalized_root};
    use std::path::Path;

    #[test]
    fn instance_key_ignores_path_spelling() {
        let forward = normalized_root(Path::new("target/r5_task_a/root"));
        let backward = normalized_root(Path::new(r"target\r5_task_a\root\"));
        assert_eq!(forward, backward);
        assert_eq!(
            instance_key(Path::new("target/r5_task_a/root")),
            instance_key(Path::new(r"target\r5_task_a\root\"))
        );
        assert_eq!(
            event_name(Path::new(r"D:\Data")),
            event_name(Path::new(r"d:/data"))
        );
    }

    #[test]
    fn instance_names_differ_per_data_root() {
        assert_ne!(
            mutex_name(Path::new("D:/kanalib-one")),
            mutex_name(Path::new("D:/kanalib-two"))
        );
        assert_ne!(
            event_name(Path::new("D:/kanalib-one")),
            event_name(Path::new("D:/kanalib-two"))
        );
    }

    #[test]
    fn instance_names_are_scoped_and_hashed() {
        let mutex = mutex_name(Path::new("D:/Data"));
        let event = event_name(Path::new("D:/Data"));
        assert!(mutex.starts_with("Local\\Kanalib.SingleInstance."));
        assert!(event.starts_with("Local\\Kanalib.Activate."));
        let key = mutex.rsplit('.').next().expect("hash suffix");
        assert_eq!(key.len(), 16);
        assert!(key.chars().all(|ch| ch.is_ascii_hexdigit()));
    }
}
