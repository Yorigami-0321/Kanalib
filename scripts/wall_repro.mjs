#!/usr/bin/env node
/**
 * 临时复现脚本：单击聚焦的英雄卡宽高比 + 右键概要遮罩覆盖。
 * 用法：node scripts/wall_repro.mjs --base http://127.0.0.1:4318/ --out web/screenshots
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
  const out = { base: 'http://127.0.0.1:4318/', out: 'web/screenshots', width: 1440, height: 900 };
  for (let i = 0; i < argv.length; i += 2) {
    if (argv[i] === '--base') out.base = argv[i + 1];
    else if (argv[i] === '--out') out.out = argv[i + 1];
    else if (argv[i] === '--width') out.width = Number(argv[i + 1]);
    else if (argv[i] === '--height') out.height = Number(argv[i + 1]);
  }
  return out;
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
    await new Promise((res, rej) => {
      this.ws.addEventListener('open', res, { once: true });
      this.ws.addEventListener('error', () => rej(new Error('CDP 连接失败')), { once: true });
    });
    this.ws.addEventListener('message', (event) => {
      const message = JSON.parse(event.data);
      if (message.id && this.pending.has(message.id)) {
        const { resolve: done, reject } = this.pending.get(message.id);
        this.pending.delete(message.id);
        if (message.error) reject(new Error(message.method + ': ' + message.error.message));
        else done(message.result);
        return;
      }
      for (const l of this.listeners) l(message);
    });
  }
  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((res, rej) => {
      this.pending.set(id, { resolve: res, reject: rej });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  close() {
    try { this.ws.close(); } catch { /* noop */ }
  }
}

async function waitForDevtools(port, timeoutMs = 12000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (r.ok) return;
    } catch { /* retry */ }
    await sleep(150);
  }
  throw new Error('Chrome 调试端口没起来');
}

