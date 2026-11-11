#!/usr/bin/env node
/**
 * 标签墙规模阈值探针：图片数 < 8 的标签回退有限世界，>= 8 保持 360° 无界。
 *
 * 用法（先在 web/ 里起服务）：
 *   node scripts/wall_probe_tag_threshold.mjs --base http://127.0.0.1:4273/ --out web/screenshots/tag360-verify
 *
 * 断言：
 *  1) `#/t/rating:e`（1 张）：有限世界，只有 1 张卡、无重复；拖拽远处不再生成新卡。
 *  2) `#/t/alcohol`（7 张）：有限世界，7 张各不相同；拖拽远处不再生成新卡。
 *  3) `#/t/%3A%3C`（8 张，阈值边界）：无界世界，卡片多于 8 张且可继续拖动补图。
 *  4) `#/t/azur_lane`（9 张）：无界世界。
 *  5) 截图：小标签 / 7 张 / 8 张边界 / 大标签各一张。
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

const MEASURE = String.raw`
  (() => {
    const canvas = document.querySelector('.canvas');
    if (!canvas) return null;
    const cr = canvas.getBoundingClientRect();
    const world = document.querySelector('.world');
    const tiles = [...document.querySelectorAll('.tile')].map((el) => {
      const r = el.getBoundingClientRect();
      return {
        id: el.dataset.id || null,
        rect: { x: r.left, y: r.top, w: r.width, h: r.height },
      };
    });
    return {
      canvas: { left: cr.left, top: cr.top, w: cr.width, h: cr.height },
      transform: world ? world.style.transform : null,
      // 有限世界会写死 .world 的宽高；无界世界两者留空。
      worldWidth: world ? world.style.width : null,
      worldHeight: world ? world.style.height : null,
      tileCount: tiles.length,
      uniqueIds: new Set(tiles.map((t) => t.id)).size,
      ids: [...new Set(tiles.map((t) => t.id))],
      hash: location.hash,
      overflowX: document.documentElement.scrollWidth - innerWidth,
    };
  })()
`;

async function main() {
  const raw = arg('base', 'http://127.0.0.1:4273/');
  const base = raw.endsWith('/') ? raw : raw + '/';
  const outDir = resolve(arg('out', 'web/screenshots/tag360-verify'));
  const chrome = CANDIDATES.find((path) => existsSync(path));
  if (!chrome) {
    console.error('找不到 Chrome/Edge');
    process.exit(2);
  }

  mkdirSync(outDir, { recursive: true });
  const profile = mkdtempSync(join(tmpdir(), 'kanalib-tag-threshold-'));
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
    await cdp.send('Emulation.setDeviceMetricsOverride', {
      width: 1440,
      height: 900,
      deviceScaleFactor: 1,
      mobile: false,
    });

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
    const shot = async (name) => {
      const png = await cdp.send('Page.captureScreenshot', { format: 'png' });
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
      await sleep(120);
    };
    const waitReady = async (label, timeoutMs = 45000) => {
      const deadline = Date.now() + timeoutMs;
      let prevSig = null;
      let stable = 0;
      let measured = null;
      let loaded = false;
      while (Date.now() < deadline) {
        measured = await evaluate(MEASURE);
        const countText = await evaluate(
          `document.querySelector('.seam__foot .seam__count')?.textContent.trim() ?? ''`,
        );
        const nums = (countText.match(/[\d,]+/g) || []).map((s) => Number(s.replace(/,/g, '')));
        loaded = nums.length === 2 && nums[0] === nums[1] && nums[0] > 0;
        if (measured && loaded) {
          const sig = [measured.transform, measured.tileCount, ...measured.ids].join('|');
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
      if (!measured || !loaded) {
        throw new Error(`${label}: 页面没就绪（canvas=${measured ? 'ok' : '缺失'}, loaded=${loaded}）`);
      }
      await sleep(1100);
      return await evaluate(MEASURE);
    };
    /** 从画布中心连续拖三段：有限世界会被夹住，无界世界会持续补图。 */
    const dragFar = async (measure) => {
      const cx = measure.canvas.left + measure.canvas.w / 2;
      const cy = measure.canvas.top + measure.canvas.h / 2;
      for (let i = 0; i < 3; i += 1) {
        await drag(cx + 260, cy + 200, cx - 360, cy - 260);
      }
      await sleep(250);
      return await evaluate(MEASURE);
    };

    const visit = async (hash, label, file) => {
      await cdp.send('Page.navigate', { url: base + hash });
      const before = await waitReady(label);
      const path = await shot(file);
      const after = await dragFar(before);
      return { before, after, screenshot: path };
    };

    // 1) 1 张：`rating:e` 不在 facets 前 2000 里，走已加载集合兜底。
    const one = await visit('#/t/rating:e', 'rating:e', 'threshold-small-rating-e.png');
    report.cases.one = {
      before: one.before,
      after: one.after,
      screenshot: one.screenshot,
      finite: one.before.worldWidth !== '',
      stableAfterDrag: one.after.tileCount === one.before.tileCount,
    };

    // 2) 7 张：阈值下方。
    const seven = await visit('#/t/alcohol', 'alcohol', 'threshold-small-alcohol.png');
    report.cases.seven = {
      before: seven.before,
      after: seven.after,
      screenshot: seven.screenshot,
      finite: seven.before.worldWidth !== '',
      stableAfterDrag: seven.after.tileCount === seven.before.tileCount,
    };

    // 3) 8 张：阈值边界，仍应无界。
    const eight = await visit('#/t/%3A%3C', ':<', 'threshold-boundary-8.png');
    report.cases.eight = {
      before: eight.before,
      after: eight.after,
      screenshot: eight.screenshot,
      infinite: eight.before.worldWidth === '',
      dragMoved: eight.before.transform !== eight.after.transform,
    };

    // 4) 9 张：阈值上方。
    const nine = await visit('#/t/azur_lane', 'azur_lane', 'threshold-large-azur_lane.png');
    report.cases.nine = {
      before: nine.before,
      after: nine.after,
      screenshot: nine.screenshot,
      infinite: nine.before.worldWidth === '',
      dragMoved: nine.before.transform !== nine.after.transform,
    };

    // 5) 大标签截图（本轮验收要求「小标签页 + 大标签页各一张」）。
    await cdp.send('Page.navigate', { url: base + '#/t/solo' });
    const large = await waitReady('solo');
    report.cases.large = { before: large, screenshot: await shot('threshold-large-solo.png') };

    const c = report.cases;
    report.passed =
      report.errors.length === 0 &&
      // 1 张：有限、唯一、拖拽后不新增
      c.one.before.tileCount === 1 &&
      c.one.before.uniqueIds === 1 &&
      c.one.finite &&
      c.one.stableAfterDrag &&
      // 7 张：有限、7 张各不相同、拖拽后不新增
      c.seven.before.tileCount === 7 &&
      c.seven.before.uniqueIds === 7 &&
      c.seven.finite &&
      c.seven.stableAfterDrag &&
      // 8 张边界：无界、拖动有效
      c.eight.infinite &&
      c.eight.before.tileCount > 8 &&
      c.eight.before.uniqueIds <= 8 &&
      c.eight.dragMoved &&
      // 9 张：无界
      c.nine.infinite &&
      c.nine.before.tileCount > 9 &&
      c.nine.dragMoved &&
      // 大标签：无界且铺满
      c.large.before.worldWidth === '' &&
      c.large.before.tileCount >= 12;
  } finally {
    if (cdp) cdp.close();
    try {
      child.kill();
    } catch {
      /* 已退出 */
    }
    hardDelete(profile);
  }

  const reportPath = join(outDir, 'tag-threshold-report.json');
  writeFileSync(reportPath, JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report, null, 2));
  console.log('REPORT=' + reportPath);
  console.log(report.passed ? 'PASSED' : 'FAILED');
  process.exit(report.passed ? 0 : 1);
}

main().catch((error) => {
  console.error(error);
  process.exit(2);
});
