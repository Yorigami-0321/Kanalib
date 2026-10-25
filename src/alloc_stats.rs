//! 取证档全局分配器：按 2 的幂尺寸档统计存活字节，定位「谁持有 2GB」。
//! 只在 frame-profiler feature 下编译进二进制；日志由 KANALIB_ALLOC_STATS=<秒> 门控。
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Once;

const MIN_SHIFT: u32 = 10;
const MAX_SHIFT: u32 = 26;
const BUCKETS: usize = (MAX_SHIFT - MIN_SHIFT + 1) as usize;
/// (128K, 256K] 档：256×256×4 BGRA 解码缓冲所在档。
const TRACE_BUCKET: usize = (18 - MIN_SHIFT) as usize;

static LIVE: [AtomicU64; BUCKETS] = [const { AtomicU64::new(0) }; BUCKETS];
static LIVE_TOTAL: AtomicU64 = AtomicU64::new(0);
static ALLOC_CNT: [AtomicU64; BUCKETS] = [const { AtomicU64::new(0) }; BUCKETS];
static FREE_CNT: [AtomicU64; BUCKETS] = [const { AtomicU64::new(0) }; BUCKETS];

fn bucket(size: usize) -> Option<usize> {
    if size == 0 {
        return None;
    }
    let shift = size.next_power_of_two().trailing_zeros();
    if shift < MIN_SHIFT || shift > MAX_SHIFT {
        return None;
    }
    Some((shift - MIN_SHIFT) as usize)
}

pub struct CountingAlloc;

/// 取证档：KANALIB_ALLOC_TRACES=1 时记录 256K 档（shift 18）每笔存活分配的调用栈，
/// dealloc 时摘除；scroll 结束时 dump 仍存活者的栈分组，直接指认持有路径。
pub mod traces {
    use std::backtrace::Backtrace;
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    static ENABLED: OnceLock<bool> = OnceLock::new();

    pub fn enabled() -> bool {
        *ENABLED.get_or_init(|| std::env::var_os("KANALIB_ALLOC_TRACES").is_some())
    }

    static LIVE_TRACES: Mutex<Option<HashMap<usize, (usize, Backtrace)>>> = Mutex::new(None);

    // dbghelp 捕获不是线程安全的：捕获全程持锁；thread-local 防嵌套分配重入自锁。
    thread_local! {
        static INSIDE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    fn map() -> std::sync::MutexGuard<'static, Option<HashMap<usize, (usize, Backtrace)>>> {
        let guard = LIVE_TRACES.lock().unwrap_or_else(|poison| poison.into_inner());
        guard
    }

    pub fn record(ptr: *mut u8, size: usize) {
        if INSIDE.with(|inside| inside.get()) {
            return;
        }
        INSIDE.with(|inside| inside.set(true));
        let mut guard = map();
        let trace = Backtrace::force_capture();
        guard
            .get_or_insert_with(HashMap::new)
            .insert(ptr as usize, (size, trace));
        drop(guard);
        INSIDE.with(|inside| inside.set(false));
    }

    pub fn forget(ptr: *mut u8) {
        if INSIDE.with(|inside| inside.get()) {
            return;
        }
        INSIDE.with(|inside| inside.set(true));
        let mut guard = map();
        if let Some(map) = guard.as_mut() {
            map.remove(&(ptr as usize));
        }
        drop(guard);
        INSIDE.with(|inside| inside.set(false));
    }

    pub fn dump() {
        if INSIDE.with(|inside| inside.get()) {
            return;
        }
        INSIDE.with(|inside| inside.set(true));
        let guard = map();
        let Some(map) = guard.as_ref() else {
            log::info!("alloc traces: none recorded");
            INSIDE.with(|inside| inside.set(false));
            return;
        };
        let mut groups: HashMap<String, (usize, usize, String)> = HashMap::new();
        for (_, (size, trace)) in map.iter() {
            let rendered = format!("{trace}");
            let key = rendered.clone();
            let entry = groups.entry(key).or_insert((0, 0, rendered));
            entry.0 += 1;
            entry.1 += size;
        }
        let mut ordered: Vec<_> = groups.into_values().collect();
        ordered.sort_by(|a, b| b.1.cmp(&a.1));
        log::info!("alloc traces: live_blocks={} groups={}", map.len(), ordered.len());
        for (count, bytes, sample) in ordered.into_iter().take(4) {
            let head: String = sample
                .lines()
                .skip(2)
                .take(60)
                .collect::<Vec<_>>()
                .join(" | ");
            log::info!("alloc trace group: blocks={count} mb={:.1} stack={head}", bytes as f64 / 1e6);
        }
        INSIDE.with(|inside| inside.set(false));
    }
}

unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc(layout);
        if !ptr.is_null() {
            LIVE_TOTAL.fetch_add(layout.size() as u64, Relaxed);
            if let Some(index) = bucket(layout.size()) {
                LIVE[index].fetch_add(layout.size() as u64, Relaxed);
                ALLOC_CNT[index].fetch_add(1, Relaxed);
                if index == TRACE_BUCKET && traces::enabled() {
                    traces::record(ptr, layout.size());
                }
            }
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = System.alloc_zeroed(layout);
        if !ptr.is_null() {
            LIVE_TOTAL.fetch_add(layout.size() as u64, Relaxed);
            if let Some(index) = bucket(layout.size()) {
                LIVE[index].fetch_add(layout.size() as u64, Relaxed);
                ALLOC_CNT[index].fetch_add(1, Relaxed);
                if index == TRACE_BUCKET && traces::enabled() {
                    traces::record(ptr, layout.size());
                }
            }
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE_TOTAL.fetch_sub(layout.size() as u64, Relaxed);
        if let Some(index) = bucket(layout.size()) {
            LIVE[index].fetch_sub(layout.size() as u64, Relaxed);
            FREE_CNT[index].fetch_add(1, Relaxed);
            if index == TRACE_BUCKET && traces::enabled() {
                traces::forget(ptr);
            }
        }
        System.dealloc(ptr, layout)
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_ptr = System.realloc(ptr, layout, new_size);
        if !new_ptr.is_null() {
            LIVE_TOTAL.fetch_add(new_size as u64, Relaxed);
            LIVE_TOTAL.fetch_sub(layout.size() as u64, Relaxed);
            if let Some(index) = bucket(layout.size()) {
                LIVE[index].fetch_sub(layout.size() as u64, Relaxed);
                FREE_CNT[index].fetch_add(1, Relaxed);
                if index == TRACE_BUCKET && traces::enabled() {
                    traces::forget(ptr);
                }
            }
            if let Some(index) = bucket(new_size) {
                LIVE[index].fetch_add(new_size as u64, Relaxed);
                ALLOC_CNT[index].fetch_add(1, Relaxed);
                if index == TRACE_BUCKET && traces::enabled() {
                    traces::record(new_ptr, new_size);
                }
            }
        }
        new_ptr
    }
}

pub fn live_bucket_bytes(index: usize) -> u64 {
    LIVE.get(index)
        .map(|counter| counter.load(Relaxed))
        .unwrap_or_default()
}

pub fn spawn_logger() {
    let Some(interval) = std::env::var("KANALIB_ALLOC_STATS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
    else {
        return;
    };
    static START: Once = Once::new();
    START.call_once(|| {
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_secs(interval));
            let mut parts = Vec::new();
            for (index, counter) in LIVE.iter().enumerate() {
                let bytes = counter.load(Relaxed);
                if bytes > 0 {
                    let shift = MIN_SHIFT + index as u32;
                    parts.push(format!("\"{}K_mb\":{:.1}", 1u64 << shift >> 10, bytes as f64 / 1e6));
                }
            }
            let allocs = ALLOC_CNT[TRACE_BUCKET].load(Relaxed);
            let frees = FREE_CNT[TRACE_BUCKET].load(Relaxed);
            let counts = format!(
                "\"256K_cnt\":{{\"alloc\":{allocs},\"free\":{frees},\"live_blk\":{}}}",
                allocs as i64 - frees as i64,
            );
            log::info!(
                "alloc stats: {{\"total_mb\":{:.1},{},{}}}",
                LIVE_TOTAL.load(Relaxed) as f64 / 1e6,
                parts.join(","),
                counts,
            );
        });
    });
}
