#!/usr/bin/env node
/**
 * 概要遮罩边缘像素探针（分数缩放下的 1px 亮线 + 反馈环形态）。
 *
 * 用法（在 web/ 里起服务，或直接指向 dist）：
 *   node scripts/wall_probe_mask_edge.mjs --serve-dist web/dist --label before
 *   node scripts/wall_probe_mask_edge.mjs --base http://127.0.0.1:4273/ --label after
 *
 * 断言（每个缩放档都跑）：
 *  1) 右上角概要打开后，卡片四边「最外 2 CSS px」的平均亮度与「内缩 4–6 CSS px」
 *     的参考带亮度差 ≤3/255。分数缩放 1.2635 / 1.6 以及 DPR 2.5 都要过。
 *     根因：遮罩是 overflow:hidden 的子元素，自己的矩形边缘也做一遍抗锯齿，
 *     在分数坐标下边界像素的遮罩覆盖 = c²，而图片覆盖 = c，于是最外一像素
 *     漏出图片/描边，形成 1px 亮线。修复把遮罩外扩到裁切边界之外。
 *  2) 遮罩矩形在四边都超出卡片 ≥1.5px（结构证据），内容内缩仍是 12px
 *     （内边距 14 - 外扩 2），中段可读性不回退。
 *  3) 概要与悬停反馈都画在 `.tile__ring`（z-index 6 > 遮罩 5、pointer-events:none、
 *     border-radius: inherit、四边统一的 2px inset 环），不再由父级 box-shadow
 *     透光细线表达；概要态下装饰环（hover/current/spotlight）关闭，
 *     仅键盘 focus-visible 仍保留 2px accent 环。
 *  4) 当前项（单击聚焦）的环是 2px accent，位于遮罩层之上。
 *
 * 验收产物：1440×900 概要态整屏截图 + 卡片裁切图 + 像素采样 JSON。
 * 截图含图库内容，只落本地，不上传。
 *
 * 退出码：0 通过；1 断言失败；2 环境问题。
 */
import { spawn } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { extname, join, resolve } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const CANDIDATES = [
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
  'C:/Program Files/Microsoft/Edge/Application/msedge.exe',
];

const VIEWPORT = { width: 1440, height: 900 };
/** 边缘亮度容差（0–255）。 */
const EDGE_TOLERANCE = 3;
/** 概要态的反馈环必须统一 2px。 */
const RING_SPREAD = 2;

function arg(name, fallback) {
  const index = process.argv.indexOf('--' + name);
  return index >= 0 ? process.argv[index + 1] : fallback;
}

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
    return new Promise((resolvePromise, reject) => {
      this.pending.set(id, { resolve: resolvePromise, reject });
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

function hardDelete(dir) {
  if (!dir || !existsSync(dir)) return;
  try {
    rmSync(dir, { recursive: true, force: true });
  } catch {
    /* 删不掉就算了 */
  }
}

/** 探针自带的静态服务：只需要 no-cache 的 html，数据/图片无所谓。 */
function startStaticServer(root) {
  const mime = {
    '.html': 'text/html; charset=utf-8',
    '.js': 'text/javascript; charset=utf-8',
    '.css': 'text/css; charset=utf-8',
    '.json': 'application/json; charset=utf-8',
    '.jpg': 'image/jpeg',
    '.jpeg': 'image/jpeg',
    '.png': 'image/png',
    '.webp': 'image/webp',
    '.gif': 'image/gif',
    '.svg': 'image/svg+xml',
  };
  const server = createServer(async (request, response) => {
    try {
      const pathname = decodeURIComponent(new URL(request.url, 'http://127.0.0.1').pathname);
      let relative = pathname === '/' ? '/index.html' : pathname;
      if (relative.endsWith('/')) relative += 'index.html';
      const file = resolve(root, '.' + relative);
      if (!file.startsWith(resolve(root))) {
        response.statusCode = 403;
        response.end('forbidden');
        return;
      }
      const data = await readFile(file);
      response.setHeader('Content-Type', mime[extname(file).toLowerCase()] ?? 'application/octet-stream');
      response.setHeader('Cache-Control', 'no-cache');
      response.end(data);
    } catch {
      response.statusCode = 404;
      response.end('not found');
    }
  });
  return new Promise((resolvePromise, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      resolvePromise({ server, port: server.address().port });
    });
  });
}

