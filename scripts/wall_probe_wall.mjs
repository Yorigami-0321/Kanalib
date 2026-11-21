#!/usr/bin/env node
/**
 * 诊断探针（只读）：首页无限墙上统计每张卡的 cover 裁切程度，
 * 找出被横条卡重度裁切的竖图，单击聚焦后对比前后可见面积。
 * 用法：node scripts/wall_probe_wall.mjs --base=http://127.0.0.1:4318/ [--id=<id前缀>]
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

const COLLECT = `(() => {
  const out = [];
  for (const tile of document.querySelectorAll('.tile')) {
    const img = tile.querySelector('img');
    if (!img || !img.naturalWidth) continue;
    const b = tile.getBoundingClientRect();
    if (b.width < 10 || b.height < 10) continue;
    const boxRatio = b.width / b.height;
    const imgRatio = img.naturalWidth / img.naturalHeight;
    const visible = Math.min(boxRatio, imgRatio) / Math.max(boxRatio, imgRatio);
    out.push({
      id: tile.dataset.id,
      x: b.x, y: b.y, w: b.width, h: b.height,
      boxRatio, imgRatio, visible,
      inView: b.x >= 140 && b.y >= 30 && b.right <= innerWidth - 30 && b.bottom <= innerHeight - 30,
    });
  }
  return out;
})()`;

async function main() {
  const argv = process.argv.slice(2);
  const base = (argv.find((a) => a.startsWith('--base'))?.split('=')[1] ?? 'http://127.0.0.1:4318/');
  const idPrefix = (argv.find((a) => a.startsWith('--id'))?.split('=')[1] ?? '');
  const width = Number(argv.find((a) => a.startsWith('--width'))?.split('=')[1] ?? 1440);
  const height = Number(argv.find((a) => a.startsWith('--height'))?.split('=')[1] ?? 900);
  const chrome = CANDIDATES.find(existsSync);
  if (!chrome) { console.error('no chrome'); process.exit(2); }
  const profile = mkdtempSync(join(tmpdir(), 'kanalib-wall-'));
  const port = 9960 + Math.floor(Math.random() * 40);
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
        buttons: type === 'mouseReleased' ? 0 : 1,
        clickCount: 1,
      });

    await cdp.send('Page.navigate', { url: base + '#/' });
    for (let i = 0; i < 100; i += 1) {
      try { const n = await evaluate(`document.querySelectorAll('.tile img.is-loaded').length`); if (n > 20) break; } catch { /* nav */ }
      await sleep(200);
    }
    await sleep(1500);

    const tiles = await evaluate(COLLECT);
    if (width <= 400) {
      const cov = await evaluate(`(() => {
        const seam = document.querySelector('.seam')?.getBoundingClientRect().right ?? 64;
        const miss = [];
        let hit = 0, total = 0;
        for (let x = seam + 20; x < innerWidth - 20; x += 40) {
          for (let y = 20; y < innerHeight - 20; y += 40) {
            total += 1;
            const el = document.elementFromPoint(x, y);
            if (el && el.closest('.tile')) hit += 1;
            else miss.push([x, y, el ? (el.className || el.tagName).toString().slice(0, 30) : 'null']);
          }
        }
        return { cover: hit / total, miss };
      })()`);
      console.log('390 coverage:', cov.cover.toFixed(3), 'misses:', JSON.stringify(cov.miss));
    }
    const portraits = tiles.filter((t) => t.imgRatio < 0.9);
    const heavy = portraits.filter((t) => t.visible < 0.6);
    console.log('tiles:', tiles.length, '| portraits:', portraits.length, '| heavy-crop portraits (visible<60%):', heavy.length);
    const buckets = { '>=80%': 0, '60-80%': 0, '40-60%': 0, '<40%': 0 };
    for (const t of portraits) {
      if (t.visible >= 0.8) buckets['>=80%'] += 1;
      else if (t.visible >= 0.6) buckets['60-80%'] += 1;
      else if (t.visible >= 0.4) buckets['40-60%'] += 1;
      else buckets['<40%'] += 1;
    }
    console.log('portrait visible-fraction buckets:', JSON.stringify(buckets));
    const worst = [...portraits].sort((a, b) => a.visible - b.visible).slice(0, 8);
    console.log('worst portraits:', JSON.stringify(worst.map((t) => ({
      id: t.id.slice(0, 8), w: Math.round(t.w), h: Math.round(t.h),
      imgRatio: +t.imgRatio.toFixed(3), visible: +(t.visible * 100).toFixed(1) + '%', inView: t.inView,
    })), null, 1));

    // 选点击目标：优先 --id 指定；--big 取视口内面积最大的竖图（问题 B 最坏情形），
    // 否则取视口内裁切最重的竖图（问题 A 最坏情形）。
    const wantBig = argv.includes('--big');
    let targetTile = idPrefix ? tiles.find((t) => t.id.startsWith(idPrefix) && t.inView) : null;
    if (!targetTile && wantBig) {
      // 大卡常半出屏：中心在安全视口内即可点。
      targetTile = portraits
        .filter((t) => {
          const cx = t.x + t.w / 2;
          const cy = t.y + t.h / 2;
          return cx >= 140 && cx <= width - 30 && cy >= 30 && cy <= height - 30;
        })
        .sort((a, b) => b.w * b.h - a.w * a.h)[0];
    }
    if (!targetTile) {
      const inViewPortraits = portraits.filter((t) => t.inView);
      targetTile = inViewPortraits.sort((a, b) => a.visible - b.visible)[0];
    }
    if (!targetTile) { console.log('no suitable in-view target; skip click phase'); return; }
    console.log('click target:', JSON.stringify({ id: targetTile.id.slice(0, 8), x: Math.round(targetTile.x), y: Math.round(targetTile.y), w: Math.round(targetTile.w), h: Math.round(targetTile.h), imgRatio: +targetTile.imgRatio.toFixed(3), visibleBefore: +(targetTile.visible * 100).toFixed(1) + '%' }));
    const preTransform = await evaluate(`document.querySelector('.world')?.style.transform`);
    console.log('pre-click world transform:', preTransform);

    await mouse('mousePressed', targetTile.x + targetTile.w / 2, targetTile.y + targetTile.h / 2);
    await mouse('mouseReleased', targetTile.x + targetTile.w / 2, targetTile.y + targetTile.h / 2);
    // 聚焦换锐利档会重挂 img：等 hero 出现且新图加载完再量。
    let dump = {};
    for (let i = 0; i < 40; i += 1) {
      await sleep(250);
      dump = await evaluate(`(() => {
        const hero = document.querySelector('.tile--spotlight');
        if (!hero) return {};
        const img = hero.querySelector('img');
        const hb = hero.getBoundingClientRect();
        const inspector = document.querySelector('.inspector');
        const ib = inspector ? inspector.getBoundingClientRect() : null;
        return {
          id: hero.dataset.id,
          rect: { x: hb.x, y: hb.y, w: hb.width, h: hb.height },
          natural: img ? { w: img.naturalWidth, h: img.naturalHeight } : null,
          complete: img ? img.complete : false,
          src: img ? img.currentSrc : '',
          inspector: ib ? { x: ib.x, y: ib.y, w: ib.width, h: ib.height } : null,
        };
      })()`);
      if (dump.rect && dump.complete) break;
    }
    if (!dump.rect) {
      const dbg = await evaluate(`(() => ({
        canvas: document.querySelector('.canvas')?.className,
        tiles: document.querySelectorAll('.tile').length,
        targetStillThere: !!document.querySelector('.tile[data-id="${targetTile.id}"]'),
        inspectorTitle: document.querySelector('.inspector')?.textContent?.slice(0, 60) ?? null,
        hitAtClick: document.elementFromPoint(${Math.round(targetTile.x + targetTile.w / 2)}, ${Math.round(targetTile.y + targetTile.h / 2)})?.closest('.tile')?.dataset.id ?? null,
        viewport: { w: innerWidth, h: innerHeight },
        targetDom: (() => {
          const t = document.querySelector('.tile[data-id="${targetTile.id}"]');
          return t ? { left: t.offsetLeft, top: t.offsetTop, w: t.offsetWidth, h: t.offsetHeight, cls: t.className } : null;
        })(),
        worldTransform: document.querySelector('.world')?.style.transform,
        url: location.href,
      }))()`);
      console.log('no hero after click; page state:', JSON.stringify(dbg));
      return;
    }
    const beforeArea = targetTile.w * targetTile.h;
    const afterArea = dump.rect.w * dump.rect.h;
    // 聚焦前后"可见图片内容"面积：cover 下整卡都是图；聚焦框等比后也是整框。
    console.log('post-click hero:', JSON.stringify(dump, null, 1));
    console.log('area before:', Math.round(beforeArea), '| after:', Math.round(afterArea),
      '| after/before:', (afterArea / beforeArea).toFixed(2),
      afterArea < beforeArea ? '→ SHRANK (问题 B 复现)' : '→ grew');
  } finally {
    cdp?.close();
    try { child.kill(); } catch { /* noop */ }
    await sleep(300);
    hardDelete(profile);
  }
}

main().catch((error) => { console.error(error); process.exit(1); });