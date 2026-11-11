#!/usr/bin/env node
/**
 * 标签墙 360° 无界画布 / 侧栏不穿透探针。
 *
 * 用法（先在 web/ 里起服务）：
 *   node scripts/wall_probe_tag360.mjs --base http://127.0.0.1:4273/ --out web/screenshots/tag360
 *
 * 断言：
 *  1) `#/t/solo`：画布左边缘 = 缝宽（桌面 120 / 窄屏 64），缝区域命中不到画布。
 *  2) 缝上按下拖动 / 点击：画布 transform 与 hash 都不变，也不进入聚焦。
 *  3) 初始视野横/纵每 100px 条带都有卡片（无大片空缺）。
 *  4) 点最上面一张卡进入聚焦后，仍然每条带都有卡片，且卡片越过画布上边缘。
 *  5) 向上拖动 420px 相机确实移动（有限世界会被夹在 y=0 不动），移动后依旧无空缺。
 *  6) 390×844 窄屏：画布左边缘 = 64，无横向溢出，条带覆盖正常。
 *
 * 退出码：0 通过；1 断言失败；2 环境问题。
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
  const response = await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent(url)}`, {
    method: 'PUT',
  });
  if (!response.ok) throw new Error('建页面失败：' + response.status);
  return await response.json();
}

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

/** 页面内注入的测量工具（每次 evaluate 都内联一份，避免跨导航丢函数）。 */
const MEASURE = String.raw`
  (() => {
    const canvas = document.querySelector('.canvas');
    if (!canvas) return null;
    const cr = canvas.getBoundingClientRect();
    const world = document.querySelector('.world');
    const tiles = [...document.querySelectorAll('.tile')].map((el) => ({
      id: el.dataset.id || null,
      index: Number(el.dataset.index ?? -1),
      rect: (() => { const r = el.getBoundingClientRect(); return { x: r.left, y: r.top, w: r.width, h: r.height, left: r.left, top: r.top, right: r.right, bottom: r.bottom }; })(),
    }));
    const step = 100;
    const rows = [];
    for (let y = cr.top; y < cr.bottom; y += step) {
      const end = Math.min(cr.bottom, y + step);
      rows.push({
        y: Math.round(y - cr.top),
        hit: tiles.some((t) => t.rect.bottom > y && t.rect.top < end && t.rect.right > cr.left && t.rect.left < cr.right),
      });
    }
    const cols = [];
    for (let x = cr.left; x < cr.right; x += step) {
      const end = Math.min(cr.right, x + step);
      cols.push({
        x: Math.round(x - cr.left),
        hit: tiles.some((t) => t.rect.right > x && t.rect.left < end && t.rect.bottom > cr.top && t.rect.top < cr.bottom),
      });
    }
    const seamEl = document.querySelector('.seam');
    const seamBox = seamEl ? seamEl.getBoundingClientRect() : null;
    const seamRight = seamBox ? seamBox.right : cr.left;
    // 取样/操作点落在缝的中心，而不是"画布左缘的一半"（画布现在从 0 开始）。
    const probeX = seamBox ? seamBox.left + seamBox.width / 2 : Math.max(2, cr.left / 2);
    // 焦点用例要点一张整张都在缝右侧的卡：只完全可见但中心压着缝的话，
    // 点击会被缝区守卫正确拦下，测不到 spotlight。
    const visibleRightOfSeam = tiles.filter(
      (t) =>
        t.rect.top >= cr.top &&
        t.rect.bottom <= cr.bottom &&
        t.rect.left >= seamRight - 1 &&
        t.rect.right <= cr.right,
    );
    const fullyVisible = (visibleRightOfSeam.length ? visibleRightOfSeam : tiles.filter(
      (t) => t.rect.top >= cr.top && t.rect.bottom <= cr.bottom && t.rect.left >= cr.left && t.rect.right <= cr.right,
    )).sort((a, b) => a.rect.top - b.rect.top || a.rect.left - b.rect.left);
    const topmost = fullyVisible[0] || null;
    const head = document.querySelector('.wallhead');
    const headRect = head ? head.getBoundingClientRect() : null;
    const headProbe = headRect
      ? document.elementFromPoint(headRect.left + Math.min(40, headRect.width / 2), headRect.top + headRect.height / 2)
      : null;
    const seamProbe = document.elementFromPoint(probeX, cr.top + cr.height / 2);
    const blank = (() => {
      for (let y = cr.top + 120; y < cr.bottom - 120; y += 40) {
        const el = document.elementFromPoint(probeX, y);
        if (el && !el.closest('.canvas') && !el.closest('button, a, input, select, textarea')) {
          return { x: probeX, y };
        }
      }
      return { x: probeX, y: cr.top + cr.height / 2 };
    })();
    const seamRect = document.querySelector('.seam')?.getBoundingClientRect() ?? null;
    return {
      viewport: { w: innerWidth, h: innerHeight },
      canvas: { left: cr.left, top: cr.top, w: cr.width, h: cr.height },
      seam: seamRect ? { left: seamRect.left, right: seamRect.right, width: seamRect.width } : null,
      // 画布必须真的铺到缝下面，玻璃才有内容可透（不是"缝右边才开始画"）。
      canvasCoversSeam: !!seamRect && cr.left <= seamRect.left && cr.right >= seamRect.right,
      transform: world ? world.style.transform : null,
      zoom: world ? new DOMMatrixReadOnly(getComputedStyle(world).transform).a : 1,
      tileCount: tiles.length,
      uniqueIds: new Set(tiles.map((t) => t.id)).size,
      rows,
      cols,
      rowsMissing: rows.filter((r) => !r.hit).map((r) => r.y),
      colsMissing: cols.filter((c) => !c.hit).map((c) => c.x),
      aboveTop: tiles.filter((t) => t.rect.top < cr.top).length,
      topmost,
      headInside: !!(headProbe && headProbe.closest('.wallhead')),
      seamInsideCanvas: !!(seamProbe && seamProbe.closest('.canvas')),
      seamTag: seamProbe ? (typeof seamProbe.className === 'string' ? seamProbe.className : seamProbe.tagName) : null,
      blank,
      spotlight: canvas.classList.contains('is-spotlight'),
      hash: location.hash,
      overflowX: document.documentElement.scrollWidth - innerWidth,
      tileWidths: tiles.slice(0, 6).map((t) => Math.round(t.rect.w)),
      tileSample: tiles.slice(0, 6).map((t) => ({ x: Math.round(t.rect.x), y: Math.round(t.rect.y), w: Math.round(t.rect.w), h: Math.round(t.rect.h) })),
      seamPoints: Array.from({ length: 9 }, (_, i) => {
        const y = cr.top + 60 + i * 90;
        const el = document.elementFromPoint(probeX, y);
        return { y, insideCanvas: !!(el && el.closest('.canvas')), tag: el ? (typeof el.className === 'string' ? el.className : el.tagName) : null };
      }),
    };
  })()
`;