async function evaluate(cdp, expression, awaitPromise = false) {
  const result = await cdp.send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise,
  });
  if (result.exceptionDetails) {
    throw new Error('页面求值失败：' + (result.exceptionDetails.text ?? 'unknown'));
  }
  return result.result.value;
}

async function waitFor(cdp, expression, label, timeoutMs = 5000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await evaluate(cdp, expression)) return true;
    await sleep(100);
  }
  throw new Error('等待超时：' + label);
}

/** 在页面里挑一张完整可见、且完全在导航缝右侧、图片已解码的卡。 */
async function pickTile(cdp, excludeId) {
  const list = await evaluate(
    cdp,
    `(() => {
      const seam = parseFloat(getComputedStyle(document.documentElement).getPropertyValue('--seam-w')) || 0;
      const vw = innerWidth, vh = innerHeight;
      return [...document.querySelectorAll('.tile')].map((tile) => {
        const rect = tile.getBoundingClientRect();
        const img = tile.querySelector('.tile__img');
        return {
          id: tile.dataset.id,
          current: tile.dataset.current === 'true',
          summary: tile.dataset.summary === 'true',
          loaded: Boolean(img && img.classList.contains('is-loaded')),
          left: rect.left,
          top: rect.top,
          right: rect.right,
          bottom: rect.bottom,
          w: rect.width,
          h: rect.height,
          worldW: parseFloat(tile.style.width),
        };
      }).filter((tile) =>
        !tile.summary && tile.loaded && tile.worldW > 0 &&
        tile.left >= seam + 24 && tile.top >= 24 &&
        tile.right <= vw - 24 && tile.bottom <= vh - 24 &&
        tile.w >= 100 && tile.h >= 100
      );
    })()`,
  );
  const candidates = list.filter((tile) => tile.id !== excludeId && !tile.current);
  const pool = candidates.length > 0 ? candidates : list;
  if (pool.length === 0) throw new Error('视口里没有可用的完整卡片');
  // 越靠视口中部越不容易被动画/滚动影响，按中心距离取最近的一张。
  const cx = VIEWPORT.width / 2;
  const cy = VIEWPORT.height / 2;
  pool.sort(
    (left, right) =>
      Math.hypot((left.left + left.right) / 2 - cx, (left.top + left.bottom) / 2 - cy) -
      Math.hypot((right.left + right.right) / 2 - cx, (right.top + right.bottom) / 2 - cy),
  );
  return pool[0];
}

async function mouse(cdp, type, x, y, button = 'none', buttons = 0, modifiers = 0) {
  await cdp.send('Input.dispatchMouseEvent', { type, x, y, button, buttons, modifiers, clickCount: type === 'mouseMoved' ? 0 : 1 });
}

async function rightClick(cdp, x, y) {
  await mouse(cdp, 'mouseMoved', x, y);
  await mouse(cdp, 'mousePressed', x, y, 'right', 2);
  await mouse(cdp, 'mouseReleased', x, y, 'right', 0);
}

async function leftClick(cdp, x, y) {
  await mouse(cdp, 'mouseMoved', x, y);
  await mouse(cdp, 'mousePressed', x, y, 'left', 1);
  await mouse(cdp, 'mouseReleased', x, y, 'left', 0);
}

async function readTile(cdp, id) {
  return await evaluate(
    cdp,
    `(() => {
      const tile = document.querySelector('.tile[data-id=' + JSON.stringify(${JSON.stringify(id)}) + ']');
      if (!tile) return null;
      const info = tile.querySelector('.tile__info');
      const body = tile.querySelector('.tile__info-body');
      const media = tile.querySelector('.tile__media');
      const ring = tile.querySelector('.tile__ring');
      const tileRect = tile.getBoundingClientRect();
      const round = (rect) => rect ? { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom, width: rect.width, height: rect.height } : null;
      const tileStyle = getComputedStyle(tile);
      return {
        tile: round(tileRect),
        info: round(info ? info.getBoundingClientRect() : null),
        body: round(body ? body.getBoundingClientRect() : null),
        media: round(media ? media.getBoundingClientRect() : null),
        current: tile.dataset.current === 'true',
        summary: tile.dataset.summary === 'true',
        hovered: tile.matches(':hover'),
        radius: tileStyle.borderTopLeftRadius,
        ring: ring
          ? {
              present: true,
              boxShadow: getComputedStyle(ring).boxShadow,
              zIndex: getComputedStyle(ring).zIndex,
              pointerEvents: getComputedStyle(ring).pointerEvents,
              borderRadius: getComputedStyle(ring).borderTopLeftRadius,
              position: getComputedStyle(ring).position,
              isChild: ring.parentElement === tile,
            }
          : { present: false },
        maskBackground: info ? getComputedStyle(info).backgroundImage : null,
      };
    })()`,
  );
}

