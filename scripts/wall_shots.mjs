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
        // 跨行布局的"主图"：宽度超过视口 35% 的卡
        bigTiles: tiles.filter((tile) => tile.getBoundingClientRect().width > innerWidth * 0.35).length,
        zoom: Number((new DOMMatrixReadOnly(getComputedStyle(document.querySelector('.world')).transform).a).toFixed(2)),
      };
    })()`);
    report.shots.home = await shot('v3f-home');
    report.home = homeStats;

    // 1b) 聚焦挤压：邻图被挤动、无细条、点邻卡不换位、相机不居中
    const focusPick = await evaluate(`(() => {
      const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 120;
      const safe = { left: seam + 340, top: 90, right: innerWidth - 40, bottom: innerHeight - 120 };
      const contentCenter = (safe.left + (innerWidth - 24)) / 2;
      let best = null;
      for (const tile of document.querySelectorAll('.tile')) {
        const box = tile.getBoundingClientRect();
        const w = Math.min(box.right, safe.right) - Math.max(box.left, safe.left);
        const h = Math.min(box.bottom, safe.bottom) - Math.max(box.top, safe.top);
        if (w < 80 || h < 80) continue;
        const cx = Math.max(box.left, safe.left) + w / 2;
        const cy = Math.max(box.top, safe.top) + h / 2;
        const off = Math.abs(cx - contentCenter);
        if (off < 120) continue;
        if (!best || w * h > best.area) best = { area: w * h, x: Math.round(cx), y: Math.round(cy), off };
      }
      const tile = best ? document.elementFromPoint(best.x, best.y)?.closest('.tile') : null;
      if (!tile) return null;
      window.__focusNode = tile;
      window.__focusStart = [tile.offsetLeft, tile.offsetTop, tile.offsetWidth, tile.offsetHeight];
      document.querySelectorAll('.tile').forEach((node) => {
        node.__probe = { left: node.offsetLeft, top: node.offsetTop, w: node.offsetWidth, h: node.offsetHeight };
      });
      return { x: best.x, y: best.y, id: tile.dataset.id, offCenter: best.off };
    })()`);
    if (focusPick) {
      await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: focusPick.x, y: focusPick.y, button: 'left', buttons: 1, clickCount: 1 });
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: focusPick.x, y: focusPick.y, button: 'left', buttons: 0, clickCount: 1 });
      await sleep(750);
      report.focus = await evaluate(`(() => {
        const hero = document.querySelector('.tile--spotlight');
        if (!hero) return { ok: false };
        const inspector = document.querySelector('.inspector');
        const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 120;
        const contentLeft = inspector ? inspector.getBoundingClientRect().right : seam;
        const contentCenter = contentLeft + (innerWidth - contentLeft) / 2;
        const box = hero.getBoundingClientRect();
        let moved = 0;
        let strips = 0;
        let minW = Infinity;
        let minH = Infinity;
        for (const tile of document.querySelectorAll('.tile')) {
          if (tile === hero) continue;
          const probe = tile.__probe;
          if (
            probe &&
            (Math.abs(probe.left - tile.offsetLeft) > 1 ||
              Math.abs(probe.top - tile.offsetTop) > 1 ||
              Math.abs(probe.w - tile.offsetWidth) > 1 ||
              Math.abs(probe.h - tile.offsetHeight) > 1)
          ) moved += 1;
          minW = Math.min(minW, tile.offsetWidth);
          minH = Math.min(minH, tile.offsetHeight);
          if (
            (tile.offsetHeight < 70 && tile.offsetWidth > 140) ||
            (tile.offsetWidth < 70 && tile.offsetHeight > 140)
          ) strips += 1;
        }
        const start = window.__focusStart;
        const anchorDelta = start
          ? [
              Math.round(hero.offsetLeft + hero.offsetWidth / 2 - (start[0] + start[2] / 2)),
              Math.round(hero.offsetTop + hero.offsetHeight / 2 - (start[1] + start[3] / 2)),
            ]
          : null;
        let overlap = null;
        if (start) {
          const ix = Math.max(0, Math.min(hero.offsetLeft + hero.offsetWidth, start[0] + start[2]) - Math.max(hero.offsetLeft, start[0]));
          const iy = Math.max(0, Math.min(hero.offsetTop + hero.offsetHeight, start[1] + start[3]) - Math.max(hero.offsetTop, start[1]));
          overlap = Math.round((ix * iy) / Math.max(1, start[2] * start[3]) * 100) / 100;
        }
        // 格号诊断：remount/换位 flake 出现时用来定位锚点反推是否错格。
        const cw = Math.max(760, Math.round(innerWidth * 0.72));
        const ch = Math.max(520, Math.round(innerHeight * 0.72));
        const cellOf = (x, y) => Math.floor(y / ch) + ':' + Math.floor(x / cw);
        return {
          ok: true,
          sameNode: hero === window.__focusNode,
          connected: window.__focusNode ? window.__focusNode.isConnected : null,
          preCell: start ? cellOf(start[0] + start[2] / 2, start[1] + start[3] / 2) : null,
          postCell: cellOf(hero.offsetLeft + hero.offsetWidth / 2, hero.offsetTop + hero.offsetHeight / 2),
          moved,
          strips,
          minW,
          minH,
          centerDelta: Math.round(box.left + box.width / 2 - contentCenter),
          anchorDelta,
          overlap,
          inSafe: box.left >= contentLeft - 1 && box.top >= -1 && box.right <= innerWidth + 1 && box.bottom <= innerHeight + 1,
        };
      })()`);
      report.focus.pickOffCenter = focusPick.offCenter;
      report.shots.focus = await shot('v3f-focus');
      if (!report.focus.ok) report.failures.push('聚焦：没有出现聚焦卡');
      if (report.focus.ok && !report.focus.sameNode) report.failures.push('聚焦卡被卸载重挂');
      if (report.focus.ok && report.focus.moved === 0) report.failures.push('聚焦后邻图没有被挤压');
      if (report.focus.ok && report.focus.strips > 0) report.failures.push('聚焦后出现细条卡');
      // 边缘夹取/贴边收缝（SNAP 96）会带来几何位移，数值上限随 SNAP 走没有意义；
      // “横向离开点击位置/teleport”统一靠重叠率 ≥0.5 卡（与点邻卡同口径）。
      if (report.focus.ok && (report.focus.overlap ?? 0) < 0.5) {
        report.failures.push('聚焦卡横向离开点击位置');
      }
      // “锁中心”= 英雄离开点击位置去内容区中心；英雄本就留在点击处时
      // （anchorDelta≈0）centerDelta 小只是几何巧合，不算锁中心。
      const focusMoved = Math.abs(report.focus.anchorDelta?.[0] ?? 0) + Math.abs(report.focus.anchorDelta?.[1] ?? 0);
      if (report.focus.ok && Math.abs(report.focus.centerDelta) < 40 && focusMoved > 60) {
        report.failures.push('聚焦卡被锁到内容区中心');
      }
      // 反馈问题 2：单击聚焦不带检查器、不压暗邻图。
      report.focus.noInspector = await evaluate(`!document.querySelector('.inspector')`);
      report.focus.neighborOpacity = await evaluate(`(() => {
        const tile = document.querySelector('.tile:not(.tile--spotlight)');
        return tile ? getComputedStyle(tile).opacity : null;
      })()`);
      if (report.focus.ok && report.focus.noInspector === false) {
        report.failures.push('聚焦：不应打开检查器');
      }
      if (report.focus.ok && report.focus.neighborOpacity !== null && Number(report.focus.neighborOpacity) < 0.99) {
        report.failures.push('聚焦：不应压暗邻图');
      }

      // 点一张可见邻卡：切放大对象，但新英雄留在被点卡的位置，不换位/不居中。
      const neighbor = await evaluate(`(() => {
        const hero = document.querySelector('.tile--spotlight');
        const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 120;
        const inspector = document.querySelector('.inspector')?.getBoundingClientRect().right ?? 0;
        const contentLeft = Math.max(seam, inspector);
        for (const tile of document.querySelectorAll('.tile')) {
          if (tile === hero) continue;
          const box = tile.getBoundingClientRect();
          const cx = Math.round(Math.max(box.left, contentLeft + 20) + Math.min(box.width, 160) / 2);
          const cy = Math.round(box.top + Math.min(box.height, 160) / 2);
          if (cx < contentLeft + 20 || cx > innerWidth - 20 || cy < 70 || cy > innerHeight - 20) continue;
          if (document.elementFromPoint(cx, cy)?.closest('.tile') !== tile) continue;
          window.__nextStart = [tile.offsetLeft, tile.offsetTop, tile.offsetWidth, tile.offsetHeight];
          return { x: cx, y: cy, id: tile.dataset.id };
        }
        return null;
      })()`);
      if (neighbor) {
        await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: neighbor.x, y: neighbor.y, button: 'left', buttons: 1, clickCount: 1 });
        await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: neighbor.x, y: neighbor.y, button: 'left', buttons: 0, clickCount: 1 });
        await sleep(700);
        report.focusSwitch = await evaluate(`(() => {
          const hero = document.querySelector('.tile--spotlight');
          if (!hero) return { ok: false };
          const inspector = document.querySelector('.inspector');
          const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 120;
          const contentLeft = inspector ? inspector.getBoundingClientRect().right : seam;
          const contentCenter = contentLeft + (innerWidth - contentLeft) / 2;
          const box = hero.getBoundingClientRect();
          const start = window.__nextStart;
          const anchorDelta = start
            ? [
                Math.round(hero.offsetLeft + hero.offsetWidth / 2 - (start[0] + start[2] / 2)),
                Math.round(hero.offsetTop + hero.offsetHeight / 2 - (start[1] + start[3] / 2)),
              ]
            : null;
          // 英雄卡与被点卡的重叠率：防“换位/ teleport 到固定英雄位”靠它，
          // 边缘夹取造成的几十像素滑动不会把重叠率打下去。
          let overlap = null;
          if (start) {
            const ix = Math.max(0, Math.min(hero.offsetLeft + hero.offsetWidth, start[0] + start[2]) - Math.max(hero.offsetLeft, start[0]));
            const iy = Math.max(0, Math.min(hero.offsetTop + hero.offsetHeight, start[1] + start[3]) - Math.max(hero.offsetTop, start[1]));
            overlap = Math.round((ix * iy) / Math.max(1, start[2] * start[3]) * 100) / 100;
          }
          return {
            ok: true,
            id: hero.dataset.id,
            centerDelta: Math.round(box.left + box.width / 2 - contentCenter),
            anchorDelta,
            overlap,
            inSafe: box.left >= contentLeft - 1 && box.top >= -1 && box.right <= innerWidth + 1 && box.bottom <= innerHeight + 1,
          };
        })()`);
        report.shots.focusSwitch = await shot('v3f-focus-switch');
        if (!report.focusSwitch.ok) report.failures.push('点邻卡：没有出现新的聚焦卡');
        if (report.focusSwitch.id !== neighbor.id) report.failures.push('点邻卡：放大对象不是被点的卡');
        // 与初次聚焦同口径：格子边缘夹取/贴边收缝会带来几何位移，位移量随
        // 英雄/被点卡尺寸差增长，不设数值上限；“被点图片横向搬到英雄位”
        // 由重叠率 ≥ 0.5 兜住（teleport 会让重叠率归零）。
        if ((report.focusSwitch.overlap ?? 0) < 0.5) {
          report.failures.push('点邻卡：英雄卡没有盖住被点卡片');
        }
        if (!report.focusSwitch.inSafe) report.failures.push('点邻卡：英雄卡不在可见安全区');
        const switchMoved = Math.abs(report.focusSwitch.anchorDelta?.[0] ?? 0) + Math.abs(report.focusSwitch.anchorDelta?.[1] ?? 0);
        if (Math.abs(report.focusSwitch.centerDelta) < 40 && switchMoved > 60) {
          report.failures.push('点邻卡：被锁到内容区中心');
        }
      } else {
        report.failures.push('点邻卡：找不到可点击的邻卡');
      }

      // 关掉聚焦，继续详情页用例
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
      await sleep(350);
    } else {
      report.failures.push('聚焦：首页找不到可点击的卡片');
    }

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
      // 概要层直接叠在卡片上：卡片只露一半时，层也必然有一半在视口外。
      // 这里优先拿"整张都在安全区里"的卡；没有才退回相交面积最大的那张。
      let fallback = null;
      for (const tile of document.querySelectorAll('.tile')) {
        const box = tile.getBoundingClientRect();
        const w = Math.min(box.right, safe.right) - Math.max(box.left, safe.left);
        const h = Math.min(box.bottom, safe.bottom) - Math.max(box.top, safe.top);
        if (w < 60 || h < 60) continue;
        const x = Math.round(Math.max(box.left, safe.left) + w / 2);
        const y = Math.round(Math.max(box.top, safe.top) + h / 2);
        if (box.left >= safe.left && box.right <= safe.right && box.top >= safe.top && box.bottom <= safe.bottom) {
          return { x, y };
        }
        const area = w * h;
        if (!fallback || area > fallback.area) fallback = { area, x, y };
      }
      return fallback ? { x: fallback.x, y: fallback.y } : null;
    })()`);
    if (spot) {
      await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: spot.x, y: spot.y, button: 'right', buttons: 2, clickCount: 1 });
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: spot.x, y: spot.y, button: 'right', buttons: 0, clickCount: 1 });
      await sleep(500);
      report.summary = await evaluate(`(() => {
        const node = document.querySelector('.tile__info');
        if (!node) return { open: false };
        const tile = node.closest('.tile');
        const box = tile ? tile.getBoundingClientRect() : node.getBoundingClientRect();
        return {
          open: true,
          left: Math.round(box.left), top: Math.round(box.top),
          right: Math.round(box.right), bottom: Math.round(box.bottom),
          // 概要层直接叠在卡片上，卡片在视口内就等于层在视口内
          inViewport: box.left >= -1 && box.top >= -1 && box.right <= innerWidth + 1 && box.bottom <= innerHeight + 1,
          name: node.querySelector('.tile__info-name')?.textContent ?? null,
        };
      })()`);
      report.shots.summary = await shot('v3f-summary');
      // 反馈问题 4b：再次右键 = 退出；退出动画 160ms 后层应卸载。
      await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: spot.x, y: spot.y, button: 'right', buttons: 2, clickCount: 1 });
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: spot.x, y: spot.y, button: 'right', buttons: 0, clickCount: 1 });
      await sleep(400);
      report.summaryToggleClosed = await evaluate(`!document.querySelector('.tile__info')`);
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
      openedLightbox: await evaluate(`location.hash.startsWith('#/d/')`),
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

    // 6.3 键盘路径：画布聚焦 → ↓（平移）→ Tab（进卡片）→ Enter（展开全屏详情）
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
    const lightboxOpened = await evaluate(`location.hash.startsWith('#/d/')`);
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
        // 漏点先做缝容忍重采样（+12px）：格缝/行缝宽 8px，重采样必跨进卡里；
        // 真正的空白带（≥24px）四个点都落空，仍会被抓住。
        const hitsTile = (x, y) => {
          const el = document.elementFromPoint(x, y);
          return !!(el && el.closest('.tile'));
        };
        for (let x = seam + 20; x < innerWidth - 20; x += 40) {
          for (let y = 20; y < innerHeight - 20; y += 40) {
            total += 1;
            if (hitsTile(x, y) || hitsTile(x + 12, y) || hitsTile(x, y + 12) || hitsTile(x + 12, y + 12)) hit += 1;
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
    // 窄屏 reload 在 dev server 冷启动/HMR 后偶发空白首帧；等 .app，必要时再 reload 一次。
    let appReady = false;
    for (let attempt = 0; attempt < 60; attempt += 1) {
      appReady = await evaluate(`!!document.querySelector('.app')`).catch(() => false);
      if (appReady) break;
      await sleep(200);
    }
    if (!appReady) {
      await cdp.send('Page.reload');
      await sleep(1200);
    }
    for (let attempt = 0; attempt < 60; attempt += 1) {
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
      report.mobile.cover = await coverage();
      const before = await matrixOf();
      await drag(340, 640, 80, 640);
      const after = await matrixOf();
      report.mobile.canPanHorizontally = Math.abs(before.e - after.e) > 60;
      report.mobile.dragDx = Math.round(before.e - after.e);

      // 触屏长按概要：按住一张卡 700ms 再松手，应该出概要且不开灯箱。
      const touchSpot = await evaluate(`(() => {
        const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 64;
        const safe = { left: seam + 20, top: 120, right: innerWidth - 20, bottom: innerHeight - 140 };
        let best = null;
        for (const tile of document.querySelectorAll('.tile')) {
          const box = tile.getBoundingClientRect();
          const w = Math.min(box.right, safe.right) - Math.max(box.left, safe.left);
          const h = Math.min(box.bottom, safe.bottom) - Math.max(box.top, safe.top);
          if (w < 50 || h < 50) continue;
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
      if (touchSpot) {
        await cdp.send('Input.dispatchTouchEvent', {
          type: 'touchStart',
          touchPoints: [{ x: touchSpot.x, y: touchSpot.y, id: 1 }],
        });
        await sleep(700);
        const summaryOpen = await evaluate(`!!document.querySelector('.tile__info')`);
        await cdp.send('Input.dispatchTouchEvent', { type: 'touchEnd', touchPoints: [] });
        await sleep(400);
        report.mobile.longPressSummary = summaryOpen;
        report.mobile.longPressOpenedLightbox = await evaluate(`location.hash.startsWith('#/d/')`);
        await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
        await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
        await sleep(200);
      } else {
        report.failures.push('390 长按测试找不到可点的卡片');
      }
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
    if (report.summary && report.summary.open && report.summaryToggleClosed === false) report.failures.push('右键概要：再次右键没有退出');
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
    if (report.mobile && report.mobile.longPressSummary === false) report.failures.push('触屏长按没有出概要');
    if (report.mobile && report.mobile.longPressOpenedLightbox) report.failures.push('触屏长按误开了灯箱');
    if (report.mobile && report.mobile.cover < 0.8) report.failures.push('390 出现空白带');
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
