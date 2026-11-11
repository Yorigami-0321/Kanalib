#!/usr/bin/env node
/**
 * 滚轮/触控板平移的帧率 A/B 探针（2026-11-11）。
 * 同一页面、同一段 2.2s 合成滚轮，交替测量：
 *   - after：发布态（.tile__img 只保留 200ms 线性淡入）；
 *   - before：运行时注入旧规则（14px blur + 1.035 scale + 700–900ms 过渡）。
 * 输出 fps / p50 / p95 / max / >32ms 长帧数，取 3 轮中位数。
 * 用法：node scripts/wall_wheel_perf.mjs http://127.0.0.1:4318/ [outDir]
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

const OLD_RULES = `
.tile__img {
  filter: blur(14px) saturate(0.85) !important;
  transform: scale(1.035) !important;
  transition: opacity 420ms var(--ease), transform 900ms var(--ease), filter 700ms var(--ease) !important;
}
.tile__img.is-loaded {
  filter: none !important;
  transform: scale(1.001) !important;
}
`;

class Cdp {
  constructor(url) {
    this.url = url;
    this.nextId = 1;
    this.pending = new Map();
    this.listeners = [];
  }

  async connect() {
    this.ws = new WebSocket(this.url);
    await new Promise((ok, no) => {
      this.ws.addEventListener('open', ok, { once: true });
      this.ws.addEventListener('error', () => no(new Error('CDP 连接失败')), { once: true });
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
    return new Promise((done, no) => {
      this.pending.set(id, { resolve: done, reject: no });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  on(listener) {
    this.listeners.push(listener);
  }

  close() {
    try {
      this.ws.close();
    } catch {
      /* closed */
    }
  }
}

const base = (process.argv[2] ?? 'http://127.0.0.1:4318/').replace(/\/?$/, '/');
const outDir = resolve(process.argv[3] ?? 'web/screenshots/ui-review/ui');
mkdirSync(outDir, { recursive: true });
const chrome = CANDIDATES.find((path) => existsSync(path));
if (!chrome) {
  console.error('找不到 Chrome/Edge');
  process.exit(1);
}
const profile = mkdtempSync(join(tmpdir(), 'kanalib-wheel-perf-'));
const port = 9700 + Math.floor(Math.random() * 90);