function hardDelete(dir) {
  if (!dir || !existsSync(dir)) return;
  if (process.platform === 'win32') {
    spawnSync('cmd', ['/c', 'rd', '/s', '/q', dir], { stdio: 'ignore' });
    if (!existsSync(dir)) return;
  }
  try { rmSync(dir, { recursive: true, force: true }); } catch { /* noop */ }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const chrome = CANDIDATES.find(existsSync);
  if (!chrome) { console.error('no chrome'); process.exit(2); }
  const outDir = resolve(args.out);
  mkdirSync(outDir, { recursive: true });
  const profile = mkdtempSync(join(tmpdir(), 'kanalib-repro-'));
  const port = 9700 + Math.floor(Math.random() * 200);
  const child = spawn(chrome, [
    '--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
    '--no-first-run', '--no-default-browser-check', '--disable-extensions', '--hide-scrollbars',
    `--window-size=${args.width},${args.height}`, 'about:blank',
  ], { stdio: 'ignore' });

  let cdp;
  try {
    await waitForDevtools(port);
    const resp = await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, { method: 'PUT' });
    const target = await resp.json();
    cdp = new Cdp(target.webSocketDebuggerUrl);
    await cdp.connect();
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    await cdp.send('Emulation.setDeviceMetricsOverride', {
      width: args.width, height: args.height, deviceScaleFactor: 1, mobile: false,
    });

    const evaluate = async (expression) => {
      const result = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
      if (result.exceptionDetails) throw new Error(result.exceptionDetails.text + ': ' + (result.exceptionDetails.exception?.description ?? ''));
      return result.result.value;
    };
    const shot = async (name, clip) => {
      const { data } = await cdp.send('Page.captureScreenshot', { format: 'png', fromSurface: true, ...(clip ? { clip } : {}) });
      const path = join(outDir, name + '.png');
      writeFileSync(path, Buffer.from(data, 'base64'));
      console.log('shot:', path);
    };
    const mouse = (type, x, y, button = 'left') =>
      cdp.send('Input.dispatchMouseEvent', {
        type, x, y, button,
        buttons: type === 'mouseReleased' ? 0 : button === 'right' ? 2 : 1,
        clickCount: 1,
      });

    await cdp.send('Page.navigate', { url: args.base + '#/' });
    // 等卡片出现
    for (let i = 0; i < 80; i += 1) {
      try {
        const n = await evaluate(`document.querySelectorAll('.tile').length`);
        if (n > 20) break;
      } catch { /* nav */ }
      await sleep(150);
    }
    await sleep(1200);

    // 找一张竖图（naturalHeight > naturalWidth * 1.2）和一张横图；必须整张在视口内
    const targets = await evaluate(`(() => {
      const out = [];
      for (const tile of document.querySelectorAll('.tile')) {
        const box = tile.getBoundingClientRect();
        if (box.left < 10 || box.top < 10 || box.right > innerWidth - 10 || box.bottom > innerHeight - 10) continue;
        if (box.width < 80 || box.height < 80) continue;
        const hit = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2)?.closest('.tile');
        if (hit !== tile) continue;
        const img = tile.querySelector('img');
        if (!img || !img.naturalWidth) continue;
        out.push({
          id: tile.dataset.id,
          x: box.x, y: box.y, w: box.width, h: box.height,
          nw: img.naturalWidth, nh: img.naturalHeight,
          portrait: img.naturalHeight > img.naturalWidth * 1.15,
          landscape: img.naturalWidth > img.naturalHeight * 1.15,
        });
      }
      return out;
    })()`);
    const portrait = targets.find((t) => t.portrait);
    const landscape = targets.find((t) => t.landscape);
    console.log('tiles:', targets.length, 'portrait:', portrait?.id, 'landscape:', landscape?.id);

    // ---- 1) 单击竖图 → 聚焦 ----
    if (portrait) {
      await mouse('mousePressed', portrait.x + portrait.w / 2, portrait.y + portrait.h / 2, 'left');
      await mouse('mouseReleased', portrait.x + portrait.w / 2, portrait.y + portrait.h / 2, 'left');
      await sleep(900);
      const hero = await evaluate(`(() => {
        const el = document.querySelector('.tile--spotlight');
        if (!el) return null;
        const img = el.querySelector('img');
        const box = el.getBoundingClientRect();
        return { w: box.width, h: box.height, nw: img?.naturalWidth ?? 0, nh: img?.naturalHeight ?? 0,
                 inspectorOpen: !!document.querySelector('.inspector, .panel') };
      })()`);
      console.log('portrait spotlight hero:', JSON.stringify(hero));
      if (hero) {
        const boxAspect = hero.w / hero.h;
        const imgAspect = hero.nw / hero.nh;
        console.log('box aspect:', boxAspect.toFixed(3), 'img aspect:', imgAspect.toFixed(3),
          'delta%:', Math.abs(boxAspect - imgAspect) / imgAspect * 100);
      }
      await shot('repro-spotlight-portrait');
    }

    // ---- 2) 单击横图 → 聚焦 ----
    if (landscape) {
      await mouse('mousePressed', landscape.x + landscape.w / 2, landscape.y + landscape.h / 2, 'left');
      await mouse('mouseReleased', landscape.x + landscape.w / 2, landscape.y + landscape.h / 2, 'left');
      await sleep(900);
      const hero = await evaluate(`(() => {
        const el = document.querySelector('.tile--spotlight');
        if (!el) return null;
        const img = el.querySelector('img');
        const box = el.getBoundingClientRect();
        return { w: box.width, h: box.height, nw: img?.naturalWidth ?? 0, nh: img?.naturalHeight ?? 0 };
      })()`);
      console.log('landscape spotlight hero:', JSON.stringify(hero));
      await shot('repro-spotlight-landscape');
    }

    // ---- 3) 右键概要：遮罩覆盖 ----
    const any = targets[10] ?? targets[0];
    if (any) {
      // 先退出聚焦
      await mouse('mousePressed', 30, 700, 'left');
      await mouse('mouseReleased', 30, 700, 'left');
      await sleep(600);
      await mouse('mousePressed', any.x + any.w / 2, any.y + any.h / 2, 'right');
      await mouse('mouseReleased', any.x + any.w / 2, any.y + any.h / 2, 'right');
      await sleep(400);
      const cover = await evaluate(`(() => {
        const info = document.querySelector('.tile__info');
        if (!info) return null;
        const tile = info.closest('.tile');
        const a = info.getBoundingClientRect();
        const b = tile.getBoundingClientRect();
        const cs = getComputedStyle(info);
        return {
          info: { x: a.x, y: a.y, w: a.width, h: a.height },
          tile: { x: b.x, y: b.y, w: b.width, h: b.height },
          tileRadius: getComputedStyle(tile).borderRadius,
          infoRadius: cs.borderRadius,
          insetDiff: { dx: b.x - a.x, dy: b.y - a.y, dw: b.width - a.width, dh: b.height - a.height },
        };
      })()`);
      console.log('summary cover:', JSON.stringify(cover, null, 1));
      await shot('repro-summary', { x: Math.max(0, any.x - 8), y: Math.max(0, any.y - 8), width: any.w + 16, height: any.h + 16, scale: 1 });
      // 再次右键（目前应该不会退出 → 记录）
      await mouse('mousePressed', any.x + any.w / 2, any.y + any.h / 2, 'right');
      await mouse('mouseReleased', any.x + any.w / 2, any.y + any.h / 2, 'right');
      await sleep(300);
      const stillOpen = await evaluate(`!!document.querySelector('.tile__info')`);
      console.log('second right-click closes summary?', !stillOpen);
    }
  } finally {
    cdp?.close();
    try { child.kill(); } catch { /* noop */ }
    await sleep(300);
    hardDelete(profile);
  }
}

main().catch((error) => { console.error(error); process.exit(1); });