async function capture(cdp, clip, scale = 1) {
  const result = await cdp.send('Page.captureScreenshot', {
    format: 'png',
    fromSurface: true,
    captureBeyondViewport: false,
    clip: { x: clip.x, y: clip.y, width: clip.width, height: clip.height, scale },
  });
  return result.data;
}

/**
 * 在页面里把截图解码到 canvas 再采样：外层 2 CSS px / 1 CSS px 带 vs 内缩 4–6 px 参考带。
 * 四边都取卡片中段 50% 的跨度，避开圆角。
 */
async function sampleEdges(cdp, base64, clip, tile) {
  const script = `(async () => {
    const img = new Image();
    img.src = 'data:image/png;base64,' + ${JSON.stringify(base64)};
    await img.decode();
    const canvas = document.createElement('canvas');
    canvas.width = img.width;
    canvas.height = img.height;
    const ctx = canvas.getContext('2d', { willReadFrequently: true });
    ctx.drawImage(img, 0, 0);
    const data = ctx.getImageData(0, 0, img.width, img.height).data;
    const lum = (x, y) => {
      const i = (Math.min(img.height - 1, Math.max(0, y)) * img.width + Math.min(img.width - 1, Math.max(0, x))) * 4;
      return 0.2126 * data[i] + 0.7152 * data[i + 1] + 0.0722 * data[i + 2];
    };
    const scale = img.width / ${clip.width};
    const px = (value) => Math.round(value * scale);
    const left = (${tile.left} - ${clip.x}) * scale;
    const right = (${tile.right} - ${clip.x}) * scale;
    const top = (${tile.top} - ${clip.y}) * scale;
    const bottom = (${tile.bottom} - ${clip.y}) * scale;
    const spanX = [left + (right - left) * 0.25, left + (right - left) * 0.75];
    const spanY = [top + (bottom - top) * 0.25, top + (bottom - top) * 0.75];
    const mean = (x0, x1, y0, y1) => {
      let total = 0, count = 0, max = 0;
      for (let y = Math.max(0, Math.ceil(y0)); y < Math.min(img.height, Math.ceil(y1)); y += 1) {
        for (let x = Math.max(0, Math.ceil(x0)); x < Math.min(img.width, Math.ceil(x1)); x += 1) {
          const value = lum(x, y);
          total += value;
          count += 1;
          if (value > max) max = value;
        }
      }
      return { mean: count ? total / count : null, max: count ? max : null, count };
    };
    // left/right/top/bottom 已经是设备像素；这里只取整，不能再乘 scale。
    const L = Math.round(left), R = Math.round(right), T = Math.round(top), B = Math.round(bottom);
    const band2 = px(2), band1 = px(1), refA = px(4), refB = px(6);
    const side = (outer, inner) => ({
      outer2: outer,
      inner: inner,
      delta2: outer.mean === null || inner.mean === null ? null : Math.abs(outer.mean - inner.mean),
    });
    return {
      imageWidth: img.width,
      imageHeight: img.height,
      pxScale: scale,
      bounds: { left, right, top, bottom },
      refBands: { band1, band2, refA, refB },
      edges: {
        top: side(
          mean(spanX[0], spanX[1], T, T + band2),
          mean(spanX[0], spanX[1], T + refA, T + refB),
        ),
        bottom: side(
          mean(spanX[0], spanX[1], B - band2, B),
          mean(spanX[0], spanX[1], B - refB, B - refA),
        ),
        left: side(
          mean(L, L + band2, spanY[0], spanY[1]),
          mean(L + refA, L + refB, spanY[0], spanY[1]),
        ),
        right: side(
          mean(R - band2, R, spanY[0], spanY[1]),
          mean(R - refB, R - refA, spanY[0], spanY[1]),
        ),
      },
      outer1: {
        top: mean(spanX[0], spanX[1], T, T + band1),
        bottom: mean(spanX[0], spanX[1], B - band1, B),
        left: mean(L, L + band1, spanY[0], spanY[1]),
        right: mean(R - band1, R, spanY[0], spanY[1]),
      },
    };
  })()`;
  return await evaluate(cdp, script, true);
}

