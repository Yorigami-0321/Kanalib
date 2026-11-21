#!/usr/bin/env node
/**
 * 图片墙性能与可读性基准：
 *   A. 1440×900 下连续拖拽 30 秒的 JS 堆 / DOM 卡片数 / 帧间隔 / 长任务曲线（status §6 P1-8）
 *   B. 侧栏半透明纸叠在真实底图上的「实际像素」对比度（status §5.4 / §6 P2-13）
 *   C. 同一段拖拽关掉 backdrop-filter 后再跑一遍，比较帧率差（status §6 P2-13）
 *
 * 只依赖 Node 22 自带的 WebSocket / fetch / child_process，不引 puppeteer/playwright。
 * 流程：起 Chrome（--headless=new，随机调试端口）→ CDP 连页面 → 钉视口 →
 *       拖拽采样 → 截图回传页面用 canvas 解码像素 → 落 perf-report.json。
 *
 * 用法：
 *   node scripts/wall_perf.mjs --base http://127.0.0.1:4318/ --out web/screenshots
 *
 * 参数：
 *   --base    站点根（补结尾 /），默认 http://127.0.0.1:4318/
 *   --out     截图与 perf-report.json 的落脚目录，默认 web/screenshots
 *   --seconds 每段拖拽时长（秒），默认 30
 *   --chrome  Chrome/Edge 可执行文件路径（默认顺序探测）
 *   --keep    保留 profile 目录（排查用）
 *
 * 退出码：0 全部断言通过；1 有断言不达标（failures 非空）；2 环境问题（没有浏览器）。
 *
 * 口径说明（重要）：
 *   headless=new 没有真实 vsync，requestAnimationFrame 实测不受 60Hz 限流（基准 ≈145Hz），
 *   所以帧间隔反映的是「主线程还能多快产出一帧」，不是显示器刷新节奏。断言仍用
 *   p95 ≤ 33.3ms（两个 60Hz 帧周期）作为卡顿红线，并同时给出 baselineHz 与
 *   Performance.getMetrics 的主线程占用率（TaskDuration 增量 / 墙钟），后者不受 rAF 影响。
 */
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const CANDIDATES = [
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
  'C:/Program Files/Microsoft/Edge/Application/msedge.exe',
];

const VIEWPORT = { width: 1440, height: 900 };
const SEAM_STRIP = 120;
const MAX_DOM_TILES = 220;
const MAX_HEAP_GROWTH = 0.4;
const FRAME_BUDGET_MS = 33.3;

function parseArgs(argv) {
  const out = { base: 'http://127.0.0.1:4318/', out: 'web/screenshots', seconds: 30, chrome: '', keep: false };
  for (let index = 0; index < argv.length; index += 1) {
    const key = argv[index];
    const value = argv[index + 1];
    if (key === '--keep') {
      out.keep = true;
      continue;
    }
    if (key === '--base' && value) {
      out.base = value.endsWith('/') ? value : value + '/';
      index += 1;
    } else if (key === '--out' && value) {
      out.out = value;
      index += 1;
    } else if (key === '--seconds' && value) {
      out.seconds = Math.max(5, Number(value) || 30);
      index += 1;
    } else if (key === '--chrome' && value) {
      out.chrome = value;
      index += 1;
    }
  }
  return out;
}

function findChrome(explicit) {
  if (explicit) return existsSync(explicit) ? explicit : '';
  return CANDIDATES.find((path) => existsSync(path)) ?? '';
}

/** 从 wall_shots.mjs 复制的零依赖 CDP 客户端，保持独立便于单跑。 */
class Cdp {
  constructor(url) {
    this.url = url;
    this.nextId = 1;
    this.pending = new Map();
    this.listeners = [];
  }

  async connect() {
    this.ws = new WebSocket(this.url);
    await new Promise((resolvePromise, reject) => {
      this.ws.addEventListener('open', resolvePromise, { once: true });
      this.ws.addEventListener('error', () => reject(new Error('CDP 连接失败')), { once: true });
    });
    this.ws.addEventListener('message', (event) => {
      const message = JSON.parse(typeof event.data === 'string' ? event.data : '');
      if (message.id && this.pending.has(message.id)) {
        const { resolve: done, reject } = this.pending.get(message.id);
        this.pending.delete(message.id);
        if (message.error) reject(new Error(message.method + ': ' + message.error.message));
        else done(message.result);
        return;
      }
      for (const listener of this.listeners) listener(message);
    });
  }

  send(method, params = {}) {
    const id = this.nextId++;
    const payload = JSON.stringify({ id, method, params });
    return new Promise((resolvePromise, reject) => {
      this.pending.set(id, { resolve: resolvePromise, reject });
      this.ws.send(payload);
    });
  }

  on(listener) {
    this.listeners.push(listener);
  }

  close() {
    try {
      this.ws.close();
    } catch {
      /* 已关闭 */
    }
  }
}

async function waitForDevtools(port, timeoutMs = 12000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (response.ok) return await response.json();
    } catch {
      /* 还没起来 */
    }
    await sleep(150);
  }
  throw new Error('Chrome 调试端口没起来');
}

