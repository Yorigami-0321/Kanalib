#!/usr/bin/env node
/**
 * 一次性诊断（只读，不改应用代码）：点击指定 id 前缀的图，dump 聚焦后
 * hero 框 / img 元素的真实几何与加载状态，定位"图片不填满聚焦框"的反例。
 * 用法：node scripts/wall_probe_case.mjs --base http://127.0.0.1:4318/ --id 30430c00
 */
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const CANDIDATES = [
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
  'C:/Program Files/Microsoft/Edge/Application/msedge.exe',
];

class Cdp {
  constructor(url) { this.url = url; this.nextId = 1; this.pending = new Map(); }
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
      }
    });
  }
  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((res, rej) => {
      this.pending.set(id, { resolve: res, reject: rej });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }
  close() { try { this.ws.close(); } catch { /* noop */ } }
}

async function waitForDevtools(port, timeoutMs = 12000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try { const r = await fetch(`http://127.0.0.1:${port}/json/version`); if (r.ok) return; } catch { /* retry */ }
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
  const argv = process.argv.slice(2);
  const base = (argv.find((a) => a.startsWith('--base'))?.split('=')[1] ?? 'http://127.0.0.1:4318/');
  const idPrefix = (argv.find((a) => a.startsWith('--id'))?.split('=')[1] ?? '');
  const width = Number(argv.find((a) => a.startsWith('--width'))?.split('=')[1] ?? 1440);
  const height = Number(argv.find((a) => a.startsWith('--height'))?.split('=')[1] ?? 900);
  if (!idPrefix) throw new Error('需要 --id <id前缀>');
  const chrome = CANDIDATES.find(existsSync);
  if (!chrome) { console.error('no chrome'); process.exit(2); }
  const profile = mkdtempSync(join(tmpdir(), 'kanalib-case-'));
  const port = 9950 + Math.floor(Math.random() * 50);
  const child = spawn(chrome, [
    '--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
    '--no-first-run', '--no-default-browser-check', '--disable-extensions', '--hide-scrollbars',
    `--window-size=${width},${height}`, 'about:blank',
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
    await cdp.send('Emulation.setDeviceMetricsOverride', { width, height, deviceScaleFactor: 1, mobile: false });

    const evaluate = async (expression) => {
      const result = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
      if (result.exceptionDetails) throw new Error(result.exceptionDetails.text + ': ' + (result.exceptionDetails.exception?.description ?? ''));
      return result.result.value;
    };
    const mouse = (type, x, y, button = 'left', extra = {}) =>
      cdp.send('Input.dispatchMouseEvent', {
        type, x, y, button,
        buttons: type === 'mouseReleased' ? 0 : button === 'right' ? 2 : 1,
        clickCount: 1, ...extra,
      });

    // 走稀有标签过滤墙，保证目标 tile 落在初始渲染范围
    await cdp.send('Page.navigate', { url: base + '#/t/mobius_(honkai_impact)' });
    for (let i = 0; i < 80; i += 1) {
      try { const n = await evaluate(`document.querySelectorAll('.tile').length`); if (n > 0) break; } catch { /* nav */ }
      await sleep(150);
    }
    await sleep(1200);

    // 找目标 tile；不在初始渲染范围就走侧栏"按文件名搜索"过滤出它
    const findTile = `(() => {
      for (const tile of document.querySelectorAll('.tile')) {
        if (!(tile.dataset.id || '').startsWith('${idPrefix}')) continue;
        const img = tile.querySelector('img');
        const b = tile.getBoundingClientRect();
        return {
          found: true, id: tile.dataset.id,
          x: b.x, y: b.y, w: b.width, h: b.height,
          nw: img?.naturalWidth ?? 0, nh: img?.naturalHeight ?? 0,
          src: img?.currentSrc ?? '',
          cls: tile.className,
        };
      }
      return { found: false };
    })()`;
    let tile = await evaluate(findTile);
    if (!tile.found) {
      await evaluate(`(() => {
        const input = document.querySelector('input[placeholder="按文件名搜索"]');
        if (!input) return false;
        const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value').set;
        setter.call(input, '${idPrefix}');
        input.dispatchEvent(new Event('input', { bubbles: true }));
        return true;
      })()`);
      await sleep(1500);
      tile = await evaluate(findTile);
    }
    if (!tile.found) {
      const dbg = await evaluate(`(() => ({
        tiles: document.querySelectorAll('.tile').length,
        ids: [...document.querySelectorAll('.tile')].slice(0, 12).map((t) => (t.dataset.id || '').slice(0, 8)),
        counter: (document.body.innerText.match(/\\d+ \\/ \\d+/) || [''])[0],
        url: location.href,
      }))()`);
      throw new Error('目标 tile 未渲染。当前页面: ' + JSON.stringify(dbg));
    }
    console.log('pre-click tile:', JSON.stringify(tile, null, 1));

    // --throttle：慢网复现白带场景（64KB/s + 300ms），点击前生效，连拍加载中间帧。
    // 修复后聚焦换图先 decode：加载期间 img 应保持 t256 旧画面（complete=true），
    // 而不是半解码的 t1024（上半清晰 + 下半白带）。
    const throttle = argv.includes('--throttle');
    if (throttle) {
      await cdp.send('Network.enable');
      await cdp.send('Network.emulateNetworkConditions', {
        offline: false, latency: 300, downloadThroughput: 64 * 1024, uploadThroughput: 64 * 1024,
      });
    }

    await mouse('mousePressed', tile.x + tile.w / 2, tile.y + tile.h / 2, 'left');
    await mouse('mouseReleased', tile.x + tile.w / 2, tile.y + tile.h / 2, 'left');

    if (throttle) {
      const { mkdirSync, writeFileSync } = await import('node:fs');
      mkdirSync('web/screenshots', { recursive: true });
      let elapsed = 0;
      for (const ms of [500, 1200, 2500]) {
        await sleep(ms - elapsed);
        elapsed = ms;
        const mid = await evaluate(`(() => {
          const img = document.querySelector('.tile--spotlight img');
          if (!img) return null;
          return { src: img.currentSrc.split('/').slice(-2).join('/'), complete: img.complete, nw: img.naturalWidth };
        })()`);
        const shot = await cdp.send('Page.captureScreenshot', { format: 'png' });
        writeFileSync(`web/screenshots/repro-midload-fixed-${ms}.png`, Buffer.from(shot.data, 'base64'));
        console.log(`midload ${ms}ms:`, JSON.stringify(mid));
      }
      await cdp.send('Network.emulateNetworkConditions', {
        offline: false, latency: 0, downloadThroughput: -1, uploadThroughput: -1,
      });
      await sleep(3000);
    } else {
      await sleep(1800);
    }

    const dump = await evaluate(`(() => {
      const hero = document.querySelector('.tile--spotlight');
      if (!hero) return { hero: null };
      const img = hero.querySelector('img');
      const hb = hero.getBoundingClientRect();
      const cs = getComputedStyle(hero);
      const ics = img ? getComputedStyle(img) : null;
      const ib = img ? img.getBoundingClientRect() : null;
      return {
        hero: {
          rect: { x: hb.x, y: hb.y, w: hb.width, h: hb.height },
          cls: hero.className,
          bg: cs.backgroundColor,
          children: [...hero.children].map((el) => el.tagName + '.' + String(el.className).split(' ').join('.')),
        },
        img: img ? {
          rect: { x: ib.x, y: ib.y, w: ib.width, h: ib.height },
          natural: { w: img.naturalWidth, h: img.naturalHeight },
          src: img.currentSrc,
          objectFit: ics.objectFit,
          cssW: ics.width, cssH: ics.height,
          complete: img.complete,
        } : null,
        // 视口内与 hero 相交的其他 tile 数（看聚焦重排是否发生）
        tilesNearby: [...document.querySelectorAll('.tile')].filter((t) => {
          const b = t.getBoundingClientRect();
          return b.right > 0 && b.left < innerWidth && b.bottom > 0 && b.top < innerHeight;
        }).length,
      };
    })()`);
    console.log('post-click dump:', JSON.stringify(dump, null, 1));
    if (dump.hero && dump.img) {
      const frameRatio = dump.hero.rect.w / dump.hero.rect.h;
      const imgRatio = dump.img.natural.w / dump.img.natural.h;
      const boxRatio = dump.img.rect.w / dump.img.rect.h;
      console.log('frame ratio:', frameRatio.toFixed(4), '| img natural ratio:', imgRatio.toFixed(4),
        '| img box ratio:', boxRatio.toFixed(4),
        '| frame-img natural delta%:', (Math.abs(frameRatio - imgRatio) / imgRatio * 100).toFixed(2),
        '| img box covers frame%:', (dump.img.rect.w * dump.img.rect.h) / (dump.hero.rect.w * dump.hero.rect.h) * 100);
    }
  } finally {
    cdp?.close();
    try { child.kill(); } catch { /* noop */ }
    await sleep(300);
    hardDelete(profile);
  }
}

main().catch((error) => { console.error(error); process.exit(1); });