/** 解析 computed box-shadow，取出 inset 环的四元组。 */
function parseInsetRing(boxShadow) {
  if (!boxShadow || boxShadow === 'none') return [];
  const rings = [];
  const re = /(rgba?\([^)]*\)|#[0-9a-fA-F]{3,8})\s+(-?[\d.]+)px\s+(-?[\d.]+)px\s+(-?[\d.]+)px\s+(-?[\d.]+)px\s+(inset)?/g;
  let match;
  while ((match = re.exec(boxShadow)) !== null) {
    if (!match[6]) continue;
    rings.push({
      color: match[1],
      offsetX: Number(match[2]),
      offsetY: Number(match[3]),
      blur: Number(match[4]),
      spread: Number(match[5]),
    });
  }
  return rings;
}

/** computed box-shadow 里是否有一条 alpha=1 的同色阴影（Chrome 会把 rgb 序列化成 rgba(...,1)）。 */
function shadowHasColor(boxShadow, color) {
  const parse = (text) => {
    const match = /rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:[,\s/]+([\d.]+))?\s*\)/.exec(text);
    return match
      ? { r: +match[1], g: +match[2], b: +match[3], a: match[4] === undefined ? 1 : +match[4] }
      : null;
  };
  const wanted = parse(color);
  if (!wanted || !boxShadow) return false;
  for (const match of boxShadow.matchAll(/rgba?\([^)]+\)/g)) {
    const found = parse(match[0]);
    if (
      found &&
      found.r === wanted.r &&
      found.g === wanted.g &&
      found.b === wanted.b &&
      found.a === 1
    ) {
      return true;
    }
  }
  return false;
}

function ringIsUniform(boxShadow, spreadWanted) {
  const rings = parseInsetRing(boxShadow);
  if (rings.length !== 1) return { pass: false, rings };
  const ring = rings[0];
  const pass =
    ring.offsetX === 0 &&
    ring.offsetY === 0 &&
    ring.blur === 0 &&
    Math.abs(ring.spread - spreadWanted) < 0.01;
  return { pass, rings };
}

/** 屏幕 px → 卡片布局 px（getBoundingClientRect 受相机缩放影响）。 */
function maskCoverage(record, zoom) {
  if (!record.info || !record.tile) return null;
  const scale = zoom || 1;
  return {
    top: (record.tile.top - record.info.top) / scale,
    bottom: (record.info.bottom - record.tile.bottom) / scale,
    left: (record.tile.left - record.info.left) / scale,
    right: (record.info.right - record.tile.right) / scale,
  };
}

function contentInset(record, zoom) {
  // 文案位置要对齐「卡片内容盒」：遮罩外扩 2px + 内边距 14px = 距卡片边 12px。
  if (!record.tile || !record.body) return null;
  const scale = zoom || 1;
  return {
    left: (record.body.left - record.tile.left) / scale,
    bottom: (record.tile.bottom - record.body.bottom) / scale,
  };
}

/** 图片裁切层（.tile__media）越过卡片边界的屏幕 px 数（概要态 1 CSS px × 相机缩放）。 */
function mediaBleed(record) {
  if (!record.media || !record.tile) return null;
  return {
    top: record.tile.top - record.media.top,
    bottom: record.media.bottom - record.tile.bottom,
    left: record.tile.left - record.media.left,
    right: record.media.right - record.tile.right,
  };
}

