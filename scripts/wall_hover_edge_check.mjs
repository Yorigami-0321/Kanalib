#!/usr/bin/env node
/**
 * 卡面覆盖层底边探针（2026-11-11）。
 * 验证 `.tile__shade` 在 hover 下是否把图片底边完全盖住：
 *   - 1440 / 390 各取一张亮图、一张暗图（按图片平均亮度挑选）；
 *   - 鼠标真实悬停，等 hover 的 transform/filter 过渡结束；
 *   - 截取卡面底部条带 + 卡面中段条带，逐行统计亮度；
 *   - 同时在页面里临时换回旧渐变做 A/B，量化“改前底边透出”。
 * 用法：node scripts/wall_hover_edge_check.mjs http://127.0.0.1:4318/ [outDir]
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

const OLD_GRADIENT =
  'linear-gradient(180deg, rgba(0, 0, 0, 0.05) 34%, rgba(0, 0, 0, 0.31) 56%, rgba(0, 0, 0, 0.62) 78%, rgba(0, 0, 0, 0.93) 100%)';

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
const profile = mkdtempSync(join(tmpdir(), 'kanalib-hover-edge-'));
const port = 9800 + Math.floor(Math.random() * 90);

const child = spawn(
  chrome,
  [
    '--headless=new',
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${profile}`,
    '--no-first-run',
    '--no-default-browser-check',
    '--hide-scrollbars',
    '--window-size=1440,900',
    'about:blank',
  ],
  { stdio: 'ignore' },
);

const report = { shots: {}, consoleErrors: [], checks: [], failures: [], cases: {} };
let cdp;

const check = (name, ok, detail) => {
  report.checks.push({ name, ok, detail });
  if (!ok) report.failures.push(name + (detail ? '：' + detail : ''));
};

const PICK_TILES = `(() => {
  const canvas = document.createElement('canvas');
  canvas.width = 6;
  canvas.height = 6;
  const ctx = canvas.getContext('2d', { willReadFrequently: true });
  const rows = [];
  const skipped = [];
  for (const tile of document.querySelectorAll('.tile')) {
    const box = tile.getBoundingClientRect();
    const reason =
      box.left < 8 || box.right > innerWidth - 8
        ? 'x'
        : box.top < 70 || box.bottom > innerHeight - 8
          ? 'y'
          : box.width < 90 || box.height < 70
            ? 'size'
            : null;
    // 卡面中心必须真正命中该卡（排除被侧栏/提示条盖住、鼠标 hover 不到的卡）。
    const hit = reason ? null : document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2);
    const covered = reason ? null : !hit || !tile.contains(hit);
    if (reason || covered) {
      skipped.push({ id: tile.dataset.id ?? null, rect: [box.left, box.top, box.width, box.height].map((v) => +v.toFixed(1)), reason: reason ?? 'covered' });
      continue;
    }
    const img = tile.querySelector('img.tile__img');
    if (!img || !img.complete) continue;
    let mean = null;
    try {
      ctx.drawImage(img, 0, 0, 6, 6);
      const data = ctx.getImageData(0, 0, 6, 6).data;
      let sum = 0;
      for (let i = 0; i < data.length; i += 4) {
        sum += 0.2126 * data[i] + 0.7152 * data[i + 1] + 0.0722 * data[i + 2];
      }
      mean = +(sum / 36).toFixed(1);
    } catch {
      mean = null;
    }
    rows.push({
      id: tile.dataset.id ?? null,
      rect: [box.left, box.top, box.width, box.height].map((value) => +value.toFixed(2)),
      mean,
    });
  }
  const scored = rows.filter((row) => row.mean !== null).sort((a, b) => b.mean - a.mean);
  return {
    total: rows.length,
    bright: scored[0] ?? null,
    dark: scored[scored.length - 1] ?? null,
    tainted: rows.every((row) => row.mean === null),
    skipped: skipped.slice(0, 24),
  };
})()`;

const sampleImage = (base64, inset = 0.1) => `(async () => {
  const img = new Image();
  img.src = ${JSON.stringify('data:image/png;base64,' + base64)};
  await img.decode();
  const canvas = new OffscreenCanvas(img.width, img.height);
  const ctx = canvas.getContext('2d');
  ctx.drawImage(img, 0, 0);
  const { data } = ctx.getImageData(0, 0, img.width, img.height);
  const w = img.width;
  const h = img.height;
  const rows = [];
  for (let y = 0; y < h; y += 1) {
    let max = 0;
    let sum = 0;
    let n = 0;
    for (let x = Math.floor(w * ${inset}); x < Math.ceil(w * (1 - ${inset})); x += 1) {
      const i = (y * w + x) * 4;
      const value = 0.2126 * data[i] + 0.7152 * data[i + 1] + 0.0722 * data[i + 2];
      if (value > max) max = value;
      sum += value;
      n += 1;
    }
    rows.push({ max: +max.toFixed(1), mean: +(sum / n).toFixed(1) });
  }
  return { width: w, height: h, rows };
})()`;

try {
  for (let i = 0; i < 80; i += 1) {
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

  const evaluate = async (expression) => {
    const result = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) {
      throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    }
    return result.result.value;
  };
  const settle = async (limit = 15000) => {
    const started = Date.now();
    while (Date.now() - started < limit) {
      const state = await evaluate(
        `(() => ({ tiles: document.querySelectorAll('.tile').length, pending: [...document.querySelectorAll('.tile img')].filter((img) => !img.complete || !img.classList.contains('is-loaded')).length }))()`,
      ).catch(() => null);
      if (state && state.tiles > 0 && state.pending === 0) return state;
      await sleep(200);
    }
    return { timeout: true };
  };
  const shotClip = async (name, clip) => {
    const { data } = await cdp.send('Page.captureScreenshot', { format: 'png', fromSurface: true, clip });
    writeFileSync(join(outDir, name + '.png'), Buffer.from(data, 'base64'));
    report.shots[name] = join(outDir, name + '.png');
    return data;
  };
  const hover = async (x, y) => {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y, button: 'none' });
  };
  // 镜头在图片就绪后还有一段缓动：等 .world 的 transform 连续两次采样不变再挑卡。
  const waitStable = async () => {
    let prev = null;
    for (let i = 0; i < 20; i += 1) {
      const cur = await evaluate(
        `(() => { const world = document.querySelector('.world'); return world ? getComputedStyle(world).transform : null; })()`,
      );
      if (cur && cur === prev) return cur;
      prev = cur;
      await sleep(300);
    }
    return prev;
  };

  const runCase = async (label, tile, width) => {
    if (!tile) {
      check(label + ' 找到候选卡面', false, 'no tile');
      return null;
    }
    const stateExpr = `(() => {
      const tile = document.querySelector('.tile[data-id="' + ${JSON.stringify(tile.id)} + '"]');
      if (!tile) return null;
      const box = tile.getBoundingClientRect();
      const shade = tile.querySelector('.tile__shade');
      return {
        hovered: tile.matches(':hover'),
        rect: [box.left, box.top, box.width, box.height].map((value) => +value.toFixed(3)),
        shade: shade ? getComputedStyle(shade).backgroundImage : null,
      };
    })()`;
    let state = null;
    for (let attempt = 0; attempt < 4; attempt += 1) {
      // 每次用卡的实时 rect 悬停：镜头若仍在缓动，旧 rect 会点空。
      const spot = await evaluate(`(() => {
        const tile = document.querySelector('.tile[data-id="' + ${JSON.stringify(tile.id)} + '"]');
        if (!tile) return null;
        const box = tile.getBoundingClientRect();
        const x = box.left + box.width / 2;
        const y = box.top + box.height / 2;
        const hit = document.elementFromPoint(x, y);
        return { x, y, inView: x > 0 && y > 0 && x < innerWidth && y < innerHeight, hit: !!hit && tile.contains(hit) };
      })()`);
      if (!spot) break;
      if (!spot.inView || !spot.hit) {
        await sleep(400);
        continue;
      }
      await hover(spot.x, spot.y);
      await sleep(1200);
      state = await evaluate(stateExpr);
      if (state?.hovered) break;
      await sleep(300);
    }
    if (!state) state = await evaluate(stateExpr);
    if (!state) {
      check(label + ' 悬停卡面存在', false, 'tile missing');
      return null;
    }
    const [x, y, w, h] = state.rect;
    const scale = 3;
    const stripTop = y + h - 40;
    const stripHeight = 46;
    const bottomClip = {
      x: Math.floor(x),
      y: Math.floor(stripTop),
      width: Math.round(w),
      height: Math.max(8, Math.round(stripHeight)),
      scale,
    };
    const midClip = {
      x: Math.floor(x),
      y: Math.floor(y + h * 0.4),
      width: Math.round(w),
      height: Math.max(8, Math.round(h * 0.12)),
      scale,
    };

    // 左右各排除 12px 圆角（+2px 抗锯齿）：那不是底边透出，是圆角与页面底色的过渡。
    const inset = Math.min(0.2, Math.max(0.1, 14 / bottomClip.width));
    const bottomData = await shotClip(`hover-edge-${label}-bottom`, bottomClip);
    const midData = await shotClip(`hover-edge-${label}-mid`, midClip);
    await shotClip(`hover-edge-${label}-full`, { x: Math.floor(x), y: Math.floor(y), width: Math.round(w), height: Math.round(h), scale: 1 });

    const bottom = await evaluate(sampleImage(bottomData, inset));
    const mid = await evaluate(sampleImage(midData, inset));

    // deviceScaleFactor（390 用 2）会计入截图像素，用实际像素/裁剪宽推导有效倍率。
    const effScale = bottom.width / bottomClip.width;
    const insideEnd = Math.round((y + h - bottomClip.y) * effScale) - 1;
    const rows = bottom.rows;
    // 统计紧邻卡面下沿的最后 1 CSS px；再往下可能是行间距外的邻图，不属于本卡覆盖层。
    const lastRows = rows.slice(Math.max(0, insideEnd - Math.round(effScale) + 1), insideEnd + 1);
    const bottomMax = Math.max(...lastRows.map((row) => row.max));
    const prevRows = rows.slice(Math.max(0, insideEnd - Math.round(effScale) * 2 + 1), insideEnd - Math.round(effScale) + 1);
    const prevMax = prevRows.length ? Math.max(...prevRows.map((row) => row.max)) : 0;
    const belowRows = rows.slice(insideEnd + 1, insideEnd + 1 + Math.round(effScale));
    const belowMax = belowRows.length ? Math.max(...belowRows.map((row) => row.max)) : 0;
    const midMax = Math.max(...mid.rows.map((row) => row.max));
    const midMean = +(mid.rows.reduce((sum, row) => sum + row.mean, 0) / mid.rows.length).toFixed(1);

    // A/B：页面内临时换回旧渐变，量化改前底边透出（不改磁盘文件）。
    const oldData = await evaluate(`(async () => {
      const tile = document.querySelector('.tile[data-id="' + ${JSON.stringify(tile.id)} + '"]');
      const shade = tile?.querySelector('.tile__shade');
      if (!shade) return null;
      shade.style.background = ${JSON.stringify(OLD_GRADIENT)};
      await new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)));
      return getComputedStyle(shade).backgroundImage;
    })()`);
    const oldBottomData = await shotClip(`hover-edge-${label}-before`, bottomClip);
    await evaluate(`(() => {
      const tile = document.querySelector('.tile[data-id="' + ${JSON.stringify(tile.id)} + '"]');
      const shade = tile?.querySelector('.tile__shade');
      if (shade) shade.style.background = '';
    })()`);
    const oldBottom = oldBottomData ? await evaluate(sampleImage(oldBottomData, inset)) : null;
    const oldLastRows = oldBottom
      ? oldBottom.rows.slice(Math.max(0, insideEnd - Math.round(effScale * 2)), insideEnd + 1)
      : [];
    const oldBottomMax = oldLastRows.length ? Math.max(...oldLastRows.map((row) => row.max)) : null;

    const result = {
      id: tile.id,
      width,
      mean: tile.mean,
      hovered: state.hovered,
      rect: state.rect,
      shade: state.shade,
      bottomClip,
      rows: { effScale, insideEnd, bottomMax, prevMax, belowMax, midMax, midMean },
      beforeFix: { bottomMax: oldBottomMax, shade: oldData },
    };
    report.cases[label] = result;

    const finalStopOk = /rgb\(0, 0, 0\)(\s+100%)?\)\s*$/.test(state.shade ?? '');
    if (label.startsWith('bright')) {
      check(label + ' 选中确实是亮图（中段 max > 90）', midMax > 90, 'midMax=' + midMax);
    }
    check(label + ' 悬停生效', state.hovered === true, 'hovered=' + state.hovered);
    check(label + ' 渐变末档全不透明', finalStopOk, state.shade ?? 'null');
    check(label + ' 底边 1px 无透出（max <= 12）', bottomMax <= 12, `bottomMax=${bottomMax}`);
    check(label + ' 底边比上方更暗（无回亮）', insideEnd + 1 >= Math.round(effScale) ? bottomMax <= prevMax + 2 : true, `bottom=${bottomMax} prev=${prevMax}`);
    check(label + ' 底边以下无浅色条（max <= 45）', belowMax <= 45, `belowMax=${belowMax}`);
    if (oldBottomMax !== null) {
      report.cases[label].ab = { after: bottomMax, before: oldBottomMax };
      console.log(`${label}: bottomMax after=${bottomMax} before=${oldBottomMax} midMax=${midMax} belowMax=${belowMax}`);
    }
    await hover(4, 4);
    await sleep(500);
    return result;
  };

  /* ===== 1440×900 ===== */
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  await cdp.send('Page.navigate', { url: base + '#/' });
  await sleep(900);
  await settle();
  await sleep(500);
  await waitStable();
  const desktopPicks = await evaluate(PICK_TILES);
  report.desktopPicks = desktopPicks;
  check('1440 候选卡面存在', desktopPicks.bright !== null && desktopPicks.dark !== null, JSON.stringify({ total: desktopPicks.total, tainted: desktopPicks.tainted }));
  const bright1440 = await runCase('bright-1440', desktopPicks.bright, 1440);
  const dark1440 = await runCase('dark-1440', desktopPicks.dark, 1440);

  /* ===== 390×844 ===== */
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: false });
  await cdp.send('Page.navigate', { url: base + '#/' });
  await sleep(900);
  await settle();
  await sleep(500);
  await waitStable();
  const mobilePicks = await evaluate(PICK_TILES);
  report.mobilePicks = mobilePicks;
  check('390 候选卡面存在', mobilePicks.bright !== null && mobilePicks.dark !== null, JSON.stringify({ total: mobilePicks.total, tainted: mobilePicks.tainted }));
  await runCase('bright-390', mobilePicks.bright, 390);
  await runCase('dark-390', mobilePicks.dark, 390);

  check('console 无报错', report.consoleErrors.length === 0, report.consoleErrors.join(' | '));
  writeFileSync(join(outDir, 'hover-edge-check.json'), JSON.stringify(report, null, 2) + '\n', 'utf8');
  console.log(JSON.stringify({ failures: report.failures, checks: report.checks.map((item) => (item.ok ? 'PASS ' : 'FAIL ') + item.name) }, null, 2));
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
