#!/usr/bin/env node
/**
 * 诊断探针：单击聚焦竖图后 dump 所有 tile 视口坐标 + ASCII 覆盖图，
 * 用于定位聚焦图周围空带的来源。用法同 wall_repro.mjs。
 */
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync } from 'node:fs';
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
        return;
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
  const width = Number(argv.find((a) => a.startsWith('--width'))?.split('=')[1] ?? 1440);
  const height = Number(argv.find((a) => a.startsWith('--height'))?.split('=')[1] ?? 900);
  const wantLandscape = argv.includes('--landscape');
  const chrome = CANDIDATES.find(existsSync);
  if (!chrome) { console.error('no chrome'); process.exit(2); }
  const profile = mkdtempSync(join(tmpdir(), 'kanalib-probe-'));
  const port = 9900 + Math.floor(Math.random() * 100);
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
    const mouse = (type, x, y, button = 'left') =>
      cdp.send('Input.dispatchMouseEvent', {
        type, x, y, button,
        buttons: type === 'mouseReleased' ? 0 : button === 'right' ? 2 : 1,
        clickCount: 1,
      });

    await cdp.send('Page.navigate', { url: base + '#/' });
    for (let i = 0; i < 80; i += 1) {
      try { const n = await evaluate(`document.querySelectorAll('.tile').length`); if (n > 20) break; } catch { /* nav */ }
      await sleep(150);
    }
    await sleep(1200);

    const portrait = await evaluate(`(() => {
      let best = null;
      for (const tile of document.querySelectorAll('.tile')) {
        const img = tile.querySelector('img');
        if (!img || !img.naturalWidth) continue;
        const box = tile.getBoundingClientRect();
        if (box.left < 10 || box.top < 10 || box.right > innerWidth - 10 || box.bottom > innerHeight - 10) continue;
        if (box.width < 80 || box.height < 80) continue;
        const hit = document.elementFromPoint(box.x + box.width / 2, box.y + box.height / 2)?.closest('.tile');
        if (hit !== tile) continue;
        const nw = img.naturalWidth, nh = img.naturalHeight;
        const ok = ${wantLandscape ? 'nw > nh * 1.15' : 'nh > nw * 1.15'};
        if (!ok) continue;
        if (!best || box.width > best.w) best = { id: tile.dataset.id, x: box.x, y: box.y, w: box.width, h: box.height };
      }
      return best;
    })()`);
    if (!portrait) throw new Error('视口里没有可点的目标图');
    console.log('click target:', portrait.id, JSON.stringify(portrait));
    await mouse('mousePressed', portrait.x + portrait.w / 2, portrait.y + portrait.h / 2, 'left');
    await mouse('mouseReleased', portrait.x + portrait.w / 2, portrait.y + portrait.h / 2, 'left');
    await sleep(1500);

    const dump = await evaluate(`(() => {
      const tiles = [...document.querySelectorAll('.tile')].map((tile) => {
        const img = tile.querySelector('img');
        const b = tile.getBoundingClientRect();
        return {
          id: (tile.dataset.id || '').slice(0, 8),
          spot: tile.classList.contains('tile--spotlight'),
          // 视口坐标
          x: Math.round(b.x), y: Math.round(b.y), w: Math.round(b.width), h: Math.round(b.height),
          // 世界坐标（绝对定位父容器里的 offset，不受相机影响）
          wx: tile.offsetLeft, wy: tile.offsetTop,
          nw: img?.naturalWidth ?? 0, nh: img?.naturalHeight ?? 0,
        };
      });
      const hero = tiles.find((t) => t.spot);
      // 视口覆盖图：24px 网格，1 char = 24px 宽 / 48px 高
      const cell = 24;
      const cols = Math.ceil(innerWidth / cell), rows = Math.ceil(innerHeight / cell);
      const cover = new Array(cols * rows).fill(0);
      for (const t of tiles) {
        if (t.w < 5 || t.h < 5) continue;
        for (let r = Math.floor(t.y / cell); r < Math.min(rows, Math.ceil((t.y + t.h) / cell)); r++)
          for (let c = Math.floor(t.x / cell); c < Math.min(cols, Math.ceil((t.x + t.w) / cell)); c++)
            cover[r * cols + c] = 1;
      }
      const grid = [];
      for (let r = 0; r < rows; r += 2) {
        let line = '';
        for (let c = 0; c < cols; c++) {
          let covered = 0;
          for (let rr = r; rr < Math.min(rows, r + 2); rr++) covered = covered || cover[rr * cols + c];
          line += covered ? '#' : '.';
        }
        grid.push(String(Math.round(r * cell)).padStart(4) + ' ' + line);
      }
      return { hero, tiles, grid, vw: innerWidth, vh: innerHeight };
    })()`);

    console.log('hero:', JSON.stringify(dump.hero));
    console.log('tiles:', dump.tiles.length);
    for (const t of dump.tiles) console.log(' ', t.spot ? '*' : ' ', t.id, `view=${t.x},${t.y} ${t.w}x${t.h} world=${t.wx},${t.wy} img=${t.nw}x${t.nh}`);
    console.log('coverage map (' + dump.vw + 'x' + dump.vh + ', 1 char = 24px wide / 48px tall):');
    for (const line of dump.grid) console.log(line);
  } finally {
    cdp?.close();
    try { child.kill(); } catch { /* noop */ }
    await sleep(300);
    hardDelete(profile);
  }
}

main().catch((error) => { console.error(error); process.exit(1); });
