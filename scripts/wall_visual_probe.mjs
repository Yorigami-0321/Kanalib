#!/usr/bin/env node
/** 一次性视觉探针：等图加载完再截图，顺带 dump 焦点/详情的布局指标。 */
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

class Cdp {
  constructor(url) { this.url = url; this.nextId = 1; this.pending = new Map(); this.listeners = []; }
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
  on(listener) { this.listeners.push(listener); }
  close() { try { this.ws.close(); } catch { /* closed */ } }
}

const base = process.argv[2] ?? 'http://127.0.0.1:5274/';
const outDir = resolve('web/screenshots/probe');
mkdirSync(outDir, { recursive: true });
const chrome = CANDIDATES.find((path) => existsSync(path));
const profile = mkdtempSync(join(tmpdir(), 'kanalib-probe-'));
const port = 9700 + Math.floor(Math.random() * 200);

const child = spawn(chrome, [
  '--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
  '--no-first-run', '--no-default-browser-check', '--hide-scrollbars', '--window-size=1440,900', 'about:blank',
], { stdio: 'ignore' });

const report = { shots: {}, consoleErrors: [] };
let cdp;
try {
  for (let i = 0; i < 80; i += 1) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (res.ok) break;
    } catch { /* wait */ }
    await sleep(120);
  }
  const target = await (await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, { method: 'PUT' })).json();
  cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.connect();
  cdp.on((message) => {
    if (message.method === 'Runtime.exceptionThrown') report.consoleErrors.push('exception: ' + (message.params.exceptionDetails?.text ?? 'unknown'));
    if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') report.consoleErrors.push(message.params.args.map((a) => a.value ?? a.description ?? '').join(' '));
  });
  await cdp.send('Page.enable');
  await cdp.send('Runtime.enable');
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  const evaluate = async (expression) => {
    const result = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) {
      throw new Error(
        result.exceptionDetails.exception?.description ?? result.exceptionDetails.text,
      );
    }
    return result.result.value;
  };
  const shot = async (name) => {
    const { data } = await cdp.send('Page.captureScreenshot', { format: 'png', fromSurface: true });
    const path = join(outDir, name + '.png');
    writeFileSync(path, Buffer.from(data, 'base64'));
    report.shots[name] = path;
  };
  const settle = async (limit = 12000) => {
    const started = Date.now();
    while (Date.now() - started < limit) {
      const state = await evaluate(`(() => ({ tiles: document.querySelectorAll('.tile').length, pending: [...document.querySelectorAll('.tile img')].filter((img) => !img.complete || !img.classList.contains('is-loaded')).length }))()`).catch(() => null);
      if (state && state.tiles > 0 && state.pending === 0) return { ...state, waited: Date.now() - started };
      await sleep(180);
    }
    return await evaluate(`(() => ({ tiles: document.querySelectorAll('.tile').length, pending: [...document.querySelectorAll('.tile img')].filter((img) => !img.complete).length, timeout: true }))()`);
  };

  await cdp.send('Page.navigate', { url: base + '#/' });
  report.homeSettle = await settle();
  await sleep(400);
  report.home = await evaluate(`(() => {
    const tiles = [...document.querySelectorAll('.tile')].map((t) => t.getBoundingClientRect());
    const world = document.querySelector('.world')?.getBoundingClientRect();
    return {
      tiles: tiles.length,
      world: world ? { x: Math.round(world.x), y: Math.round(world.y), w: Math.round(world.width), h: Math.round(world.height) } : null,
      accent: getComputedStyle(document.documentElement).getPropertyValue('--accent').trim(),
      avgTile: tiles.length ? [Math.round(tiles.reduce((s, b) => s + b.width, 0) / tiles.length), Math.round(tiles.reduce((s, b) => s + b.height, 0) / tiles.length)] : null,
      background: getComputedStyle(document.body).backgroundColor,
    };
  })()`);
  await shot('home');

  // Tab 走查：稳定 DOM 顺序下，焦点不应落到视口外的卡片。
  await evaluate(`document.querySelector('.canvas')?.focus()`);
  report.tabWalk = [];
  for (let index = 0; index < 6; index += 1) {
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9 });
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Tab', code: 'Tab', windowsVirtualKeyCode: 9 });
    await sleep(450);
    report.tabWalk.push(
      await evaluate(`(() => {
        const node = document.activeElement;
        const box = node?.getBoundingClientRect();
        return {
          tag: node?.tagName ?? null,
          cls: typeof node?.className === 'string' ? node.className : null,
          id: node?.dataset?.id ?? null,
          inView: box ? box.right > 0 && box.left < innerWidth && box.bottom > 0 && box.top < innerHeight : null,
        };
      })()`),
    );
  }

  // 单色预览（N / 侧栏按钮）：图片应变灰，UI 强调色保留。
  await evaluate(`(() => {
    const button = [...document.querySelectorAll('.seam__tab')].find((node) => (node.getAttribute('title') ?? '').includes('单色预览'));
    button?.click();
    return !!button;
  })()`);
  await sleep(500);
  report.mono = await evaluate(`(() => {
    const img = document.querySelector('.tile__img.is-loaded');
    return {
      appClass: document.querySelector('.app')?.className ?? null,
      filter: img ? getComputedStyle(img).filter : null,
      pressed: [...document.querySelectorAll('.seam__tab')].find((node) => (node.getAttribute('title') ?? '').includes('恢复彩色'))?.getAttribute('aria-pressed') ?? null,
      accent: getComputedStyle(document.documentElement).getPropertyValue('--accent').trim(),
    };
  })()`);
  await shot('mono');
  await evaluate(`[...document.querySelectorAll('.seam__tab')].find((node) => (node.getAttribute('title') ?? '').includes('恢复彩色'))?.click()`);
  await sleep(300);

  // 单击聚焦：原位放大、不重排邻卡、不做强制居中。
  const beforeFocus = await evaluate(`(() => {
    const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 120;
    const safe = { left: seam + 340, top: 90, right: innerWidth - 40, bottom: innerHeight - 120 };
    let best = null;
    for (const tile of document.querySelectorAll('.tile')) {
      const box = tile.getBoundingClientRect();
      const w = Math.min(box.right, safe.right) - Math.max(box.left, safe.left);
      const h = Math.min(box.bottom, safe.bottom) - Math.max(box.top, safe.top);
      if (w < 80 || h < 80) continue;
      const cx = Math.max(box.left, safe.left) + w / 2;
      const cy = Math.max(box.top, safe.top) + h / 2;
      // 偏爱离内容中心更远的卡，用来验证“不会把点击的卡锁到中心”。
      const off = Math.abs(cx - ((innerWidth + safe.left) / 2));
      const score = w * h * (1 + off / innerWidth);
      if (!best || score > best.score) best = { score, x: Math.round(cx), y: Math.round(cy), off: Math.round(off) };
    }
    const tile = best ? document.elementFromPoint(best.x, best.y)?.closest('.tile') : null;
    if (tile) {
      window.__focusNode = tile;
      const box = tile.getBoundingClientRect();
      window.__focusStart = [box.left, box.top, box.width, box.height];
      window.__focusStartOffset = [tile.offsetLeft, tile.offsetTop, tile.offsetWidth, tile.offsetHeight];
    }
    document.querySelectorAll('.tile').forEach((node) => {
      node.__probe = { left: node.offsetLeft, top: node.offsetTop, w: node.offsetWidth, h: node.offsetHeight };
    });
    const worldNode = document.querySelector('.world');
    const world = worldNode
      ? new DOMMatrixReadOnly(getComputedStyle(worldNode).transform)
      : { e: 0, f: 0 };
    return {
      spot: best,
      cam: [world.e, world.f],
      world: !!worldNode,
      tiles: document.querySelectorAll('.tile').length,
      focusId: tile?.dataset.id ?? null,
    };
  })()`);
  report.beforeFocus = beforeFocus;
  if (beforeFocus.spot) {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: beforeFocus.spot.x, y: beforeFocus.spot.y, button: 'left', buttons: 1, clickCount: 1 });
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: beforeFocus.spot.x, y: beforeFocus.spot.y, button: 'left', buttons: 0, clickCount: 1 });
    await sleep(150);
    report.focusMid = await evaluate(`(() => {
      const hero = document.querySelector('.tile--spotlight');
      const box = hero?.getBoundingClientRect();
      return box ? { rect: [Math.round(box.left), Math.round(box.top), Math.round(box.width), Math.round(box.height)], sameNode: hero === window.__focusNode } : null;
    })()`);
    await shot('focus-grow');
    await sleep(600);
    report.spotlight = await evaluate(`(() => {
      const hero = document.querySelector('.tile--spotlight');
      const inspector = document.querySelector('.inspector');
      if (!hero) return { ok: false };
      const box = hero.getBoundingClientRect();
      const world = new DOMMatrixReadOnly(getComputedStyle(document.querySelector('.world')).transform);
      let moved = 0;
      let lost = 0;
      let strips = 0;
      let minW = Infinity;
      let minH = Infinity;
      for (const tile of document.querySelectorAll('.tile')) {
        if (tile === hero) continue;
        const probe = tile.__probe;
        if (!probe) { lost += 1; continue; }
        if (
          Math.abs(probe.left - tile.offsetLeft) > 1 ||
          Math.abs(probe.top - tile.offsetTop) > 1 ||
          Math.abs(probe.w - tile.offsetWidth) > 1 ||
          Math.abs(probe.h - tile.offsetHeight) > 1
        ) moved += 1;
        minW = Math.min(minW, tile.offsetWidth);
        minH = Math.min(minH, tile.offsetHeight);
        if ((tile.offsetHeight < 80 && tile.offsetWidth > 160) || (tile.offsetWidth < 80 && tile.offsetHeight > 160)) strips += 1;
      }
      const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 120;
      const contentLeft = inspector ? inspector.getBoundingClientRect().right : seam;
      const contentCenter = contentLeft + (innerWidth - contentLeft) / 2;
      return {
        ok: true,
        rect: [Math.round(box.left), Math.round(box.top), Math.round(box.width), Math.round(box.height)],
        start: (window.__focusStart ?? []).map((value) => Math.round(value)),
        inspector: inspector ? [Math.round(inspector.getBoundingClientRect().left), Math.round(inspector.getBoundingClientRect().width)] : null,
        cam: [world.e, world.f],
        camDelta: [Math.round(world.e - ${beforeFocus.cam[0]}), Math.round(world.f - ${beforeFocus.cam[1]})],
        centerDelta: Math.round(box.left + box.width / 2 - contentCenter),
        moved,
        lost,
        strips,
        minW,
        minH,
        inSafe: box.left >= contentLeft - 1 && box.top >= -1 && box.right <= innerWidth + 1 && box.bottom <= innerHeight + 1,
        sameNode: hero === window.__focusNode,
        anchorDelta: (() => {
          const start = window.__focusStartOffset;
          if (!start) return null;
          return [
            Math.round(hero.offsetLeft + hero.offsetWidth / 2 - (start[0] + start[2] / 2)),
            Math.round(hero.offsetTop + hero.offsetHeight / 2 - (start[1] + start[3] / 2)),
          ];
        })(),
      };
    })()`);
    await shot('spotlight');

    // 点一张可见邻卡：不应把画面锁到中心，也不应重排其他卡。
    const neighbor = await evaluate(`(() => {
      const hero = document.querySelector('.tile--spotlight');
      const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 120;
      const inspector = document.querySelector('.inspector')?.getBoundingClientRect().right ?? 0;
      for (const tile of document.querySelectorAll('.tile')) {
        if (tile === hero) continue;
        const box = tile.getBoundingClientRect();
        const cx = Math.round(Math.max(box.left, Math.max(seam, inspector) + 20) + Math.min(box.width, 160) / 2);
        const cy = Math.round(box.top + Math.min(box.height, 160) / 2);
        if (cx < Math.max(seam, inspector) + 20 || cx > innerWidth - 20 || cy < 70 || cy > innerHeight - 20) continue;
        if (document.elementFromPoint(cx, cy)?.closest('.tile') !== tile) continue;
        window.__nextStartOffset = [tile.offsetLeft, tile.offsetTop, tile.offsetWidth, tile.offsetHeight];
        return { x: cx, y: cy, id: tile.dataset.id };
      }
      return null;
    })()`);
    if (neighbor) {
      const camBefore = await evaluate(`(() => { const m = new DOMMatrixReadOnly(getComputedStyle(document.querySelector('.world')).transform); return [m.e, m.f]; })()`);
      await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: neighbor.x, y: neighbor.y, button: 'left', buttons: 1, clickCount: 1 });
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: neighbor.x, y: neighbor.y, button: 'left', buttons: 0, clickCount: 1 });
      await sleep(650);
      report.focusSwitch = await evaluate(`(() => {
        const hero = document.querySelector('.tile--spotlight');
        const box = hero?.getBoundingClientRect();
        const m = new DOMMatrixReadOnly(getComputedStyle(document.querySelector('.world')).transform);
        const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 120;
        const inspector = document.querySelector('.inspector')?.getBoundingClientRect().right ?? 0;
        const contentLeft = Math.max(seam, inspector);
        const contentCenter = contentLeft + (innerWidth - contentLeft) / 2;
        return {
          id: hero?.dataset.id ?? null,
          rect: box ? [Math.round(box.left), Math.round(box.top), Math.round(box.width), Math.round(box.height)] : null,
          camDelta: [Math.round(m.e - ${camBefore[0]}), Math.round(m.f - ${camBefore[1]})],
          centerDelta: box ? Math.round(box.left + box.width / 2 - contentCenter) : null,
          inSafe: box ? box.left >= contentLeft - 1 && box.right <= innerWidth + 1 && box.top >= -1 && box.bottom <= innerHeight + 1 : null,
          anchorDelta: (() => {
            const start = window.__nextStartOffset;
            if (!hero || !start) return null;
            return [
              Math.round(hero.offsetLeft + hero.offsetWidth / 2 - (start[0] + start[2] / 2)),
              Math.round(hero.offsetTop + hero.offsetHeight / 2 - (start[1] + start[3] / 2)),
            ];
          })(),
        };
      })()`);
      await shot('focus-switch');
    }
  }

    // 直接进详情（双击聚焦后的坐标会随布局重排，点原坐标不可靠）
    const focusedId = await evaluate(`document.querySelector('.tile--spotlight')?.dataset.id ?? null`);
    if (focusedId) {
      await cdp.send('Page.navigate', { url: base + '#/d/' + focusedId });
      await sleep(2200);
      report.detail = await evaluate(`(() => {
        const img = document.querySelector('.view__track > .view__pane:nth-child(2) .view__img');
        const inspector = document.querySelector('.inspector');
        const ambient = document.querySelector('.view__ambient');
        const box = img?.getBoundingClientRect();
        return {
          ok: !!img,
          hash: location.hash,
          img: box ? [Math.round(box.left), Math.round(box.top), Math.round(box.width), Math.round(box.height)] : null,
          src: img?.currentSrc?.split('/').slice(-2).join('/'),
          loaded: img?.classList.contains('is-loaded'),
          ambient: ambient ? getComputedStyle(ambient).backgroundImage.slice(0, 40) : null,
          inspector: inspector ? Math.round(inspector.getBoundingClientRect().width) : 0,
        };
      })()`);
      await shot('detail');
      await sleep(600);
      await shot('detail-settled');

      // 留白视图（M）：图片收进画框；收起信息后出现图注。
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'm', code: 'KeyM', windowsVirtualKeyCode: 77 });
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'm', code: 'KeyM', windowsVirtualKeyCode: 77 });
      await sleep(600);
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'i', code: 'KeyI', windowsVirtualKeyCode: 73 });
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'i', code: 'KeyI', windowsVirtualKeyCode: 73 });
      await sleep(600);
      report.matte = await evaluate(`(() => {
        const view = document.querySelector('.view--detail');
        const img = document.querySelector('.view__track > .view__pane:nth-child(2) .view__img') ?? document.querySelector('.view__img');
        const box = img?.getBoundingClientRect();
        const caption = document.querySelector('.view__caption');
        return {
          classOn: view?.classList.contains('view--matte') ?? null,
          rect: box ? [Math.round(box.left), Math.round(box.top), Math.round(box.width), Math.round(box.height)] : null,
          caption: caption?.textContent?.trim() ?? null,
          captionBox: caption ? (() => { const b = caption.getBoundingClientRect(); return [Math.round(b.left), Math.round(b.top), Math.round(b.width)]; })() : null,
          inspector: !!document.querySelector('.inspector'),
        };
      })()`);
      await shot('matte');
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'i', code: 'KeyI', windowsVirtualKeyCode: 73 });
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'i', code: 'KeyI', windowsVirtualKeyCode: 73 });
      await sleep(400);
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'm', code: 'KeyM', windowsVirtualKeyCode: 77 });
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'm', code: 'KeyM', windowsVirtualKeyCode: 77 });
      await sleep(300);

      // ←/→ 切换：动画中途 + 落位后各拍一张，验证滑轨与预加载
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'ArrowRight', code: 'ArrowRight', windowsVirtualKeyCode: 39 });
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'ArrowRight', code: 'ArrowRight', windowsVirtualKeyCode: 39 });
      await sleep(220);
      await shot('detail-slide');
      await sleep(900);
      await shot('detail-next');
      report.slide = await evaluate(`(() => {
        const panes = [...document.querySelectorAll('.view__track > .view__pane')].map((pane) => {
          const img = pane.querySelector('.view__img');
          return img ? { loaded: img.classList.contains('is-loaded'), src: img.currentSrc.split('/').slice(-2).join('/') } : null;
        });
        return {
          hash: location.hash,
          anim: document.querySelector('.view__track')?.className ?? null,
          panes,
        };
      })()`);

      // 自动放映：点侧栏 ▶ 后 4.2s 应换图，进度线存在；暂停后不再换图。
      const beforePlay = await evaluate(`location.hash`);
      const clicked = await evaluate(`(() => {
        const button = [...document.querySelectorAll('.seam__tab')].find((node) => (node.getAttribute('title') ?? '').includes('自动放映'));
        button?.click();
        return !!button;
      })()`);
      await sleep(900);
      report.play = await evaluate(`(() => ({
        clicked: ${clicked},
        bar: !!document.querySelector('.view__playbar'),
        pressed: [...document.querySelectorAll('.seam__tab')].find((node) => (node.getAttribute('title') ?? '').includes('暂停放映'))?.getAttribute('aria-pressed') ?? null,
      }))()`);
      await shot('playing');
      await sleep(3800);
      report.play.afterHash = await evaluate(`location.hash`);
      report.play.advanced = report.play.afterHash !== beforePlay;
      await evaluate(`[...document.querySelectorAll('.seam__tab')].find((node) => (node.getAttribute('title') ?? '').includes('暂停放映'))?.click()`);
      await sleep(300);
      report.play.barAfterPause = await evaluate(`!!document.querySelector('.view__playbar')`);

      // 复制链接：授权剪贴板后点按钮，读回文本应指向当前图片。
      await cdp.send('Browser.grantPermissions', {
        origin: new URL(base).origin,
        permissions: ['clipboardReadWrite', 'clipboardSanitizedWrite'],
      });
      await evaluate(`[...document.querySelectorAll('.inspector__link--button')].find((node) => node.textContent?.includes('复制链接'))?.click()`);
      await sleep(350);
      report.copy = await evaluate(`(async () => ({
        label: [...document.querySelectorAll('.inspector__link--button')].map((node) => node.textContent).join(' | '),
        text: await navigator.clipboard.readText().catch(() => ''),
        hash: location.hash,
      }))()`);
    }
    // 详情返回墙
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
    await sleep(900);
    await shot('back-to-wall');

  // 标签总览
  await cdp.send('Page.navigate', { url: base + '#/tags' });
  await sleep(4500);
  await shot('tags');
  // 单标签
  await cdp.send('Page.navigate', { url: base + '#/t/1girl' });
  await sleep(2000);
  await settle(8000);
  await shot('tag');

  // 二级墙聚焦：英雄卡中心应落在「缝 + 检查器之后的内容区」中心，而不是被垂直余量推偏。
  const tagSpot = await evaluate(`(() => {
    const tiles = [...document.querySelectorAll('.tile')];
    const tile = tiles.sort((a, b) => b.getBoundingClientRect().width - a.getBoundingClientRect().width)[0];
    if (!tile) return null;
    const box = tile.getBoundingClientRect();
    return { x: Math.round(box.left + box.width / 2), y: Math.round(box.top + box.height / 2), id: tile.dataset.id };
  })()`);
  if (tagSpot) {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: tagSpot.x, y: tagSpot.y, button: 'left', buttons: 1, clickCount: 1 });
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: tagSpot.x, y: tagSpot.y, button: 'left', buttons: 0, clickCount: 1 });
    await sleep(800);
    report.tagFocus = await evaluate(`(() => {
      const hero = document.querySelector('.tile--spotlight');
      const box = hero?.getBoundingClientRect();
      const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 120;
      const inspector = document.querySelector('.inspector')?.getBoundingClientRect().width ?? 0;
      const expectedX = seam + inspector + (innerWidth - seam - inspector) / 2;
      const expectedY = innerHeight / 2;
      return {
        ok: !!hero,
        rect: box ? [Math.round(box.left), Math.round(box.top), Math.round(box.width), Math.round(box.height)] : null,
        center: box ? [Math.round(box.left + box.width / 2), Math.round(box.top + box.height / 2)] : null,
        expected: [Math.round(expectedX), Math.round(expectedY)],
        delta: box ? [Math.round(box.left + box.width / 2 - expectedX), Math.round(box.top + box.height / 2 - expectedY)] : null,
      };
    })()`);
    await shot('tag-focus');
  }

  // 移动端首页
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
  await cdp.send('Page.navigate', { url: base + '#/' });
  await sleep(2500);
  await settle(8000);
  report.mobile = await evaluate(`(() => {
    const tiles = [...document.querySelectorAll('.tile')];
    const first = tiles[0];
    const title = first?.querySelector('.tile__copy strong');
    const chip = first?.querySelector('.tile__chip');
    const box = first?.getBoundingClientRect();
    return {
      tiles: tiles.length,
      firstCard: box ? [Math.round(box.width), Math.round(box.height)] : null,
      titleFont: title ? getComputedStyle(title).fontSize : null,
      titleLines: title ? getComputedStyle(title).webkitLineClamp : null,
      chipDisplay: chip ? getComputedStyle(chip).display : null,
      chipFont: chip ? getComputedStyle(chip).fontSize : null,
      scrollW: document.documentElement.scrollWidth,
      innerW: innerWidth,
    };
  })()`);
  await shot('home-390');

  // 390 聚焦：英雄卡应在底部 sheet 上方完整可见，相机不动，邻卡不重排。
  const mobileSpot = await evaluate(`(() => {
    const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 64;
    let best = null;
    for (const tile of document.querySelectorAll('.tile')) {
      const box = tile.getBoundingClientRect();
      const w = Math.min(box.right, innerWidth - 20) - Math.max(box.left, seam + 20);
      const h = Math.min(box.bottom, innerHeight - 120) - Math.max(box.top, 80);
      if (w < 60 || h < 60) continue;
      const area = w * h;
      if (!best || area > best.area) best = { area, x: Math.round(Math.max(box.left, seam + 20) + w / 2), y: Math.round(Math.max(box.top, 80) + h / 2) };
    }
    const tile = best ? document.elementFromPoint(best.x, best.y)?.closest('.tile') : null;
    if (!tile) return null;
    const box = tile.getBoundingClientRect();
    window.__mFocus = tile;
    window.__mStart = [box.left, box.top, box.width, box.height];
    document.querySelectorAll('.tile').forEach((node) => {
      node.__mProbe = { left: node.offsetLeft, top: node.offsetTop, w: node.offsetWidth, h: node.offsetHeight };
    });
    const m = new DOMMatrixReadOnly(getComputedStyle(document.querySelector('.world')).transform);
    return { x: best.x, y: best.y, cam: [m.e, m.f] };
  })()`);
  if (mobileSpot) {
    await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: mobileSpot.x, y: mobileSpot.y, button: 'left', buttons: 1, clickCount: 1 });
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: mobileSpot.x, y: mobileSpot.y, button: 'left', buttons: 0, clickCount: 1 });
    await sleep(700);
    report.mobileFocus = await evaluate(`(() => {
      const hero = document.querySelector('.tile--spotlight');
      const box = hero?.getBoundingClientRect();
      const sheet = document.querySelector('.inspector')?.getBoundingClientRect();
      const m = new DOMMatrixReadOnly(getComputedStyle(document.querySelector('.world')).transform);
      let moved = 0, strips = 0;
      for (const tile of document.querySelectorAll('.tile')) {
        if (tile === hero) continue;
        const p = tile.__mProbe;
        if (p && (Math.abs(p.left - tile.offsetLeft) > 1 || Math.abs(p.top - tile.offsetTop) > 1 || Math.abs(p.w - tile.offsetWidth) > 1 || Math.abs(p.h - tile.offsetHeight) > 1)) moved += 1;
        if ((tile.offsetHeight < 70 && tile.offsetWidth > 140) || (tile.offsetWidth < 70 && tile.offsetHeight > 140)) strips += 1;
      }
      return {
        ok: !!hero,
        rect: box ? [Math.round(box.left), Math.round(box.top), Math.round(box.width), Math.round(box.height)] : null,
        start: (window.__mStart ?? []).map((value) => Math.round(value)),
        sheetTop: sheet ? Math.round(sheet.top) : null,
        aboveSheet: box && sheet ? box.bottom <= sheet.top + 1 : null,
        camDelta: [Math.round(m.e - ${mobileSpot.cam[0]}), Math.round(m.f - ${mobileSpot.cam[1]})],
        sameNode: hero === window.__mFocus,
        moved,
        strips,
      };
    })()`);
    await shot('mobile-focus');
  }

  writeFileSync(join(outDir, 'probe.json'), JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report, null, 2));
} finally {
  cdp?.close();
  try { child.kill(); } catch { /* ignore */ }
  if (process.platform === 'win32') spawnSync('cmd', ['/c', 'rd', '/s', '/q', profile], { stdio: 'ignore' });
  else rmSync(profile, { recursive: true, force: true });
}