async function main() {
  const raw = arg('base', 'http://127.0.0.1:4273/');
  const base = raw.endsWith('/') ? raw : raw + '/';
  const outDir = resolve(arg('out', 'web/screenshots/tag360'));
  const chrome = CANDIDATES.find((path) => existsSync(path));
  if (!chrome) {
    console.error('找不到 Chrome/Edge');
    process.exit(2);
  }

  mkdirSync(outDir, { recursive: true });
  const profile = mkdtempSync(join(tmpdir(), 'kanalib-tag360-'));
  const port = 9900 + Math.floor(Math.random() * 300);
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
      '--window-size=1440,900',
      'about:blank',
    ],
    { stdio: 'ignore' },
  );

  const report = { base, outDir, errors: [], cases: {}, passed: false };
  let cdp = null;
  try {
    await waitForDevtools(port);
    const target = await newTarget(port, 'about:blank');
    cdp = new Cdp(target.webSocketDebuggerUrl);
    await cdp.connect();
    cdp.on((message) => {
      if (message.method === 'Runtime.exceptionThrown') {
        report.errors.push('exception: ' + (message.params.exceptionDetails?.text ?? 'unknown'));
      } else if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
        report.errors.push(message.params.args.map((a) => a.value ?? a.description ?? '').join(' '));
      }
    });
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');

    const evaluate = async (expression) => {
      const result = await cdp.send('Runtime.evaluate', {
        expression,
        returnByValue: true,
        awaitPromise: true,
      });
      if (result.exceptionDetails) {
        throw new Error(
          '页面脚本异常：' + (result.exceptionDetails.exception?.description ?? result.exceptionDetails.text),
        );
      }
      return result.result?.value;
    };
    const shot = async (name, clip) => {
      const png = await cdp.send('Page.captureScreenshot', clip ? { format: 'png', clip } : { format: 'png' });
      const path = join(outDir, name);
      writeFileSync(path, Buffer.from(png.data, 'base64'));
      return path;
    };
    const mouse = async (type, x, y, extra = {}) => {
      await cdp.send('Input.dispatchMouseEvent', {
        type,
        x,
        y,
        button: type === 'mouseMoved' ? 'none' : 'left',
        buttons: type === 'mouseReleased' ? 0 : 1,
        clickCount: 1,
        ...extra,
      });
    };
    const drag = async (fromX, fromY, toX, toY, steps = 12) => {
      await mouse('mousePressed', fromX, fromY);
      for (let i = 1; i <= steps; i += 1) {
        const t = i / steps;
        await mouse('mouseMoved', fromX + (toX - fromX) * t, fromY + (toY - fromY) * t);
        await sleep(16);
      }
      await mouse('mouseReleased', toX, toY);
      await sleep(60);
    };
    const click = async (x, y) => {
      await mouse('mousePressed', x, y);
      await sleep(30);
      await mouse('mouseReleased', x, y);
      await sleep(60);
    };
    const waitReady = async (label, timeoutMs = 45000, minTiles = 12) => {
      const deadline = Date.now() + timeoutMs;
      let prevSig = null;
      let stable = 0;
      let measured = null;
      let loaded = false;
      while (Date.now() < deadline) {
        measured = await evaluate(MEASURE);
        // 侧栏底部 "已加载 / 总数"：分片全到齐前相机会在 ready 切换时重新居中，
        // 这时候量到的卡片矩形会整体偏移，必须先等数据齐再量几何。
        const countText = await evaluate(
          `document.querySelector('.seam__foot .seam__count')?.textContent.trim() ?? ''`,
        );
        const nums = (countText.match(/[\d,]+/g) || []).map((s) => Number(s.replace(/,/g, '')));
        loaded = nums.length === 2 && nums[0] === nums[1] && nums[0] > 0;
        if (measured && loaded) {
          const sig = [
            measured.transform,
            measured.tileCount,
            ...measured.tileSample.map((t) => `${t.x},${t.y},${t.w},${t.h}`),
          ].join('|');
          if (sig === prevSig) stable += 1;
          else stable = 0;
          prevSig = sig;
          if (stable >= 2) break;
        } else {
          stable = 0;
          prevSig = null;
        }
        await sleep(400);
      }
      if (!measured || measured.tileCount < minTiles || !loaded) {
        throw new Error(
          `${label}: 卡片没就绪（${measured ? measured.tileCount : 'canvas 缺失'}，loaded=${loaded}）`,
        );
      }
      // 入场波次 900ms，等它播完再量矩形。
      await sleep(1100);
      return await evaluate(MEASURE);
    };
    const setViewport = async (width, height, mobile) => {
      await cdp.send('Emulation.setDeviceMetricsOverride', {
        width,
        height,
        deviceScaleFactor: 1,
        mobile,
      });
    };

    // ---------- 桌面 1440×900 ----------
    await setViewport(1440, 900, false);
    await cdp.send('Page.navigate', { url: base + '#/t/solo' });
    const initial = await waitReady('desktop');

    report.cases.desktop = {
      viewport: initial.viewport,
      canvas: initial.canvas,
      canvasCoversSeam: initial.canvasCoversSeam,
      seamWidth: initial.seam ? initial.seam.width : null,
      tileCount: initial.tileCount,
      uniqueIds: initial.uniqueIds,
      rowsMissing: initial.rowsMissing,
      colsMissing: initial.colsMissing,
      aboveTop: initial.aboveTop,
      tileWidths: initial.tileWidths,
    };
    const desktopShot = await shot('tag360-desktop-initial.png');
    // 缝下有图：只截左侧一栏，确认玻璃里能看到画布内容。
    const glassShot = await shot('sidebar-glass-desktop.png', { x: 0, y: 0, width: 260, height: 900, scale: 1 });

    // 缝区硬拦截：画布铺到缝下面后，不再断言"缝内采样点不在 canvas 内"，
    // 而是验证"在缝里按下拖动 220px，画布/路由/聚焦都不动；右侧滚轮照常平移缩放"。
    const seamBefore = await evaluate(`({ transform: document.querySelector('.world')?.style.transform, hash: location.hash })`);
    report.cases.seam = {
      canvasLeft: initial.canvas.left,
      canvasCoversSeam: initial.canvasCoversSeam,
      seamWidth: initial.seam ? initial.seam.width : null,
      seamTag: initial.seamTag,
      // 仅作记录：缝元素自己盖在画布上，命中测试仍返回缝而不是 canvas。
      seamInsideCanvas: initial.seamInsideCanvas,
      seamPointsInsideCanvas: initial.seamPoints.filter((p) => p.insideCanvas).length,
      seamPoints: initial.seamPoints,
      wallheadBlocksClick: initial.headInside,
      blank: initial.blank,
    };
    // 在缝上按下拖动：侧栏可能自己滚动，但画布不能动、路由不能跳。
    await drag(initial.blank.x, initial.blank.y, initial.blank.x, initial.blank.y - 220);
    const seamAfter = await evaluate(
      `({ transform: document.querySelector('.world')?.style.transform, hash: location.hash, spotlight: document.querySelector('.canvas')?.classList.contains('is-spotlight') })`,
    );
    report.cases.seam.before = seamBefore;
    report.cases.seam.after = seamAfter;
    report.cases.seam.moved = seamBefore.transform !== seamAfter.transform;
    report.cases.seam.spotted = seamAfter.spotlight;
    report.cases.seam.hashChanged = seamBefore.hash !== seamAfter.hash;

    // 真实滚轮落在缝上：事件归缝，画布不能动。
    const seamRealWheelBefore = await evaluate(`document.querySelector('.world')?.style.transform`);
    await cdp.send('Input.dispatchMouseEvent', {
      type: 'mouseWheel',
      x: initial.blank.x,
      y: initial.blank.y,
      deltaX: 0,
      deltaY: 240,
    });
    await sleep(450);
    const seamRealWheelAfter = await evaluate(`document.querySelector('.world')?.style.transform`);
    report.cases.seam.realWheelMoved = seamRealWheelBefore !== seamRealWheelAfter;

    // 缝区滚轮（合成事件直接打到画布，模拟事件从缝下冒上来）：guard 必须放行。
    const seamWheelBefore = await evaluate(`document.querySelector('.world')?.style.transform`);
    const seamWheelPrevented = await evaluate(`(() => {
      const host = document.querySelector('.canvas');
      const event = new WheelEvent('wheel', { clientX: 40, clientY: 420, deltaY: 300, bubbles: true, cancelable: true });
      return !host.dispatchEvent(event);
    })()`);
    await sleep(450);
    const seamWheelAfter = await evaluate(`document.querySelector('.world')?.style.transform`);
    report.cases.seam.wheelBefore = seamWheelBefore;
    report.cases.seam.wheelAfter = seamWheelAfter;
    report.cases.seam.wheelPrevented = seamWheelPrevented;
    report.cases.seam.wheelMoved = seamWheelBefore !== seamWheelAfter;

    // 右侧滚轮：仍然可以平移，Ctrl+滚轮仍然可以缩放。
    const wheelX = Math.round(initial.canvas.left + initial.canvas.w * 0.72);
    const wheelY = Math.round(initial.canvas.top + initial.canvas.h * 0.55);
    const panBefore = await evaluate(`document.querySelector('.world')?.style.transform`);
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseWheel', x: wheelX, y: wheelY, deltaX: 0, deltaY: 240 });
    await sleep(500);
    const panAfter = await evaluate(`document.querySelector('.world')?.style.transform`);
    const zoomBefore = await evaluate(
      `(() => { const w = document.querySelector('.world'); return w ? new DOMMatrixReadOnly(getComputedStyle(w).transform).a : 1; })()`,
    );
    await cdp.send('Input.dispatchMouseEvent', {
      type: 'mouseWheel',
      x: wheelX,
      y: wheelY,
      deltaX: 0,
      deltaY: -120,
      modifiers: 2,
    });
    await sleep(500);
    const zoomAfter = await evaluate(
      `(() => { const w = document.querySelector('.world'); return w ? new DOMMatrixReadOnly(getComputedStyle(w).transform).a : 1; })()`,
    );
    report.cases.seam.wheelPoint = {
      x: wheelX,
      y: wheelY,
      target: await evaluate(`(() => { const el = document.elementFromPoint(${wheelX}, ${wheelY}); return el ? (el.closest('.canvas') ? 'canvas' : el.tagName) : null; })()`),
    };
    report.cases.seam.wheelPanMoved = panBefore !== panAfter;
    report.cases.seam.zoomBefore = zoomBefore;
    report.cases.seam.zoomAfter = zoomAfter;
    report.cases.seam.zoomChanged = Math.abs(zoomAfter - zoomBefore) > 0.001;

    // 顶部卡片：点它，截聚焦后的画面。
    const top = initial.topmost;
    if (!top) throw new Error('桌面：没有完全可见的卡片可点');
    await click(top.rect.x + top.rect.w / 2, top.rect.y + top.rect.h / 2);
    await sleep(700);
    const focused = await evaluate(MEASURE);
    report.cases.focus = {
      spotlight: focused.spotlight,
      rowsMissing: focused.rowsMissing,
      colsMissing: focused.colsMissing,
      aboveTop: focused.aboveTop,
      tileCount: focused.tileCount,
    };
    const focusShot = await shot('tag360-desktop-top-card.png');

    // 向上拖 420：有限世界会夹在 y=0 不动；无界世界必须真的移动。
    const beforeDrag = await evaluate(`({ transform: document.querySelector('.world')?.style.transform })`);
    const cx = initial.canvas.left + initial.canvas.w / 2;
    const cy = initial.canvas.top + initial.canvas.h * 0.62;
    await drag(cx, cy, cx, cy - 420);
    await sleep(500);
    const dragged = await evaluate(MEASURE);
    const afterDrag = await evaluate(`({ transform: document.querySelector('.world')?.style.transform })`);
    report.cases.dragUp = {
      before: beforeDrag.transform,
      after: afterDrag.transform,
      moved: beforeDrag.transform !== afterDrag.transform,
      rowsMissing: dragged.rowsMissing,
      colsMissing: dragged.colsMissing,
      aboveTop: dragged.aboveTop,
      tileCount: dragged.tileCount,
    };
    const dragShot = await shot('tag360-desktop-drag-up.png');

    // ---------- 移动端 390×844 ----------
    await setViewport(390, 844, true);
    await cdp.send('Page.navigate', { url: base + '#/t/solo' });
    const mobile = await waitReady('mobile');
    report.cases.mobile = {
      viewport: mobile.viewport,
      canvas: mobile.canvas,
      canvasCoversSeam: mobile.canvasCoversSeam,
      seamWidth: mobile.seam ? mobile.seam.width : null,
      tileCount: mobile.tileCount,
      rowsMissing: mobile.rowsMissing,
      colsMissing: mobile.colsMissing,
      overflowX: mobile.overflowX,
      seamInsideCanvas: mobile.seamInsideCanvas,
      seamPointsInsideCanvas: mobile.seamPoints.filter((p) => p.insideCanvas).length,
    };
    const mobileShot = await shot('tag360-mobile-initial.png');
    const mobileGlassShot = await shot('sidebar-glass-mobile.png', { x: 0, y: 0, width: 160, height: 844, scale: 1 });

    // ---------- 有限小标签（显式排序）：verticalSlack 下点击锚点不偏 ----------
    // 1 张图的标签墙走有限世界：内容比视口矮时整墙垂直居中（verticalSlack > 0），
    // 点击卡片还原世界坐标必须扣掉这段纯视觉余量，否则英雄卡会被甩到错误位置。
    await setViewport(1440, 900, false);
    // 带一个查询参数强制整页重载：前面的滚轮缩放会留在同一文档的相机里，
    // 不清掉会让"点击锚点不偏"的基线带着 1.26× 缩放跑。
    await cdp.send('Page.navigate', { url: base + '?probe=finite#/t/rating:e?sort=name' });
    const finite = await waitReady('finite', 30000, 1);
    const finiteBefore = await evaluate(`(() => {
      const el = document.querySelector('.tile');
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { cx: r.left + r.width / 2, cy: r.top + r.height / 2, w: r.width, h: r.height };
    })()`);
    if (!finiteBefore) throw new Error('finite: 没有卡片可点');
    await click(finiteBefore.cx, finiteBefore.cy);
    await sleep(700);
    const finiteAfter = await evaluate(`(() => {
      const el = document.querySelector('.tile--spotlight') || document.querySelector('.tile');
      if (!el) return null;
      const r = el.getBoundingClientRect();
      const cr = document.querySelector('.canvas').getBoundingClientRect();
      return {
        cx: r.left + r.width / 2,
        cy: r.top + r.height / 2,
        insideX: r.left >= cr.left - 1 && r.right <= cr.right + 1,
        insideY: r.top >= cr.top - 1 && r.bottom <= cr.bottom + 1,
        spotlight: document.querySelector('.canvas')?.classList.contains('is-spotlight'),
      };
    })()`);
    report.cases.finiteAnchor = {
      tileCount: finite.tileCount,
      before: finiteBefore,
      after: finiteAfter,
      delta: finiteAfter
        ? [Math.round(finiteAfter.cx - finiteBefore.cx), Math.round(finiteAfter.cy - finiteBefore.cy)]
        : null,
    };

    const d = report.cases.desktop;
    const seam = report.cases.seam;
    const focus = report.cases.focus;
    const dragUp = report.cases.dragUp;
    const m = report.cases.mobile;
    const fin = report.cases.finiteAnchor;
    report.screenshots = { desktopShot, glassShot, focusShot, dragShot, mobileShot, mobileGlassShot };
    report.passed =
      report.errors.length === 0 &&
      // 画布铺满视口，并且真的盖住缝（玻璃下有图）。
      Math.abs(d.canvas.left) <= 1 &&
      Math.abs(d.canvas.w - d.viewport.w) <= 1 &&
      d.canvasCoversSeam === true &&
      d.tileCount >= 12 &&
      d.rowsMissing.length === 0 &&
      d.colsMissing.length === 0 &&
      // 缝区按下/拖动/滚轮都不驱动画布；右侧滚轮照常平移与缩放。
      Math.abs(seam.canvasLeft) <= 1 &&
      Math.abs(seam.seamWidth - 120) <= 1 &&
      seam.canvasCoversSeam === true &&
      seam.wallheadBlocksClick === false &&
      seam.moved === false &&
      seam.spotted === false &&
      seam.hashChanged === false &&
      seam.realWheelMoved === false &&
      seam.wheelMoved === false &&
      seam.wheelPrevented === false &&
      seam.wheelPanMoved === true &&
      seam.zoomChanged === true &&
      focus.spotlight === true &&
      focus.rowsMissing.length === 0 &&
      focus.colsMissing.length === 0 &&
      focus.aboveTop > 0 &&
      dragUp.moved === true &&
      dragUp.rowsMissing.length === 0 &&
      dragUp.colsMissing.length === 0 &&
      dragUp.aboveTop > 0 &&
      Math.abs(m.canvas.left) <= 1 &&
      Math.abs(m.canvas.w - m.viewport.w) <= 1 &&
      m.canvasCoversSeam === true &&
      Math.abs(m.seamWidth - 64) <= 1 &&
      m.overflowX <= 0 &&
      m.rowsMissing.length === 0 &&
      m.colsMissing.length === 0 &&
      fin.delta !== null &&
      Math.abs(fin.delta[0]) <= 40 &&
      Math.abs(fin.delta[1]) <= 40 &&
      fin.after.spotlight === true &&
      fin.after.insideX === true &&
      fin.after.insideY === true;
  } finally {
    if (cdp) cdp.close();
    try {
      child.kill();
    } catch {
      /* 已退出 */
    }
    hardDelete(profile);
  }

  const reportPath = join(outDir, 'tag360-report.json');
  writeFileSync(reportPath, JSON.stringify(report, null, 2));
  report.reportPath = reportPath;
  console.log(JSON.stringify(report, null, 2));
  process.exit(report.passed ? 0 : 1);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