async function main() {
  const serveDist = arg('serve-dist', '');
  const label = arg('label', 'run');
  const outDir = resolve(arg('out', 'web/screenshots/round-a'));
  let staticServer = null;
  let base;
  if (serveDist) {
    const started = await startStaticServer(resolve(serveDist));
    staticServer = started.server;
    base = `http://127.0.0.1:${started.port}/`;
  } else {
    const raw = arg('base', 'http://127.0.0.1:4273/');
    base = raw.endsWith('/') ? raw : raw + '/';
  }
  const chrome = CANDIDATES.find((path) => existsSync(path));
  if (!chrome) {
    console.error('找不到 Chrome/Edge');
    process.exit(2);
  }
  mkdirSync(outDir, { recursive: true });

  const profile = mkdtempSync(join(tmpdir(), 'kanalib-mask-'));
  const port = 10500 + Math.floor(Math.random() * 300);
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
      `--window-size=${VIEWPORT.width},${VIEWPORT.height}`,
      'about:blank',
    ],
    { stdio: 'ignore' },
  );

  const report = {
    label,
    base,
    viewport: VIEWPORT,
    tolerance: EDGE_TOLERANCE,
    cases: [],
    hoverRing: null,
    currentRing: null,
    screenshots: {},
    consoleErrors: [],
    passed: false,
  };
  const failures = [];
  let cdp = null;
  try {
    await waitForDevtools(port);
    const target = await newTarget(port, 'about:blank');
    cdp = new Cdp(target.webSocketDebuggerUrl);
    await cdp.connect();
    cdp.on((message) => {
      if (message.method === 'Runtime.exceptionThrown') {
        report.consoleErrors.push('exception: ' + (message.params.exceptionDetails?.text ?? 'unknown'));
      } else if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
        report.consoleErrors.push(message.params.args.map((item) => item.value ?? item.description ?? '').join(' '));
      }
    });
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');

    const setMetrics = (deviceScaleFactor) =>
      cdp.send('Emulation.setDeviceMetricsOverride', {
        width: VIEWPORT.width,
        height: VIEWPORT.height,
        deviceScaleFactor,
        mobile: false,
      });

    /** 每次重载都带唯一 query：重置相机（zoom 1）与所有交互状态。 */
    let nonce = 0;
    const reload = async () => {
      nonce += 1;
      await cdp.send('Page.navigate', { url: base + '?probe=mask-' + nonce + '#/t/solo?sort=name' });
      await waitFor(cdp, `document.querySelectorAll('.tile').length >= 6`, '墙挂载');
      await waitFor(
        cdp,
        `[...document.querySelectorAll('.tile')].filter((tile) => tile.querySelector('.tile__img.is-loaded')).length >= 4`,
        '首屏图片解码',
      );
      await sleep(400);
    };

    const zoomTo = async (deltaY) => {
      const x = Math.round(VIEWPORT.width * 0.6);
      const y = Math.round(VIEWPORT.height * 0.5);
      await mouse(cdp, 'mouseMoved', x, y);
      await cdp.send('Input.dispatchMouseEvent', {
        type: 'mouseWheel',
        x,
        y,
        deltaX: 0,
        deltaY,
        modifiers: 2,
      });
      await sleep(900);
    };

    const runCase = async ({ name, dpr, deltaY, hover = true }) => {
      await setMetrics(dpr);
      await reload();
      if (deltaY) await zoomTo(deltaY);
      const picked = await pickTile(cdp);
      const zoomActual = picked.w / picked.worldW;
      const centerX = (picked.left + picked.right) / 2;
      const centerY = (picked.top + picked.bottom) / 2;
      await rightClick(cdp, centerX, centerY);
      await waitFor(cdp, `Boolean(document.querySelector('.tile[data-summary="true"]'))`, '概要打开');
      await sleep(360);
      if (!hover) {
        // 复验「鼠标移开」的同一状态：悬停滤镜/抬升不再参与合成。
        await mouse(cdp, 'mouseMoved', VIEWPORT.width - 8, VIEWPORT.height - 8);
        await sleep(320);
      }

      const record = await readTile(cdp, picked.id);
      const clip = {
        x: Math.max(0, Math.floor(record.tile.left - 4)),
        y: Math.max(0, Math.floor(record.tile.top - 4)),
        width: 0,
        height: 0,
      };
      clip.width = Math.min(VIEWPORT.width - clip.x, Math.ceil(record.tile.right + 4) - clip.x);
      clip.height = Math.min(VIEWPORT.height - clip.y, Math.ceil(record.tile.bottom + 4) - clip.y);

      const cropName = `mask-edge-${label}-${name}-crop.png`;
      const fullName = `mask-edge-${label}-${name}-full.png`;
      const cropBase64 = await capture(cdp, clip);
      writeFileSync(join(outDir, cropName), Buffer.from(cropBase64, 'base64'));
      const full = await cdp.send('Page.captureScreenshot', { format: 'png', fromSurface: true });
      writeFileSync(join(outDir, fullName), Buffer.from(full.data, 'base64'));
      report.screenshots[name] = { crop: cropName, full: fullName };

      const edges = await sampleEdges(cdp, cropBase64, clip, record.tile);
      const coverage = maskCoverage(record, zoomActual);
      const inset = contentInset(record, zoomActual);
      const bleed = mediaBleed(record);
      const sides = Object.entries(edges.edges).map(([side, value]) => ({
        side,
        outer2Mean: value.outer2.mean,
        innerMean: value.inner.mean,
        // 只把「比内部更亮」当缺陷：裁切边界像素混入页面底色时会比内部暗，
        // 那是正常抗锯齿过渡，不是亮线。
        brightExcess: value.outer2.mean === null || value.inner.mean === null
          ? null
          : value.outer2.mean - value.inner.mean,
        absDelta: value.delta2,
        outer2Count: value.outer2.count,
        outer1Mean: edges.outer1[side]?.mean ?? null,
      }));
      // 验收口径：四边最外 2px 与内部的亮度差 ≤3/255（既不能更亮，也不能明显更暗）。
      const edgePass = sides.every(
        (side) =>
          side.brightExcess !== null &&
          side.brightExcess <= EDGE_TOLERANCE &&
          side.absDelta !== null &&
          side.absDelta <= EDGE_TOLERANCE,
      );
      const coveragePass = Boolean(coverage) && Object.values(coverage).every((value) => value >= 1.5);
      const contentPass = Boolean(inset) && Math.abs(inset.left - 12) <= 0.6 && Math.abs(inset.bottom - 12) <= 0.6;
      // 图片裁切层必须越过卡片边界：最外设备像素才会被图片完整覆盖。
      const bleedPass = Boolean(bleed) && Object.values(bleed).every((value) => value >= 0.5);
      const summaryRingPass = record.ring.present === false || record.ring.boxShadow === 'none';

      const caseResult = {
        name,
        dpr,
        zoomRequested: deltaY ? undefined : 1,
        zoomActual,
        wheelDeltaY: deltaY ?? 0,
        tileId: picked.id,
        tile: record.tile,
        info: record.info,
        media: record.media,
        radius: record.radius,
        maskCoverage: coverage,
        mediaBleed: bleed,
        contentInset: inset,
        maskBackground: record.maskBackground,
        sample: {
          imageWidth: edges.imageWidth,
          imageHeight: edges.imageHeight,
          pxScale: edges.pxScale,
          bounds: edges.bounds,
          refBands: edges.refBands,
        },
        edges: Object.fromEntries(sides.map((side) => [side.side, side])),
        edgePass,
        coveragePass,
        contentPass,
        bleedPass,
        summary: {
          current: record.current,
          hovered: record.hovered,
          ring: record.ring,
          suppressed: summaryRingPass,
        },
      };
      report.cases.push(caseResult);
      if (!edgePass) {
        failures.push(
          `${name} 边缘亮度差超 3/255：` +
            sides
              .map(
                (side) =>
                  `${side.side}=${side.brightExcess === null ? 'n/a' : side.brightExcess.toFixed(1)}` +
                  `(abs ${side.absDelta === null ? 'n/a' : side.absDelta.toFixed(1)})`,
              )
              .join(' '),
        );
      }
      if (!coveragePass) failures.push(`${name} 遮罩未外扩到裁切边界：${JSON.stringify(coverage)}`);
      if (!bleedPass) failures.push(`${name} 图片裁切层没有越过卡片边界：${JSON.stringify(bleed)}`);
      if (!contentPass) failures.push(`${name} 内容内缩不是 12px：${JSON.stringify(inset)}`);
      if (!summaryRingPass) failures.push(`${name} 概要态装饰环没关掉：${record.ring.boxShadow}`);

      // 关掉概要，等浮层卸载（160ms 淡出 + 余量）。
      await rightClick(cdp, centerX, centerY);
      await waitFor(cdp, `!document.querySelector('.tile[data-summary="true"]')`, '概要关闭');
      await sleep(280);
      return caseResult;
    };

    // 1) 相机缩放 1 / 1.2635 / 1.6（DPR 1）。deltaY -120 → ×1.264，-400 → 夹到 1.6。
    await runCase({ name: 'zoom-1', dpr: 1, deltaY: 0 });
    await runCase({ name: 'zoom-1.264', dpr: 1, deltaY: -120 });
    await runCase({ name: 'zoom-1.264-nohover', dpr: 1, deltaY: -120, hover: false });
    await runCase({ name: 'zoom-1.6', dpr: 1, deltaY: -400 });
    // 2) 相机上限是 1.6；2.5 用 DPR 2.5 在 zoom 1 下压分数设备像素边界。
    await runCase({ name: 'dpr-2.5', dpr: 2.5, deltaY: 0 });

    // 3) 反馈环形态：普通悬停 + 单击聚焦后的「当前项」。
    await setMetrics(1);
    await reload();
    const hoverTile = await pickTile(cdp);
    const hoverCenter = {
      x: (hoverTile.left + hoverTile.right) / 2,
      y: (hoverTile.top + hoverTile.bottom) / 2,
    };
    await mouse(cdp, 'mouseMoved', hoverCenter.x, hoverCenter.y);
    await sleep(260);
    const hovered = await readTile(cdp, hoverTile.id);
    const hoverForm = ringIsUniform(hovered.ring.boxShadow, RING_SPREAD);
    const hoverPass =
      hovered.ring.present === true &&
      hovered.ring.isChild === true &&
      hovered.ring.zIndex !== 'auto' &&
      Number(hovered.ring.zIndex) > 5 &&
      hovered.ring.pointerEvents === 'none' &&
      hovered.ring.borderRadius === hovered.radius &&
      hoverForm.pass;
    report.hoverRing = {
      tileId: hoverTile.id,
      ring: hovered.ring,
      tileRadius: hovered.radius,
      parsed: hoverForm.rings,
      formPass: hoverPass,
    };
    if (!hoverPass) failures.push('hover 反馈不是遮罩之上的统一 2px 环：' + JSON.stringify(hovered.ring));
    const hoverShot = await cdp.send('Page.captureScreenshot', { format: 'png', fromSurface: true });
    report.screenshots.hover = `mask-edge-${label}-hover.png`;
    writeFileSync(join(outDir, report.screenshots.hover), Buffer.from(hoverShot.data, 'base64'));
    // 悬停卡局部：看 2px 环的形状（整圈、同宽、贴圆角）。
    const hoverClip = {
      x: Math.max(0, Math.floor(hovered.tile.left - 8)),
      y: Math.max(0, Math.floor(hovered.tile.top - 8)),
      width: 0,
      height: 0,
    };
    hoverClip.width = Math.min(VIEWPORT.width - hoverClip.x, Math.ceil(hovered.tile.right + 8) - hoverClip.x);
    hoverClip.height = Math.min(VIEWPORT.height - hoverClip.y, Math.ceil(hovered.tile.bottom + 8) - hoverClip.y);
    const hoverCrop = await capture(cdp, hoverClip);
    report.screenshots.hoverCrop = `mask-edge-${label}-hover-crop.png`;
    writeFileSync(join(outDir, report.screenshots.hoverCrop), Buffer.from(hoverCrop, 'base64'));

    await leftClick(cdp, hoverCenter.x, hoverCenter.y);
    await sleep(700);
    const currentTile = await readTile(cdp, hoverTile.id);
    const currentForm = ringIsUniform(currentTile.ring.boxShadow, RING_SPREAD);
    const accent = await evaluate(
      cdp,
      `(() => {
        const probe = document.createElement('span');
        probe.style.background = getComputedStyle(document.documentElement).getPropertyValue('--accent').trim();
        document.body.appendChild(probe);
        const color = getComputedStyle(probe).backgroundColor;
        probe.remove();
        return color;
      })()`,
    );
    const currentPass =
      currentTile.current === true &&
      currentTile.ring.present === true &&
      currentForm.pass &&
      shadowHasColor(currentTile.ring.boxShadow, accent);
    report.currentRing = {
      tileId: hoverTile.id,
      current: currentTile.current,
      ring: currentTile.ring,
      accent,
      parsed: currentForm.rings,
      formPass: currentPass,
    };
    if (!currentPass) failures.push('当前项环不是 2px accent：' + JSON.stringify(currentTile.ring));
    const currentClip = {
      x: Math.max(0, Math.floor(currentTile.tile.left - 10)),
      y: Math.max(0, Math.floor(currentTile.tile.top - 10)),
      width: 0,
      height: 0,
    };
    currentClip.width = Math.min(VIEWPORT.width - currentClip.x, Math.ceil(currentTile.tile.right + 10) - currentClip.x);
    currentClip.height = Math.min(VIEWPORT.height - currentClip.y, Math.ceil(currentTile.tile.bottom + 10) - currentClip.y);
    const currentCrop = await capture(cdp, currentClip);
    report.screenshots.currentCrop = `mask-edge-${label}-current-crop.png`;
    writeFileSync(join(outDir, report.screenshots.currentCrop), Buffer.from(currentCrop, 'base64'));
    // 4x 放大左上角：确认环是实心 2px、贴着圆角，而不是只有计算样式。
    const cornerClip = {
      x: Math.max(0, Math.floor(currentTile.tile.left - 4)),
      y: Math.max(0, Math.floor(currentTile.tile.top - 4)),
      width: Math.min(120, Math.ceil(currentTile.tile.width)),
      height: Math.min(120, Math.ceil(currentTile.tile.height)),
    };
    const cornerCrop = await capture(cdp, cornerClip, 4);
    report.screenshots.currentCornerZoom = `mask-edge-${label}-current-corner-4x.png`;
    writeFileSync(
      join(outDir, report.screenshots.currentCornerZoom),
      Buffer.from(cornerCrop, 'base64'),
    );

    // 退出聚焦，别把状态带进后续（本轮没有后续，但保持探针可重复）。
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });

    report.passed = failures.length === 0;
    report.failures = failures;
    writeFileSync(join(outDir, `mask-edge-${label}-report.json`), JSON.stringify(report, null, 2));
    console.log(`[mask-edge:${label}] ${report.passed ? 'PASSED' : 'FAILED'}`);
    for (const item of report.cases) {
      const edgeText = Object.values(item.edges)
        .map(
          (side) =>
            `${side.side}=${side.brightExcess === null ? 'n/a' : side.brightExcess.toFixed(1)}`,
        )
        .join(' ');
      console.log(
        `  ${item.name}: zoom=${item.zoomActual.toFixed(4)} dpr=${item.dpr} hover=${item.summary.hovered} edge Δ ${edgeText} | mask ${JSON.stringify(item.maskCoverage)} | ring ${item.summary.ring.present ? item.summary.ring.boxShadow : 'missing'}`,
      );
    }
    console.log(`  hoverRing: ${JSON.stringify(report.hoverRing?.ring ?? null)}`);
    console.log(`  currentRing: ${JSON.stringify(report.currentRing?.ring ?? null)}`);
    for (const failure of failures) console.log('  ✗ ' + failure);
    console.log(`  report: ${join(outDir, `mask-edge-${label}-report.json`)}`);
    process.exitCode = report.passed ? 0 : 1;
  } catch (error) {
    report.passed = false;
    report.failures = [...failures, String(error?.stack ?? error)];
    writeFileSync(join(outDir, `mask-edge-${label}-report.json`), JSON.stringify(report, null, 2));
    console.error(`[mask-edge:${label}] 环境/执行错误：`, error);
    process.exitCode = 2;
  } finally {
    cdp?.close();
    child.kill();
    if (staticServer) staticServer.close();
    await sleep(300);
    hardDelete(profile);
  }
}

await main();
