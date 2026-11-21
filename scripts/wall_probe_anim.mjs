#!/usr/bin/env node
/**
 * 诊断探针（只读）：动画衔接取证（反馈问题 6）。
 * 1) 双击展开：400–560ms 逐帧量「克隆图在不在 / 真实 track 透明度」，
 *    旧版 track-reveal 1ms@440ms 与克隆 420ms 落位之间有 20ms 双空窗。
 * 2) 检查器退出：详情页按 I 收起后应先挂 .inspector--leave 播 200ms 再卸载。
 * 用法：node scripts/wall_probe_anim.mjs --base=http://127.0.0.1:4318/
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

const FRAME = `(() => {
  const clone = document.querySelector('body > img[aria-hidden="true"]');
  const track = document.querySelector('.view__track');
  return {
    clone: !!clone,
    trackOpacity: track ? +getComputedStyle(track).opacity : null,
    veiled: !!document.querySelector('.view--expanding'),
  };
})()`;

async function main() {
  const argv = process.argv.slice(2);
  const base = (argv.find((a) => a.startsWith('--base'))?.split('=')[1] ?? 'http://127.0.0.1:4318/');
  const chrome = CANDIDATES.find(existsSync);
  if (!chrome) { console.error('no chrome'); process.exit(2); }
  const profile = mkdtempSync(join(tmpdir(), 'kanalib-anim-'));
  const port = 9970 + Math.floor(Math.random() * 30);
  const child = spawn(chrome, [
    '--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${profile}`,
    '--no-first-run', '--no-default-browser-check', '--disable-extensions', '--hide-scrollbars',
    '--window-size=1440,900', 'about:blank',
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
    await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
    const evaluate = async (expression) => {
      const result = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
      if (result.exceptionDetails) throw new Error(result.exceptionDetails.text);
      return result.result.value;
    };
    const mouse = (type, x, y, clickCount = 1) =>
      cdp.send('Input.dispatchMouseEvent', {
        type, x, y, button: 'left',
        buttons: type === 'mouseReleased' ? 0 : 1,
        clickCount,
      });

    await cdp.send('Page.navigate', { url: base + '#/' });
    for (let i = 0; i < 80; i += 1) {
      try { const n = await evaluate(`document.querySelectorAll('.tile img.is-loaded').length`); if (n > 10) break; } catch { /* nav */ }
      await sleep(200);
    }
    await sleep(800);

    // 双击一张整卡在安全区里的卡 → 展开飞行
    const spot = await evaluate(`(() => {
      const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 120;
      for (const tile of document.querySelectorAll('.tile')) {
        const b = tile.getBoundingClientRect();
        if (b.left < seam + 40 || b.top < 90 || b.right > innerWidth - 60 || b.bottom > innerHeight - 120) continue;
        if (b.width < 120 || b.height < 120) continue;
        return { x: Math.round(b.left + b.width / 2), y: Math.round(b.top + b.height / 2) };
      }
      return null;
    })()`);
    if (!spot) { console.log('no safe tile for double-click'); return; }
    await mouse('mousePressed', spot.x, spot.y, 1);
    await mouse('mouseReleased', spot.x, spot.y, 1);
    await sleep(120);
    const t0 = Date.now();
    await mouse('mousePressed', spot.x, spot.y, 2);
    await mouse('mouseReleased', spot.x, spot.y, 2);
    const frames = [];
    for (const at of [380, 410, 430, 450, 480, 520, 560]) {
      const wait = at - (Date.now() - t0);
      if (wait > 0) await sleep(wait);
      frames.push({ at, ...(await evaluate(FRAME)) });
    }
    console.log('expand frames:', JSON.stringify(frames));
    const gap = frames.filter((f) => f.at >= 400 && f.at <= 520 && !f.clone && (f.trackOpacity ?? 0) < 0.05);
    console.log(gap.length === 0 ? 'expand: no blank gap in 400-520ms' : 'expand: BLANK GAP at ' + JSON.stringify(gap));

    // 检查器退出：I 收起 → 先 .inspector--leave 再卸载
    await sleep(600);
    const before = await evaluate(`!!document.querySelector('.inspector')`);
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'i', code: 'KeyI', windowsVirtualKeyCode: 73 });
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'i', code: 'KeyI', windowsVirtualKeyCode: 73 });
    await sleep(60);
    const leaving = await evaluate(`!!document.querySelector('.inspector--leave')`);
    await sleep(300);
    const gone = await evaluate(`!document.querySelector('.inspector')`);
    console.log('inspector exit:', JSON.stringify({ before, leavingAt60ms: leaving, goneAt360ms: gone }));
  } finally {
    cdp?.close();
    try { child.kill(); } catch { /* noop */ }
    await sleep(300);
    hardDelete(profile);
  }
}

main().catch((error) => { console.error(error); process.exit(1); });