const child = spawn(
  chrome,
  [
    '--headless=new',
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${profile}`,
    '--no-first-run',
    '--no-default-browser-check',
    '--hide-scrollbars',
    '--disable-background-timer-throttling',
    '--disable-renderer-backgrounding',
    '--disable-backgrounding-occluded-windows',
    '--force-device-scale-factor=1',
    '--window-size=1440,900',
    'about:blank',
  ],
  { stdio: 'ignore' },
);

const SAMPLER = `
window.__wheelPerf = (() => {
  let deltas = [];
  let running = false;
  let last = 0;
  const tick = (now) => {
    if (!running) return;
    if (last) deltas.push(now - last);
    last = now;
    requestAnimationFrame(tick);
  };
  return {
    start() {
      deltas = [];
      running = true;
      last = performance.now();
      requestAnimationFrame(tick);
    },
    stop() {
      running = false;
      const values = deltas.slice(2).sort((a, b) => a - b);
      const q = (p) => (values.length ? values[Math.min(values.length - 1, Math.round((values.length - 1) * p))] : 0);
      const total = deltas.reduce((sum, value) => sum + value, 0);
      return {
        frames: values.length,
        elapsed: Math.round(total),
        fps: total ? Number((values.length / (total / 1000)).toFixed(1)) : 0,
        p50: Number(q(0.5).toFixed(2)),
        p95: Number(q(0.95).toFixed(2)),
        max: Number(q(1).toFixed(2)),
        long32: values.filter((value) => value > 32).length,
      };
    },
  };
})();
true
`;

const report = { measurements: [], runs: {}, consoleErrors: [], failures: [] };
let cdp;

const median = (values) => {
  const sorted = values.slice().sort((a, b) => a - b);
  return sorted[Math.floor((sorted.length - 1) / 2)];
};

try {
  for (let i = 0; i < 100; i += 1) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (res.ok) break;
    } catch {
      /* wait */
    }
    await sleep(120);
  }
  const target = await (await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, { method: 'PUT' })).json();
  cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.connect();
  cdp.on((message) => {
    if (message.method === 'Runtime.exceptionThrown') {
      report.consoleErrors.push('exception: ' + (message.params.exceptionDetails?.text ?? 'unknown'));
    }
    if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
      report.consoleErrors.push(message.params.args.map((arg) => arg.value ?? arg.description ?? '').join(' '));
    }
  });
  await cdp.send('Page.enable');
  await cdp.send('Runtime.enable');
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });

  const evaluate = async (expression) => {
    const result = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) {
      throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    }
    return result.result.value;
  };

  const load = async () => {
    await cdp.send('Page.navigate', { url: 'about:blank' });
    await sleep(120);
    await cdp.send('Page.navigate', { url: base + '#/' });
    for (let attempt = 0; attempt < 120; attempt += 1) {
      const state = await evaluate(
        `(() => {
          const tiles = [...document.querySelectorAll('.tile')];
          return {
            tiles: tiles.length,
            pending: [...document.querySelectorAll('img.tile__img')].filter((img) => !img.complete || !img.classList.contains('is-loaded')).length,
          };
        })()`,
      ).catch(() => null);
      if (state && state.tiles > 5 && state.pending === 0) break;
      await sleep(150);
    }
    await evaluate(SAMPLER);
    await sleep(450);
  };

  const runOnce = async (variant) => {
    await load();
    if (variant === 'before') {
      await evaluate(`(() => {
        const style = document.createElement('style');
        style.id = 'perf-old-tile-img';
        style.textContent = ${JSON.stringify(OLD_RULES)};
        document.head.appendChild(style);
        return true;
      })()`);
      await sleep(300);
    }
    const applied = await evaluate(`(() => {
      const img = document.querySelector('img.tile__img');
      if (!img) return null;
      const style = getComputedStyle(img);
      return { filter: style.filter, transform: style.transform, transition: style.transition };
    })()`);
    await evaluate(`__wheelPerf.start()`);
    const started = Date.now();
    // 2.2s 连续滚轮：16ms 一条，deltaY=100，指针停在画布中部。
    while (Date.now() - started < 2200) {
      await cdp.send('Input.dispatchMouseEvent', {
        type: 'mouseWheel',
        x: 900,
        y: 460,
        deltaX: 0,
        deltaY: 100,
        button: 'none',
        buttons: 0,
      });
      await sleep(16);
    }
    await sleep(120);
    const stats = await evaluate(`__wheelPerf.stop()`);
    const tiles = await evaluate(`document.querySelectorAll('.tile').length`);
    const record = { variant, applied, tiles, ...stats };
    report.measurements.push(record);
    console.log(`${variant}: fps=${stats.fps} p95=${stats.p95}ms max=${stats.max}ms long32=${stats.long32} frames=${stats.frames} tiles=${tiles}`);
    return record;
  };

  // 交替跑，抵消浏览器缓存与机器噪声：after / before × 3。
  for (let round = 1; round <= 3; round += 1) {
    report.runs['after-' + round] = await runOnce('after');
    report.runs['before-' + round] = await runOnce('before');
  }

  const summary = {
    after: {
      fps: median(Object.values(report.runs).filter((run) => run.variant === 'after').map((run) => run.fps)),
      p95: median(Object.values(report.runs).filter((run) => run.variant === 'after').map((run) => run.p95)),
      long32: median(Object.values(report.runs).filter((run) => run.variant === 'after').map((run) => run.long32)),
    },
    before: {
      fps: median(Object.values(report.runs).filter((run) => run.variant === 'before').map((run) => run.fps)),
      p95: median(Object.values(report.runs).filter((run) => run.variant === 'before').map((run) => run.p95)),
      long32: median(Object.values(report.runs).filter((run) => run.variant === 'before').map((run) => run.long32)),
    },
  };
  report.summary = summary;
  report.delta = {
    fps: Number((summary.after.fps - summary.before.fps).toFixed(1)),
    p95: Number((summary.after.p95 - summary.before.p95).toFixed(2)),
    long32: summary.after.long32 - summary.before.long32,
  };
  if (!/0\.42s|420ms/.test(String(report.runs['before-1'].applied?.transition ?? ''))) {
    report.failures.push('before 注入未生效（transition 未回到旧值）：' + JSON.stringify(report.runs['before-1'].applied));
  }
  if (report.consoleErrors.length > 0) report.failures.push('console 有报错：' + report.consoleErrors.join(' | '));
  if (summary.after.fps <= summary.before.fps) report.failures.push('收敛后 fps 未提升：' + summary.after.fps + ' vs ' + summary.before.fps);

  writeFileSync(join(outDir, 'wheel-perf.json'), JSON.stringify(report, null, 2) + '\n', 'utf8');
  console.log(JSON.stringify({ summary, delta: report.delta, failures: report.failures }, null, 2));
  if (report.failures.length > 0) process.exitCode = 1;
} finally {
  cdp?.close();
  try {
    child.kill();
  } catch {
    /* ignore */
  }
  if (process.platform === 'win32') spawnSync('cmd', ['/c', 'rd', '/s', '/q', profile], { stdio: 'ignore' });
  else rmSync(profile, { recursive: true, force: true });
}
