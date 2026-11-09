#!/usr/bin/env node
/**
 * 图片墙 headless 视觉验收：四个页面 + 右键概要，一次跑完。
 *
 * 只依赖 Node 22 自带的 WebSocket 与 fetch，不引 puppeteer/playwright。
 * 流程：起 Chrome（--headless=new，随机调试端口）→ CDP 连页面 → 逐页导航、截图、
 * 收集 console 错误 → 落 report.json。
 *
 * 用法（先在 web/ 里起服务）：
 *   node scripts/wall_shots.mjs --base http://127.0.0.1:4318/ --out web/screenshots
 *
 * 参数：
 *   --base   站点根（必须以 / 结尾），默认 http://127.0.0.1:4318/
 *   --out    截图与 report.json 的落脚目录，默认 web/screenshots
 *   --width  桌面视口宽，默认 1440
 *   --height 桌面视口高，默认 900
 *   --chrome Chrome/Edge 可执行文件路径（默认顺序探测）
 *   --keep   保留 profile 目录（排查用）
 *
 * 退出码：0 全部截图落盘；1 有页面失败或控制台报错；2 环境问题（没有浏览器）。
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

function parseArgs(argv) {
  const out = { base: 'http://127.0.0.1:4318/', out: 'web/screenshots', width: 1440, height: 900, chrome: '', keep: false };
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index];
    const value = argv[index + 1];
    if (key === '--keep') {
      out.keep = true;
      index -= 1;
      continue;
    }
    if (key === '--base') out.base = value.endsWith('/') ? value : value + '/';
    else if (key === '--out') out.out = value;
    else if (key === '--width') out.width = Number(value);
    else if (key === '--height') out.height = Number(value);
    else if (key === '--chrome') out.chrome = value;
  }
  return out;
}

function findChrome(explicit) {
  if (explicit) return existsSync(explicit) ? explicit : '';
  return CANDIDATES.find((path) => existsSync(path)) ?? '';
}

/** 最小 CDP 客户端：一条连接、按 id 派发。 */
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

  once(method, timeoutMs = 15000) {
    return new Promise((resolvePromise, reject) => {
      const timer = setTimeout(() => reject(new Error('等待 ' + method + ' 超时')), timeoutMs);
      const listener = (message) => {
        if (message.method !== method) return;
        clearTimeout(timer);
        this.listeners = this.listeners.filter((entry) => entry !== listener);
        resolvePromise(message.params);
      };
      this.listeners.push(listener);
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

/** 永久删除临时目录（不送回收站）：Chrome profile 属于可再生成产物。
 *  沙箱/拦截环境下 fs.rmSync 会先试 trash 再失败，所以 Windows 上直接调 rd。 */
function hardDelete(dir) {
  if (!dir || !existsSync(dir)) return;
  if (process.platform === 'win32') {
    spawnSync('cmd', ['/c', 'rd', '/s', '/q', dir], { stdio: 'ignore' });
    if (!existsSync(dir)) return;
  }
  try {
    rmSync(dir, { recursive: true, force: true });
  } catch {
    /* 删不掉就算了，别把验收搞挂 */
  }
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
  const profile = args.keep
    ? join(outDir, '.profile-shots')
    : mkdtempSync(join(tmpdir(), 'kanalib-shots-'));
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
      `--window-size=${args.width},${args.height}`,
      'about:blank',
    ],
    { stdio: 'ignore' },
  );

  const report = { base: args.base, viewport: { width: args.width, height: args.height }, shots: {}, consoleErrors: [], failures: [] };
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
        report.consoleErrors.push(message.params.args.map((arg) => arg.value ?? arg.description ?? '').join(' '));
      }
    });
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    await cdp.send('Emulation.setDeviceMetricsOverride', {
      width: args.width,
      height: args.height,
      deviceScaleFactor: 1,
      mobile: false,
    });

    const evaluate = async (expression) => {
      const result = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
      if (result.exceptionDetails) throw new Error(result.exceptionDetails.text);
      return result.result.value;
    };

    const shot = async (name) => {
      const { data } = await cdp.send('Page.captureScreenshot', { format: 'png', fromSurface: true });
      const path = join(outDir, name + '.png');
      writeFileSync(path, Buffer.from(data, 'base64'));
      return path;
    };

    const go = async (hash, settleMs = 2200) => {
      const url = args.base + hash;
      await cdp.send('Page.navigate', { url });
      await sleep(settleMs);
    };

    // 1) 首页（dev server 冷启动会做依赖预构建并触发整页重载，所以等卡片真的出现）
    //    同时探一次入场动效：.tile--enter 只在页面进入的窗口内存在，过期就查不到了。
    await cdp.send('Page.navigate', { url: args.base + '#/' });
    let enterSeen = 0;
    for (let attempt = 0; attempt < 60; attempt += 1) {
      try {
        const state = await evaluate(
          `(() => ({ tiles: document.querySelectorAll('.tile').length, enter: document.querySelectorAll('.tile--enter').length }))()`,
        );
        enterSeen = Math.max(enterSeen, state.enter ?? 0);
        if (state.tiles > 0 && attempt > 6) break;
      } catch {
        /* 导航瞬间执行上下文会重建 */
      }
      await sleep(150);
    }
    await sleep(700);
    report.enterAnimated = enterSeen > 0;
    const homeStats = await evaluate(`(() => {
      const tiles = [...document.querySelectorAll('.tile')];
      return {
        tiles: tiles.length,
        accent: getComputedStyle(document.documentElement).getPropertyValue('--accent').trim(),
        seam: document.querySelector('.seam')?.getBoundingClientRect().width ?? 0,
        firstId: tiles[0]?.dataset.id ?? null,
        docHeight: document.querySelector('.world')?.getBoundingClientRect().height ?? 0,
        pending: tiles.filter((tile) => tile.querySelector('img:not(.is-loaded)')).length,
        failed: tiles.filter((tile) => tile.querySelector('.tile__fallback')).length,
      };
    })()`);
    report.shots.home = await shot('v3f-home');
    report.home = homeStats;

    // 2) 详情（用首页第一张）
    if (homeStats.firstId) {
      await go('#/d/' + homeStats.firstId, 2000);
      report.shots.detail = await shot('v3f-detail');
      report.detail = await evaluate(`(() => {
        const img = document.querySelector('.view__img');
        if (!img) return { ok: false };
        const box = img.getBoundingClientRect();
        return { ok: true, w: Math.round(box.width), h: Math.round(box.height), src: img.currentSrc.split('/').slice(-2).join('/') };
      })()`);
    }

    // 3) 标签总览（要等全部 shard 到齐，给 6s）
    await go('#/tags', 6000);
    report.shots.tags = await shot('v3f-tags');
    const tagsStats = await evaluate(`(() => {
      const cards = [...document.querySelectorAll('.tagcard')];
      return { cards: cards.length, first: cards[0]?.querySelector('.tagcard__name')?.textContent ?? null };
    })()`);
    report.tags = tagsStats;

    // 4) 单标签墙
    if (tagsStats.first) {
      await go('#/t/' + encodeURIComponent(tagsStats.first), 2600);
      report.shots.tag = await shot('v3f-tag');
      report.tag = await evaluate(`(() => ({
        tiles: document.querySelectorAll('.tile').length,
        title: document.querySelector('.wallhead__title')?.textContent ?? null,
      }))()`);
    }

    // 5) 右键概要：在首页挑一张不被导航缝压住的卡
    await go('#/', 2600);
    const spot = await evaluate(`(() => {
      const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 120;
      // 无限画布上几乎没有"整张都在视口里"的卡（格子会故意切边），
      // 所以取"与安全区相交面积最大"的那张，再点相交区域的中心。
      const safe = { left: seam + 40, top: 90, right: innerWidth - 60, bottom: innerHeight - 120 };
      let best = null;
      for (const tile of document.querySelectorAll('.tile')) {
        const box = tile.getBoundingClientRect();
        const w = Math.min(box.right, safe.right) - Math.max(box.left, safe.left);
        const h = Math.min(box.bottom, safe.bottom) - Math.max(box.top, safe.top);
        if (w < 60 || h < 60) continue;
        const area = w * h;
        if (!best || area > best.area) {
          best = {
            area,
            x: Math.round(Math.max(box.left, safe.left) + w / 2),
            y: Math.round(Math.max(box.top, safe.top) + h / 2),
          };
        }
      }
      return best ? { x: best.x, y: best.y } : null;
    })()`);
    if (spot) {
      await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: spot.x, y: spot.y, button: 'right', buttons: 2, clickCount: 1 });
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: spot.x, y: spot.y, button: 'right', buttons: 0, clickCount: 1 });
      await sleep(500);
      report.summary = await evaluate(`(() => {
        const node = document.querySelector('.summary');
        if (!node) return { open: false };
        const box = node.getBoundingClientRect();
        return { open: true, left: Math.round(box.left), top: Math.round(box.top), right: Math.round(box.right), bottom: Math.round(box.bottom), inViewport: box.left >= 0 && box.top >= 0 && box.right <= innerWidth && box.bottom <= innerHeight };
      })()`);
      report.shots.summary = await shot('v3f-summary');
    } else {
      report.failures.push('右键概要：找不到不被遮挡的卡片');
    }

    // 6) 交互实测：拖拽 / Ctrl+滚轮 / 键盘路径 / hover（必须在桌面视口跑，坐标才落在画布里）
    const matrixOf = () =>
      evaluate(
        `(() => { const world = document.querySelector('.world'); if (!world) return { a: 0, e: 0, f: 0 }; const m = new DOMMatrixReadOnly(getComputedStyle(world).transform); return { a: m.a, e: m.e, f: m.f }; })()`,
      );

    // 关掉概要，避免它挡在拖拽起点上
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
    await sleep(300);

    const interactions = {};

    // 6.1 hover 先测：缩放/平移之后再找"完整在视口里"的卡会不稳
    const hoverSpot = await evaluate(`(() => {
      const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 120;
      const tile = [...document.querySelectorAll('.tile')].find((node) => {
        const box = node.getBoundingClientRect();
        return box.left > seam + 60 && box.right < innerWidth - 60 && box.top > 120 && box.bottom < innerHeight - 120;
      });
      if (!tile) return null;
      const box = tile.getBoundingClientRect();
      return { x: Math.round(box.left + box.width / 2), y: Math.round(box.top + box.height / 2) };
    })()`);
    if (hoverSpot) {
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: hoverSpot.x, y: hoverSpot.y });
      await sleep(400);
      interactions.hover = await evaluate(`(() => {
        const tile = document.elementFromPoint(${hoverSpot.x}, ${hoverSpot.y})?.closest('.tile');
        if (!tile) return { ok: false };
        const small = tile.querySelector('.tile__copy small');
        return {
          ok: true,
          filter: getComputedStyle(tile).filter,
          detailOpacity: small ? Number(getComputedStyle(small).opacity) : 0,
        };
      })()`);
    }

    // 6.1 左键拖拽平移
    const panBefore = await matrixOf();
    await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: 900, y: 460, button: 'left', buttons: 1, clickCount: 1 });
    for (const step of [1, 2, 3, 4]) {
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 900 - step * 35, y: 460 + step * 18, button: 'left', buttons: 1 });
      await sleep(40);
    }
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: 760, y: 532, button: 'left', buttons: 0, clickCount: 1 });
    await sleep(500);
    const panAfter = await matrixOf();
    interactions.drag = {
      before: [Math.round(panBefore.e), Math.round(panBefore.f)],
      after: [Math.round(panAfter.e), Math.round(panAfter.f)],
      panned: Math.abs(panAfter.e - panBefore.e) > 40 || Math.abs(panAfter.f - panBefore.f) > 40,
      openedLightbox: await evaluate(`!!document.querySelector('.pswp')`),
    };

    // 6.2 Ctrl + 滚轮缩放（以光标为锚）
    const zoomBeforeWheel = (await matrixOf()).a;
    await cdp.send('Input.dispatchMouseEvent', {
      type: 'mouseWheel',
      x: 900,
      y: 460,
      deltaX: 0,
      deltaY: -240,
      modifiers: 2,
    });
    await sleep(400);
    const zoomAfterWheel = (await matrixOf()).a;
    interactions.ctrlWheel = { before: zoomBeforeWheel, after: zoomAfterWheel, zoomed: zoomAfterWheel > zoomBeforeWheel + 0.02 };

    // 6.3 键盘路径：画布聚焦 → ↓（平移）→ Tab（进卡片）→ Enter（开灯箱）
    await evaluate(`document.querySelector('.canvas')?.focus()`);
    const keyBefore = await matrixOf();
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'ArrowDown', code: 'ArrowDown', windowsVirtualKeyCode: 40 });
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'ArrowDown', code: 'ArrowDown', windowsVirtualKeyCode: 40 });
    await sleep(350);
    const keyAfter = await matrixOf();
    await cdp.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9 });
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9 });
    await sleep(250);
    const focusInfo = await evaluate(`(() => { const el = document.activeElement; return { tag: el?.tagName ?? '', cls: el?.className ?? '', role: el?.getAttribute?.('role') ?? '' }; })()`);
    await cdp.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
    await sleep(900);
    const lightboxOpened = await evaluate(`!!document.querySelector('.pswp')`);
    interactions.keyboard = {
      arrowPanned: Math.abs(keyAfter.f - keyBefore.f) > 60,
      focusedRole: focusInfo.role,
      focusedClass: focusInfo.cls,
      enterOpenedLightbox: lightboxOpened,
    };
    if (lightboxOpened) {
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
      await sleep(500);
    }

    // 6.5 侧栏悬停不得改变底色：曾经 hover 把纸面 76% 提到 88%，亮图底下会"暗一下"。
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 980, y: 500 });
    await sleep(450);
    const seamRest = await evaluate(
      `getComputedStyle(document.querySelector('.seam')).backgroundColor`,
    );
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 60, y: 420 });
    await sleep(450);
    const seamHoverStyle = await evaluate(`(() => {
      const node = document.querySelector('.seam');
      if (!node) return { bg: '', blur: '' };
      const style = getComputedStyle(node);
      return { bg: style.backgroundColor, blur: style.backdropFilter || style.webkitBackdropFilter };
    })()`);
    report.shots.seamHover = await shot('v3f-seam-hover');
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x: 980, y: 500 });
    await sleep(200);
    interactions.seamHover = {
      rest: seamRest,
      hover: seamHoverStyle.bg,
      blur: seamHoverStyle.blur,
      stable: seamRest === seamHoverStyle.bg,
    };
    report.interactions = interactions;
    // 7) 规则 11：任意方向连续拖拽 20000px 都不"到头"，且视野里没有空白带
    const coverage = () =>
      evaluate(`(() => {
        const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 120;
        let hit = 0;
        let total = 0;
        // 8px 的缝会造成零星漏点，所以按 40px 网格密采样，只看"成片的空白"。
        for (let x = seam + 20; x < innerWidth - 20; x += 40) {
          for (let y = 20; y < innerHeight - 20; y += 40) {
            total += 1;
            const el = document.elementFromPoint(x, y);
            if (el && el.closest('.tile')) hit += 1;
          }
        }
        return total ? Math.round((hit / total) * 1000) / 1000 : 0;
      })()`);
    const drag = async (x0, y0, x1, y1) => {
      await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: x0, y: y0, button: 'left', buttons: 1, clickCount: 1 });
      for (let step = 1; step <= 4; step += 1) {
        await cdp.send('Input.dispatchMouseEvent', {
          type: 'mouseMoved',
          x: x0 + ((x1 - x0) * step) / 4,
          y: y0 + ((y1 - y0) * step) / 4,
          button: 'left',
          buttons: 1,
        });
        await sleep(28);
      }
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: x1, y: y1, button: 'left', buttons: 0, clickCount: 1 });
      await sleep(200);
    };

    const startMatrix = await matrixOf();
    // 向左拖 18 次（每次 1100px）＝ 看过 20000px 向右的空间
    for (let i = 0; i < 18; i += 1) await drag(1320, 460, 220, 460);
    const pannedMatrix = await matrixOf();
    const horizontalCover = await coverage();
    // 向上拖 8 次 ＝ 纵向也一直有新内容
    for (let i = 0; i < 8; i += 1) await drag(760, 780, 760, 180);
    const verticalMatrix = await matrixOf();
    const verticalCover = await coverage();
    const tileCount = await evaluate(`document.querySelectorAll('.tile').length`);
    await shot('v3f-far');
    await cdp.send('Input.dispatchKeyEvent', { type: 'rawKeyDown', key: 'Home', code: 'Home', windowsVirtualKeyCode: 36 });
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Home', code: 'Home', windowsVirtualKeyCode: 36 });
    await sleep(500);
    const homeMatrix = await matrixOf();
    await shot('v3f-home-after');
    report.rule11 = {
      startX: Math.round(startMatrix.e),
      afterX: Math.round(pannedMatrix.e),
      travelledX: Math.round(startMatrix.e - pannedMatrix.e),
      afterY: Math.round(verticalMatrix.f),
      travelledY: Math.round(startMatrix.f - verticalMatrix.f),
      coverHorizontal: horizontalCover,
      coverVertical: verticalCover,
      domTiles: tileCount,
      homeX: Math.round(homeMatrix.e),
      homeY: Math.round(homeMatrix.f),
    };

    // 8) 窄屏首页（390×844）：放在交互实测之后，避免改了视口把桌面坐标搞乱。
    //    先 reload——交互那几步把相机缩放过，直接改视口会把那张放大的画面带进截图。
    await cdp.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
    await cdp.send('Page.reload');
    for (let attempt = 0; attempt < 40; attempt += 1) {
      const count = await evaluate(`document.querySelectorAll('.tile').length`).catch(() => 0);
      if (count > 0) break;
      await sleep(200);
    }
    await sleep(700);
    report.shots.mobile = await shot('v3f-home-390');
    report.mobile = await evaluate(`(() => {
      const seam = document.querySelector('.seam')?.getBoundingClientRect();
      const world = document.querySelector('.world');
      return {
        seam: seam ? Math.round(seam.width) : 0,
        tiles: document.querySelectorAll('.tile').length,
        scrollW: document.documentElement.scrollWidth,
        worldTransform: world ? getComputedStyle(world).transform : '',
      };
    })()`);
    if (report.mobile) {
      const before = await matrixOf();
      await drag(340, 640, 80, 640);
      const after = await matrixOf();
      report.mobile.canPanHorizontally = Math.abs(before.e - after.e) > 60;
      report.mobile.dragDx = Math.round(before.e - after.e);
    }

    // 8) 双指缩放：两根手指张开，看世界 transform 的 scale 有没有真的变
    await cdp.send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
    const zoomOf = () =>
      evaluate(
        `(() => { const world = document.querySelector('.world'); if (!world) return 0; return Math.round(new DOMMatrixReadOnly(getComputedStyle(world).transform).a * 1000) / 1000; })()`,
      );
    const zoomBefore = await zoomOf();
    await cdp.send('Input.dispatchTouchEvent', {
      type: 'touchStart',
      touchPoints: [{ x: 160, y: 420, id: 1 }, { x: 240, y: 420, id: 2 }],
    });
    await sleep(80);
    await cdp.send('Input.dispatchTouchEvent', {
      type: 'touchMove',
      touchPoints: [{ x: 80, y: 420, id: 1 }, { x: 320, y: 420, id: 2 }],
    });
    await sleep(160);
    const zoomDuring = await zoomOf();
    await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
    await sleep(160);
    report.pinch = { before: zoomBefore, during: zoomDuring, zoomed: zoomDuring > zoomBefore + 0.05 };

    if (!report.home?.tiles) report.failures.push('首页没有渲染出任何卡片');
    if (report.detail && !report.detail.ok) report.failures.push('详情页没有大图');
    if (report.summary && !report.summary.open) report.failures.push('右键概要没打开');
    if (report.summary && report.summary.open && !report.summary.inViewport) report.failures.push('右键概要溢出视口');
    if (report.consoleErrors.length > 0) report.failures.push('控制台有 ' + report.consoleErrors.length + ' 条错误');
    if (report.pinch && !report.pinch.zoomed) report.failures.push('双指缩放没生效');
    if (report.enterAnimated === false) report.failures.push('入场动效没触发');
    if (report.rule11) {
      if (report.rule11.travelledX < 15000) report.failures.push('规则 11：横向没走过 20000px');
      if (report.rule11.travelledY < 5000) report.failures.push('规则 11：纵向走不动');
      if (report.rule11.coverHorizontal < 0.8) report.failures.push('规则 11：走远后出现空白带');
      if (report.rule11.coverVertical < 0.8) report.failures.push('规则 11：纵向走远后出现空白带');
      if (report.rule11.domTiles > 220) report.failures.push('规则 11：DOM 卡片数没有收敛');
      if (Math.abs(report.rule11.homeX) > 900) report.failures.push('Home 没有回到起点');
    }
    if (report.mobile && !report.mobile.canPanHorizontally) report.failures.push('390 下不能横向移动（碰到边界）');
    if (report.interactions) {
      if (!report.interactions.drag?.panned) report.failures.push('拖拽没有平移画布');
      if (report.interactions.drag?.openedLightbox) report.failures.push('拖拽被误判成单击（打开了灯箱）');
      if (!report.interactions.ctrlWheel?.zoomed) report.failures.push('Ctrl+滚轮没有缩放');
      if (!report.interactions.keyboard?.arrowPanned) report.failures.push('画布方向键没有平移');
      if (report.interactions.keyboard?.focusedRole !== 'button') report.failures.push('Tab 没有进到卡片');
      if (!report.interactions.keyboard?.enterOpenedLightbox) report.failures.push('卡片 Enter 没打开灯箱');
      if (report.interactions.hover && report.interactions.hover.ok) {
        if (report.interactions.hover.filter === 'none') report.failures.push('hover 没有滤镜反馈');
        if (report.interactions.hover.detailOpacity < 0.9) report.failures.push('hover 时分辨率行没出现');
      }
      if (report.interactions.seamHover && !report.interactions.seamHover.stable) {
        report.failures.push('侧栏悬停改变了底色（扫过会"暗一下"）');
      }
    }

    writeFileSync(join(outDir, 'report.json'), JSON.stringify(report, null, 2) + '\n', 'utf8');
    console.log(JSON.stringify({ home: report.home, detail: report.detail, tags: report.tags, tag: report.tag, summary: report.summary, mobile: report.mobile, failures: report.failures }, null, 2));
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