async function newTarget(port, url) {
  const response = await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent(url)}`, { method: 'PUT' });
  if (!response.ok) throw new Error('建页面失败：' + response.status);
  return await response.json();
}

/** 永久删除临时目录（不送回收站）：Chrome profile 属于可再生成产物。 */
function hardDelete(dir) {
  if (!dir || !existsSync(dir)) return;
  if (process.platform === 'win32') {
    spawnSync('cmd', ['/c', 'rd', '/s', '/q', dir], { stdio: 'ignore' });
    if (!existsSync(dir)) return;
  }
  try {
    rmSync(dir, { recursive: true, force: true });
  } catch {
    /* 删不掉就算了 */
  }
}

/**
 * 注入页面侧的采集工具。整段是字符串，避免与 Node 侧模板串互相转义。
 * 全部挂在 window.__wallPerf 上，一次注入、多次调用。
 */
const PAGE_HELPERS = `
window.__wallPerf = (() => {
  const lin = (value) => {
    const c = value / 255;
    return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  };
  const lumOf = (r, g, b) => 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
  const median = (list) => {
    if (!list.length) return 0;
    const sorted = list.slice().sort((a, b) => a - b);
    return sorted[Math.floor((sorted.length - 1) / 2)];
  };
  const quantile = (list, q) => {
    if (!list.length) return 0;
    const sorted = list.slice().sort((a, b) => a - b);
    return sorted[Math.min(sorted.length - 1, Math.max(0, Math.round((sorted.length - 1) * q)))];
  };

  const state = { deltas: [], longTasks: [], running: false, observer: null, session: Math.random().toString(36).slice(2) };

  const resetFrameProbe = () => {
    state.deltas = [];
    state.longTasks = [];
    state.running = true;
    let last = performance.now();
    const tick = (now) => {
      state.deltas.push(now - last);
      last = now;
      if (state.running) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
    try {
      if (state.observer) state.observer.disconnect();
      state.observer = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) state.longTasks.push(entry.duration);
      });
      state.observer.observe({ type: 'longtask', buffered: false });
    } catch (error) {
      state.observer = null;
    }
    return true;
  };

  const stopFrameProbe = () => {
    state.running = false;
    try {
      if (state.observer) state.observer.disconnect();
    } catch (error) {
      /* observer 已断开 */
    }
    // 去掉前两帧：rAF 起步那一帧的间隔含探针注入耗时。
    const deltas = state.deltas.slice(2).sort((a, b) => a - b);
    const total = deltas.length;
    const dropped = deltas.filter((value) => value > 33.3).length;
    const windowMs = state.deltas.reduce((sum, value) => sum + value, 0);
    const p50 = quantile(deltas, 0.5);
    return {
      frames: total,
      p50: Number(p50.toFixed(2)),
      p95: Number(quantile(deltas, 0.95).toFixed(2)),
      p99: Number(quantile(deltas, 0.99).toFixed(2)),
      max: Number(quantile(deltas, 1).toFixed(2)),
      droppedPct: total ? Number(((dropped / total) * 100).toFixed(2)) : 0,
      baselineHz: p50 > 0 ? Math.round(1000 / p50) : 0,
      windowMs: Math.round(windowMs),
      longTasks: {
        count: state.longTasks.length,
        totalMs: Number(state.longTasks.reduce((sum, value) => sum + value, 0).toFixed(1)),
        maxMs: Number(quantile(state.longTasks, 1).toFixed(1)),
      },
    };
  };

  const tileCount = () => document.querySelectorAll('.tile').length;
  const loadedCount = () => document.querySelectorAll('.tile img.is-loaded').length;
  const session = () => state.session;

  /** 找画面里最亮的若干张已加载图（8×8 缩略算均值与 p75），用于把亮图拖到侧栏底下。
   *  排序用 p75：渐变图均值会被暗角拉低，p75 更能代表「一大片都亮」。 */
  const brightTiles = (limit) => {
    const canvas = document.createElement('canvas');
    canvas.width = 8;
    canvas.height = 8;
    const ctx = canvas.getContext('2d', { willReadFrequently: true });
    const out = [];
    for (const tile of document.querySelectorAll('.tile')) {
      const img = tile.querySelector('img.is-loaded');
      if (!img) continue;
      const box = tile.getBoundingClientRect();
      if (box.width < 60 || box.height < 60) continue;
      try {
        ctx.clearRect(0, 0, 8, 8);
        ctx.drawImage(img, 0, 0, 8, 8);
        const data = ctx.getImageData(0, 0, 8, 8).data;
        const samples = [];
        for (let i = 0; i < data.length; i += 4) samples.push(lumOf(data[i], data[i + 1], data[i + 2]));
        samples.sort((a, b) => a - b);
        const mean = samples.reduce((sum, value) => sum + value, 0) / samples.length;
        out.push({
          id: tile.dataset.id || '',
          lum: Number(mean.toFixed(4)),
          p75: Number(samples[Math.floor((samples.length - 1) * 0.75)].toFixed(4)),
          rect: { left: box.left, top: box.top, width: box.width, height: box.height },
        });
      } catch (error) {
        /* 取不到像素（未解码）就跳过 */
      }
    }
    out.sort((a, b) => b.p75 - a.p75);
    return out.slice(0, limit);
  };

  const tileRect = (id) => {
    for (const tile of document.querySelectorAll('.tile')) {
      if (tile.dataset.id !== id) continue;
      const box = tile.getBoundingClientRect();
      return { left: box.left, top: box.top, width: box.width, height: box.height };
    }
    return null;
  };

  /** 把任意 CSS 颜色解析成 0–255 的 rgb + alpha（兼容 color(srgb r g b / a) 与 rgba()）。 */
  const parseColor = (value) => {
    const numbers = (value.match(/[\\d.]+/g) || []).map(Number);
    if (value.indexOf('color(') === 0) {
      return {
        r: numbers[0] * 255,
        g: numbers[1] * 255,
        b: numbers[2] * 255,
        a: numbers.length > 3 ? numbers[3] : 1,
      };
    }
    return { r: numbers[0] || 0, g: numbers[1] || 0, b: numbers[2] || 0, a: numbers.length > 3 ? numbers[3] : 1 };
  };

  /** 理论最坏底：把「纯白图片」垫在 76% 纸面下面，量纸面与 72% 次级文字的名义对比度。
   *  实际笔画有抗锯齿，真实值会更低；这个数只用来回答「亮图有没有可能把可读性压破」。 */
  const seamBound = () => {
    const seam = document.querySelector('.seam');
    if (!seam) return null;
    const paper = parseColor(getComputedStyle(seam).backgroundColor);
    const labels = [...document.querySelectorAll('.seam__label')];
    const label = labels.find((element) => !element.closest('.is-on')) || labels[0];
    if (!label) return null;
    const ink = parseColor(getComputedStyle(label).color);
    const over = (color, alpha) => ({
      r: alpha * color.r + (1 - alpha) * 255,
      g: alpha * color.g + (1 - alpha) * 255,
      b: alpha * color.b + (1 - alpha) * 255,
    });
    const paperOnWhite = over({ r: paper.r, g: paper.g, b: paper.b }, paper.a);
    const textOnPaper = {
      r: ink.a * ink.r + (1 - ink.a) * paperOnWhite.r,
      g: ink.a * ink.g + (1 - ink.a) * paperOnWhite.g,
      b: ink.a * ink.b + (1 - ink.a) * paperOnWhite.b,
    };
    const fg = lumOf(textOnPaper.r, textOnPaper.g, textOnPaper.b);
    const bg = lumOf(paperOnWhite.r, paperOnWhite.g, paperOnWhite.b);
    return {
      paperAlpha: Number(paper.a.toFixed(2)),
      inkAlpha: Number(ink.a.toFixed(2)),
      fg: Number(fg.toFixed(4)),
      bg: Number(bg.toFixed(4)),
      contrast: Number(((fg + 0.05) / (bg + 0.05)).toFixed(2)),
    };
  };

  /**
   * 把整屏截图解码成像素，逐条侧栏文字量「笔画 vs 紧邻纸面」的实际对比度。
   * 前景：文字矩形内亮度前 10% 的中位数（笔画核心，含抗锯齿的保守取值）。
   * 背景：文字矩形上/下 6–18px、左/右同高度（按 16px 切片）的纸面中位数，取最亮的一侧——
   *       纸面最亮的那个方向才是可读性的最坏情况。
   */
  const analyzeSeam = async (base64) => {
    const image = new Image();
    image.src = 'data:image/png;base64,' + base64;
    await image.decode();
    const width = image.naturalWidth;
    const height = image.naturalHeight;
    const canvas = document.createElement('canvas');
    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext('2d', { willReadFrequently: true });
    ctx.drawImage(image, 0, 0);
    const data = ctx.getImageData(0, 0, width, height).data;
    const lum = new Float32Array(width * height);
    for (let i = 0, p = 0; i < data.length; i += 4, p += 1) {
      lum[p] = lumOf(data[i], data[i + 1], data[i + 2]);
    }
    const collect = (x0, y0, x1, y1) => {
      const values = [];
      const left = Math.max(0, Math.floor(x0));
      const right = Math.min(width - 1, Math.ceil(x1));
      const top = Math.max(0, Math.floor(y0));
      const bottom = Math.min(height - 1, Math.ceil(y1));
      for (let y = top; y <= bottom; y += 1) {
        for (let x = left; x <= right; x += 1) values.push(lum[y * width + x]);
      }
      return values;
    };
    const seam = document.querySelector('.seam');
    const seamBox = seam.getBoundingClientRect();
    const seamRight = Math.min(width - 1, Math.round(seamBox.right) - 1);
    const entries = [];
    for (const element of document.querySelectorAll('.seam__word, .seam__label, .seam__count')) {
      const box = element.getBoundingClientRect();
      if (box.width < 3 || box.height < 3) continue;
      const inside = collect(box.left + 1, box.top + 1, box.right - 1, box.bottom - 1);
      if (!inside.length) continue;
      inside.sort((a, b) => a - b);
      const head = inside.slice(Math.max(0, Math.floor(inside.length * 0.9)));
      const tail = inside.slice(0, Math.max(1, Math.ceil(inside.length * 0.1)));
      const fg = median(head);
      const bgInside = median(tail);
      const x0 = Math.max(0, Math.round(box.left) - 2);
      const x1 = Math.min(seamRight, Math.round(box.right) + 2);
      const bands = [];
      // 上下邻带：贴近文字盒，适合 33px 见方的小块。
      if (box.top - 20 > 0) bands.push(median(collect(x0, box.top - 18, x1, box.top - 6)));
      if (box.bottom + 20 < height) bands.push(median(collect(x0, box.bottom + 6, x1, box.bottom + 18)));
      // 左右邻带：竖排文字可以有一百多像素高，只取上下两端会漏掉中段压上来的亮图，
      // 所以按 16px 切片取同一高度的纸面，再取 90 分位当局部最亮。
      const slices = [];
      for (let top = box.top; top < box.bottom; top += 16) {
        const bottom = Math.min(box.bottom, top + 16);
        const leftStrip = collect(2, top, Math.max(2, Math.round(box.left) - 6), bottom);
        const rightStrip = collect(Math.min(seamRight, Math.round(box.right) + 6), top, seamRight, bottom);
        if (leftStrip.length) slices.push(median(leftStrip));
        if (rightStrip.length) slices.push(median(rightStrip));
      }
      if (slices.length) bands.push(quantile(slices, 0.9));
      const bg = bands.length ? Math.max.apply(null, bands) : bgInside;
      const contrast = (fg + 0.05) / (bg + 0.05);
      const isPrimary = !element.classList.contains('seam__count') && !!element.closest('.seam__tab.is-on');
      entries.push({
        kind: element.classList.contains('seam__count') ? 'meta' : isPrimary ? 'primary' : 'secondary',
        cls: element.className,
        text: (element.textContent || '').trim().slice(0, 24),
        color: getComputedStyle(element).color,
        rect: { x: Math.round(box.x), y: Math.round(box.y), w: Math.round(box.width), h: Math.round(box.height) },
        fg: Number(fg.toFixed(4)),
        bg: Number(bg.toFixed(4)),
        bgInside: Number(bgInside.toFixed(4)),
        contrast: Number(contrast.toFixed(2)),
        contrastInside: Number(((fg + 0.05) / (bgInside + 0.05)).toFixed(2)),
      });
    }
    return {
      image: { width, height },
      seamWidth: Math.round(seamBox.width),
      backgroundFilter: getComputedStyle(seam).backdropFilter,
      entries,
    };
  };

  return { resetFrameProbe, stopFrameProbe, tileCount, loadedCount, session, brightTiles, tileRect, analyzeSeam, seamBound };
})();
true
`;

function buildDragPath(seconds) {
  const steps = [];
  const cycle = 2250;
  const hold = 2100;
  const total = seconds * 1000;
  for (let t = 0; t < total; t += 16) {
    const tc = t % cycle;
    const pressed = tc < hold;
    const progress = pressed ? tc / hold : 0;
    // 手不用移出视口：从右侧按下，缓慢拖到左侧，抬手 150ms 后回到右侧重按。
    const x = pressed ? 1320 - 1090 * progress : 1320;
    const y = 450 + 42 * Math.sin((t / 1000) * 1.9);
    steps.push({ t, x: Math.round(x), y: Math.round(y), pressed });
  }
  return steps;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const chrome = findChrome(args.chrome);
  if (!chrome) {
    console.error('找不到 Chrome/Edge，用 --chrome 指定可执行文件');
    process.exit(2);
  }

  const outDir = resolve(args.out);
  mkdirSync(outDir, { recursive: true });
  const profile = args.keep ? join(outDir, '.profile-perf') : mkdtempSync(join(tmpdir(), 'kanalib-perf-'));
  const port = 9200 + Math.floor(Math.random() * 500);

  const child = spawn(
    chrome,
    [
      '--headless=new',
      `--remote-debugging-port=${port}`,
      `--user-data-dir=${profile}`,
      '--no-first-run',
      '--no-default-browser-check',
      '--disable-extensions',
      '--hide-scrollbars',
      // 这三个参数把「无显示器 → 后台页节流」摘掉，否则 rAF / 定时器会被按 1fps 限流，数据没有意义。
      '--disable-background-timer-throttling',
      '--disable-renderer-backgrounding',
      '--disable-backgrounding-occluded-windows',
      '--force-device-scale-factor=1',
      `--window-size=${VIEWPORT.width},${VIEWPORT.height}`,
      'about:blank',
    ],
    { stdio: 'ignore' },
  );

  const report = {
    generatedAt: new Date().toISOString(),
    base: args.base,
    viewport: VIEWPORT,
    seconds: args.seconds,
    environment: { chrome, platform: process.platform, node: process.version },
    longDrag: null,
    seamContrast: null,
    blurComparison: null,
    notes: [
      '帧间隔由页面内 requestAnimationFrame 采集：headless=new 没有 vsync，实测基准 ≈143Hz（p50 ≈7ms），所以 p95 ≤33.3ms 是「约两个 60Hz 帧周期」的卡顿红线，不代表显示器刷新率。',
      'backdrop-filter 由合成器执行，rAF 只能反映主线程；headless 走软件渲染（SwiftShader），GPU 侧成本不能代表真机，C 段的差值只能读作「主线程几乎不受影响」。',
      '堆增长用首末各 3 次 GC 后的最小值（保留堆）比较；500ms 原始采样峰值含未回收垃圾（本机可达 38MB），不作为泄漏判据。',
      '侧栏对比度取「文字矩形内亮度前 10% 中位数」作文本笔画、「矩形外 4–18px 的纸面中位数」作背景；笔画受 11px 抗锯齿影响，实测值低于名义色。',
    ],
    pageErrors: [],
    failures: [],
  };
  let cdp = null;

  try {
    await waitForDevtools(port);
    const target = await newTarget(port, 'about:blank');
    cdp = new Cdp(target.webSocketDebuggerUrl);
    await cdp.connect();
    cdp.on((message) => {
      // 只记录，不当断言：并行开发时 HMR 重载会带出瞬时异常，测量有效性由 session 判断。
      if (message.method === 'Runtime.exceptionThrown') {
        const details = message.params.exceptionDetails;
        report.pageErrors.push('exception: ' + (details?.text ?? '') + ' ' + (details?.exception?.description ?? '').slice(0, 300));
      } else if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
        report.pageErrors.push(message.params.args.map((argument) => argument.value ?? argument.description ?? '').join(' ').slice(0, 300));
      }
    });
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    await cdp.send('Performance.enable');
    await cdp.send('HeapProfiler.enable');
    await cdp.send('Emulation.setDeviceMetricsOverride', {
      width: VIEWPORT.width,
      height: VIEWPORT.height,
      deviceScaleFactor: 1,
      mobile: false,
    });

    const evaluate = async (expression) => {
      const result = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
      if (result.exceptionDetails) {
        throw new Error(result.exceptionDetails.text + ' ' + (result.exceptionDetails.exception?.description ?? ''));
      }
      return result.result.value;
    };

    const installHelpers = () => evaluate(PAGE_HELPERS);

    const readMetrics = async () => {
      const { metrics } = await cdp.send('Performance.getMetrics');
      const map = {};
      for (const item of metrics) map[item.name] = item.value;
      return map;
    };

    /** 前后各做 3 次 GC 取最小值当「保留堆」。
     *  实测单次 collectGarbage 之后的读数仍会摆动 ±30%（10.1 / 9.9 / 12.9 / 8.5MB），
     *  不取稳定值会把噪声算成「增长 73%」。 */
    const stableHeap = async () => {
      const readings = [];
      for (let round = 0; round < 3; round += 1) {
        await cdp.send('HeapProfiler.collectGarbage');
        await sleep(250);
        readings.push((await readMetrics()).JSHeapUsedSize ?? 0);
      }
      readings.sort((a, b) => a - b);
      return Math.round(readings[0]);
    };

    /** 回首页并等到卡片与图片都就绪。
     *  先跳 about:blank 再回来：同名 hash 的 Page.navigate 是同文档导航，不会重建文档，
     *  注入过的 <style> 会活到下一段测量（踩过这个坑）。HMR 也会整页重载，所以给足轮询时间。 */
    const loadHome = async ({ settleMs = 1600 } = {}) => {
      await cdp.send('Page.navigate', { url: 'about:blank' });
      await sleep(120);
      await cdp.send('Page.navigate', { url: args.base + '#/' });
      let tiles = 0;
      for (let attempt = 0; attempt < 100; attempt += 1) {
        try {
          tiles = await evaluate(`document.querySelectorAll('.tile').length`);
        } catch (error) {
          tiles = 0;
        }
        if (tiles > 5) break;
        await sleep(150);
      }
      await sleep(settleMs);
      await installHelpers();
      const state = await evaluate(`({ tiles: __wallPerf.tileCount(), loaded: __wallPerf.loadedCount(), hash: location.hash })`);
      if (state.tiles === 0) throw new Error('首页没有渲染出任何卡片');
      return state;
    };

    /** 把整张卡拖到侧栏底下（指针始终留在视口内，慢速拖动 + 松手后等惯性停）。 */
    const panRect = async (dx, dy) => {
      let remainingX = Math.round(dx);
      let remainingY = Math.round(dy);
      let guard = 0;
      while ((Math.abs(remainingX) > 2 || Math.abs(remainingY) > 2) && guard < 8) {
        guard += 1;
        const stepX = Math.max(-340, Math.min(340, remainingX));
        const stepY = Math.max(-280, Math.min(280, remainingY));
        const startX = stepX >= 0 ? 360 : 1100;
        const startY = stepY >= 0 ? 240 : 680;
        await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: startX, y: startY, button: 'left', buttons: 1, clickCount: 1 });
        for (let step = 1; step <= 4; step += 1) {
          await cdp.send('Input.dispatchMouseEvent', {
            type: 'mouseMoved',
            x: Math.round(startX + (stepX * step) / 4),
            y: Math.round(startY + (stepY * step) / 4),
            button: 'left',
            buttons: 1,
          });
          await sleep(40);
        }
        // 抬手前先在同一位置补一次移动，把指针速度归零 —— 否则松手会触发惯性，
        // 相机多滑出几百像素，定位就得反复纠偏。
        await sleep(120);
        await cdp.send('Input.dispatchMouseEvent', {
          type: 'mouseMoved',
          x: Math.round(startX + stepX),
          y: Math.round(startY + stepY),
          button: 'left',
          buttons: 1,
        });
        await sleep(40);
        await cdp.send('Input.dispatchMouseEvent', {
          type: 'mouseReleased',
          x: Math.round(startX + stepX),
          y: Math.round(startY + stepY),
          button: 'left',
          buttons: 0,
          clickCount: 1,
        });
        remainingX -= stepX;
        remainingY -= stepY;
        await sleep(350);
      }
    };

    const panTileUnderSeam = async (id, targetY = VIEWPORT.height / 2) => {
      for (let round = 0; round < 4; round += 1) {
        const rect = await evaluate(`__wallPerf.tileRect(${JSON.stringify(id)})`);
        if (!rect) return false;
        const dx = SEAM_STRIP / 2 - (rect.left + rect.width / 2);
        const dy = targetY - (rect.top + rect.height / 2);
        if (Math.abs(dx) < 30 && Math.abs(dy) < 60) return true;
        await panRect(dx, dy);
      }
      return true;
    };

    const captureScreenshot = async () => {
      const { data } = await cdp.send('Page.captureScreenshot', { format: 'png', fromSurface: true });
      return data;
    };

    /** 一段拖拽：按预生成的时间序列发鼠标事件，同时每 500ms 采样堆与 DOM。
     *  堆的「增长」用前后各一次强制 GC 之后的保留堆比较——否则中间一次大回收就能把
     *  增长率算成负数（冒烟测试踩到过 -71%）。原始序列仍原样保留，供看峰值。 */
    const measureDrag = async (label) => {
      const session = await evaluate(`__wallPerf.session()`);
      const baseline = await stableHeap();
      await evaluate(`__wallPerf.resetFrameProbe()`);
      const startedAt = Date.now();
      const startMetrics = await readMetrics();
      const samples = { heap: [], tiles: [] };
      const path = buildDragPath(args.seconds);

      let dragDone = false;
      let dragError = null;
      const dragPromise = (async () => {
        const t0 = Date.now();
        let holding = false;
        for (let index = 0; index < path.length; index += 1) {
          const step = path[index];
          const wait = step.t - (Date.now() - t0);
          if (wait > 0) await sleep(wait);
          // 抬手后的 150ms 空档不发事件，让 rAF 至少跑回静止节奏，接下一轮按下的位置才一致。
          if (!step.pressed) continue;
          if (!holding) {
            await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: step.x, y: step.y, button: 'left', buttons: 1, clickCount: 1 });
            holding = true;
          }
          await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: step.x, y: step.y, button: 'left', buttons: 1 });
          const next = path[index + 1];
          if (!next || !next.pressed) {
            await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: step.x, y: step.y, button: 'left', buttons: 0, clickCount: 1 });
            holding = false;
          }
        }
      })()
        .catch((error) => {
          dragError = error;
        })
        .finally(() => {
          dragDone = true;
        });

      while (!dragDone) {
        await sleep(500);
        try {
          const metrics = await readMetrics();
          samples.heap.push(Math.round(metrics.JSHeapUsedSize ?? 0));
          samples.tiles.push(await evaluate(`__wallPerf.tileCount()`));
        } catch (error) {
          report.pageErrors.push(label + '：采样中断（' + (error.message || error) + '）');
          break;
        }
      }
      await dragPromise;
      if (dragError) throw dragError;

      const elapsed = (Date.now() - startedAt) / 1000;
      // 页面若在测量中途整页重载，注入的 __wallPerf 会消失，这里必须容错并宣告作废。
      let probe = null;
      let sameSession = false;
      try {
        probe = await evaluate(`__wallPerf.stopFrameProbe()`);
        sameSession = (await evaluate(`__wallPerf.session()`)) === session;
      } catch (error) {
        probe = null;
      }
      if (!probe) {
        return { label, valid: false, invalidReason: '页面在测量期间重载，探针失效', elapsedSeconds: Number(elapsed.toFixed(1)), sessionStable: false };
      }
      const endMetrics = await readMetrics();
      const retained = await stableHeap();

      if (samples.heap.length === 0) {
        return {
          label,
          valid: false,
          invalidReason: '没有采到堆数据（采样期间页面失联）',
          elapsedSeconds: Number(elapsed.toFixed(1)),
          sessionStable: sameSession,
        };
      }
      const heapPeak = Math.max.apply(null, samples.heap);
      const taskSeconds = (endMetrics.TaskDuration ?? 0) - (startMetrics.TaskDuration ?? 0);
      const scriptSeconds = (endMetrics.ScriptDuration ?? 0) - (startMetrics.ScriptDuration ?? 0);
      const tilesMin = Math.min.apply(null, samples.tiles);
      const valid = sameSession && tilesMin > 0 && probe.frames > args.seconds * 20;

      return {
        label,
        valid,
        // 作废原因写清楚，重试用尽后能直接看出是哪一类干扰。
        invalidReason: valid
          ? ''
          : !sameSession
            ? '测量期间页面被重载（HMR / 刷新）'
            : tilesMin === 0
              ? '测量期间画面出现空页（0 张卡）'
              : '采样帧数过少（' + probe.frames + '）',
        elapsedSeconds: Number(elapsed.toFixed(1)),
        heap: {
          method: 'GC×3 取最小值的保留堆（首末各一次）＋ 500ms 原始采样',
          firstMB: Number((baseline / 1048576).toFixed(2)),
          lastMB: Number((retained / 1048576).toFixed(2)),
          peakMB: Number((heapPeak / 1048576).toFixed(2)),
          growthPct: Number((((retained - baseline) / baseline) * 100).toFixed(1)),
          growthPeakPct: Number((((heapPeak - baseline) / baseline) * 100).toFixed(1)),
          seriesMB: samples.heap.map((value) => Number((value / 1048576).toFixed(1))),
        },
        tiles: {
          min: tilesMin,
          max: Math.max.apply(null, samples.tiles),
          series: samples.tiles,
        },
        frames: probe,
        mainThread: {
          taskRatio: Number((taskSeconds / elapsed).toFixed(3)),
          scriptRatio: Number((scriptSeconds / elapsed).toFixed(3)),
        },
        sessionStable: sameSession,
        domNodes: Math.round(endMetrics.Nodes ?? 0),
      };
    };

    /** 一段测量带重试：并行开发时 HMR 会整页重载（App.tsx 改动会触发），数据随即作废。
     *  作废判据：window 上的 session 令牌变了 / 采样里出现 0 张卡 / 帧数少得不像话。 */
    const runPhase = async (label, prepare, settleMs = 1600) => {
      for (let attempt = 1; attempt <= 3; attempt += 1) {
        await loadHome({ settleMs });
        if (prepare) {
          const prepared = await prepare();
          if (!prepared) {
            report.pageErrors.push(label + '：第 ' + attempt + ' 次准备失败（注入未生效）');
            continue;
          }
        }
        const result = await measureDrag(label + '（第 ' + attempt + ' 次）');
        result.attempts = attempt;
        if (result.valid) return result;
        report.pageErrors.push(label + '：' + result.invalidReason + '，作废重试');
      }
      throw new Error(label + '：三次尝试都作废（页面持续被重载？）');
    };

    // ===== A. 长时拖拽（blur 按发布形态开着）=====
    report.longDrag = await runPhase('A 长时拖拽', null);

    // ===== C. 同一段拖拽，关掉 backdrop-filter =====
    const blurOff = await runPhase('C 关闭模糊', async () => {
      await evaluate(`(() => {
        const style = document.createElement('style');
        style.id = 'perf-no-blur';
        style.textContent = '.seam{backdrop-filter:none !important;-webkit-backdrop-filter:none !important;}';
        document.head.appendChild(style);
        return true;
      })()`);
      // 注入的 <style> 要等一次样式重算才反映到 getComputedStyle（同一任务内读到的还是旧值）。
      await sleep(250);
      const applied = await evaluate(`getComputedStyle(document.querySelector('.seam')).backdropFilter`);
      report.blurInjection = applied;
      return applied === 'none';
    });
    report.blurComparison = {
      on: report.longDrag.frames,
      off: blurOff.frames,
      onMainThread: report.longDrag.mainThread,
      offMainThread: blurOff.mainThread,
      blurOffApplied: report.blurInjection,
      onAttempts: report.longDrag.attempts,
      offAttempts: blurOff.attempts,
      delta: {
        p95: Number((blurOff.frames.p95 - report.longDrag.frames.p95).toFixed(2)),
        p99: Number((blurOff.frames.p99 - report.longDrag.frames.p99).toFixed(2)),
        droppedPct: Number((blurOff.frames.droppedPct - report.longDrag.frames.droppedPct).toFixed(2)),
        taskRatio: Number((blurOff.mainThread.taskRatio - report.longDrag.mainThread.taskRatio).toFixed(3)),
      },
    };

    // ===== B. 侧栏在真实底图上的对比度 =====
    const homeState = await loadHome({ settleMs: 2200 });
    const samples = [];

    const sampleSeam = async (name, tileLum) => {
      const base64 = await captureScreenshot();
      const analysis = await evaluate(`__wallPerf.analyzeSeam(${JSON.stringify(base64)})`);
      if (analysis.image.width !== VIEWPORT.width || analysis.image.height !== VIEWPORT.height) {
        throw new Error('B 对比度：截图尺寸与视口不一致（' + analysis.image.width + '×' + analysis.image.height + '）');
      }
      return { name, tileLum, base64, analysis };
    };

    samples.push(await sampleSeam('home', 0));
    // 两轮：每轮重新挑「当前画面里最亮、且还不在缝下面」的那张高卡，
    // 分别把它挪到缝的上/中/下三段，让顶部 Kanalib、中部页签、底部计数都轮到亮图垫底。
    for (let round = 1; round <= 2; round += 1) {
      const bright = await evaluate(`__wallPerf.brightTiles(10)`);
      const target = bright.find(
        (entry) => entry.rect.left + entry.rect.width / 2 > SEAM_STRIP + 40 && entry.rect.height > 160,
      );
      if (!target) break;
      for (const targetY of [120, 450, 780]) {
        const moved = await panTileUnderSeam(target.id, targetY);
        if (!moved) break;
        await sleep(450);
        samples.push(await sampleSeam('bright-' + round + '-y' + targetY, target.p75));
      }
    }

    const relevantWorstOf = (sample) => {
      let best = null;
      for (const entry of sample.analysis.entries) {
        if (entry.kind !== 'primary' && entry.kind !== 'secondary') continue;
        if (!best || entry.contrast < best.contrast) best = entry;
      }
      return best;
    };
    const worstOfKind = (kind) => {
      let best = null;
      for (const sample of samples) {
        for (const entry of sample.analysis.entries) {
          if (entry.kind !== kind) continue;
          if (!best || entry.contrast < best.contrast) best = { ...entry, position: sample.name, tileLum: sample.tileLum };
        }
      }
      return best;
    };

    let worstSample = null;
    for (const sample of samples) {
      const worst = relevantWorstOf(sample);
      if (worst && (!worstSample || worst.contrast < worstSample.worst.contrast)) worstSample = { sample, worst };
    }
    const theoretical = await evaluate(`__wallPerf.seamBound()`);

    if (!worstSample) {
      report.failures.push('B 对比度：没有量到任何侧栏文字');
    } else {
      const shotName = 'perf-seam-worst.png';
      writeFileSync(join(outDir, shotName), Buffer.from(worstSample.sample.base64, 'base64'));
      const entries = worstSample.sample.analysis.entries;
      report.seamContrast = {
        worst: worstSample.worst,
        position: worstSample.sample.name,
        tileLum: worstSample.sample.tileLum,
        brightestBg: Number(Math.max.apply(null, entries.map((entry) => entry.bg)).toFixed(4)),
        screenshot: shotName,
        seamWidth: worstSample.sample.analysis.seamWidth,
        backgroundFilter: worstSample.sample.analysis.backgroundFilter,
        theoreticalWorst: theoretical,
        primaryWorst: worstOfKind('primary'),
        secondaryWorst: worstOfKind('secondary'),
        metaWorst: worstOfKind('meta'),
        positions: samples.map((sample) => {
          const worst = relevantWorstOf(sample);
          return {
            name: sample.name,
            tileLum: sample.tileLum,
            backgroundFilter: sample.analysis.backgroundFilter,
            brightestBg: sample.analysis.entries.length
              ? Number(Math.max.apply(null, sample.analysis.entries.map((entry) => entry.bg)).toFixed(4))
              : null,
            worstContrast: worst ? worst.contrast : null,
            worstText: worst ? worst.text : null,
          };
        }),
      };
    }

    // ===== 断言 =====
    const drag = report.longDrag;
    if (drag.tiles.max > MAX_DOM_TILES) {
      report.failures.push('A 虚拟化没有收敛：DOM 卡片峰值 ' + drag.tiles.max + ' > ' + MAX_DOM_TILES);
    }
    if (drag.heap.growthPct > MAX_HEAP_GROWTH * 100) {
      report.failures.push('A JS 堆 ' + drag.elapsedSeconds + 's 增长 ' + drag.heap.growthPct + '% > 40%');
    }
    if (drag.frames.p95 > FRAME_BUDGET_MS) {
      report.failures.push('A 帧 p95 ' + drag.frames.p95 + 'ms > 33.3ms');
    }
    if (report.blurComparison && report.blurComparison.blurOffApplied !== 'none') {
      report.failures.push('C 关闭模糊没生效（backdrop-filter=' + report.blurComparison.blurOffApplied + '）');
    }
    if (report.seamContrast) {
      const primary = report.seamContrast.primaryWorst;
      const secondary = report.seamContrast.secondaryWorst;
      if (!secondary) report.failures.push('B 对比度：没找到次级文字样本');
      else if (secondary.contrast < 4.5) report.failures.push('B 次级文字对比度 ' + secondary.contrast + ':1 < 4.5:1（' + secondary.text + '）');
      if (!primary) report.failures.push('B 对比度：没找到主文字样本');
      else if (primary.contrast < 7) report.failures.push('B 主文字对比度 ' + primary.contrast + ':1 < 7:1（' + primary.text + '）');
    }
    report.home = homeState;

    writeFileSync(join(outDir, 'perf-report.json'), JSON.stringify(report, null, 2) + '\n', 'utf8');

    // ===== 终端摘要 =====
    const pct = (value) => (value >= 0 ? '+' : '') + value + '%';
    console.log('== A 长时拖拽（' + drag.elapsedSeconds + 's，blur 开，GC 后保留堆）==');
    console.log(
      'JS 堆：' + drag.heap.firstMB + 'MB → ' + drag.heap.lastMB + 'MB（' + pct(drag.heap.growthPct) + '）；期间原始采样峰值 ' + drag.heap.peakMB + 'MB（含未回收垃圾，不计入判据）',
    );
    console.log('DOM 卡片：' + drag.tiles.min + '–' + drag.tiles.max + '（上限 ' + MAX_DOM_TILES + '）');
    console.log(
      '帧：p50 ' + drag.frames.p50 + 'ms / p95 ' + drag.frames.p95 + 'ms / p99 ' + drag.frames.p99 + 'ms，掉帧 ' + drag.frames.droppedPct + '%，采样 ' + drag.frames.frames + ' 帧，基准 ≈' + drag.frames.baselineHz + 'Hz',
    );
    console.log(
      '长任务：' + drag.frames.longTasks.count + ' 条 / 合计 ' + drag.frames.longTasks.totalMs + 'ms（最长 ' + drag.frames.longTasks.maxMs + 'ms）；主线程占用 ' + Math.round(drag.mainThread.taskRatio * 100) + '%',
    );
    if (drag.attempts > 1) console.log('（A 因页面重载重试 ' + drag.attempts + ' 次后取到有效数据）');
    if (report.blurComparison) {
      const delta = report.blurComparison.delta;
      console.log('== C backdrop-filter 开关对比 ==');
      console.log(
        '开：p95 ' + report.blurComparison.on.p95 + 'ms / p99 ' + report.blurComparison.on.p99 + 'ms / 掉帧 ' + report.blurComparison.on.droppedPct + '% / 占用 ' + Math.round(report.blurComparison.onMainThread.taskRatio * 100) + '%',
      );
      console.log(
        '关：p95 ' + report.blurComparison.off.p95 + 'ms / p99 ' + report.blurComparison.off.p99 + 'ms / 掉帧 ' + report.blurComparison.off.droppedPct + '% / 占用 ' + Math.round(report.blurComparison.offMainThread.taskRatio * 100) + '%',
      );
      console.log('差（关 − 开）：p95 ' + delta.p95 + 'ms，p99 ' + delta.p99 + 'ms，掉帧 ' + delta.droppedPct + ' 个百分点，主线程占用 ' + delta.taskRatio);
    }
    if (report.seamContrast) {
      const seam = report.seamContrast;
      const kindName = { primary: '主文字', secondary: '次级文字', meta: '计数文字' };
      console.log('== B 侧栏实际像素对比度 ==');
      console.log('最差样本：' + seam.position + '（底图亮度 ' + seam.tileLum + '），缝内最亮纸面 L=' + seam.brightestBg + '，截图 ' + seam.screenshot);
      console.log(
        '最差文字：' + seam.worst.text + '（' + (kindName[seam.worst.kind] ?? seam.worst.kind) + '）' + seam.worst.contrast + ':1 @ ' + seam.worst.rect.x + ',' + seam.worst.rect.y + '，fg L=' + seam.worst.fg + ' / bg L=' + seam.worst.bg,
      );
      if (seam.primaryWorst) console.log('主文字最差：' + seam.primaryWorst.contrast + ':1（' + seam.primaryWorst.text + '，' + seam.primaryWorst.position + '）');
      if (seam.secondaryWorst) console.log('次级文字最差：' + seam.secondaryWorst.contrast + ':1（' + seam.secondaryWorst.text + '，' + seam.secondaryWorst.position + '）');
      if (seam.metaWorst) console.log('计数文字最差：' + seam.metaWorst.contrast + ':1（38% 白，v3 未列口径，仅供参考）');
      if (seam.theoreticalWorst) {
        console.log(
          '理论下限（纯白图垫底、按名义色算，不含抗锯齿）：' + seam.theoreticalWorst.contrast + ':1（纸面 α=' + seam.theoreticalWorst.paperAlpha + '，文字 α=' + seam.theoreticalWorst.inkAlpha + '）',
        );
      }
    }
    if (report.pageErrors.length > 0) console.log('页面侧异常/作废记录：' + report.pageErrors.length + ' 条（见 perf-report.json 的 pageErrors）');
    console.log('== 判定 ==');
    if (report.failures.length === 0) console.log('全部达标');
    else for (const failure of report.failures) console.log('不达标：' + failure);
    console.log('报告：' + join(outDir, 'perf-report.json'));
    process.exitCode = report.failures.length > 0 ? 1 : 0;
  } catch (error) {
    console.error(String(error && error.message ? error.message : error));
    process.exitCode = 1;
  } finally {
    cdp?.close();
    child.kill();
    if (!args.keep) {
      await sleep(300);
      hardDelete(profile);
    }
  }
}

void main();
