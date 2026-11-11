#!/usr/bin/env node
/**
 * 断片恢复探针：拦截分片请求 → 首屏失败（空态 + 重试按钮）→ 点击重试 → 分片放行 → 卡片回来。
 *
 * 只验证失败路径的一条链：
 *   1) 0 张时出现“数据源没加载成功”+重试按钮（不是两个错误入口）
 *   2) 用真实鼠标坐标点击重试（不是 DOM click）
 *   3) 放行后卡片渲染、空态消失
 *
 * 用法（先在 web/ 里起服务）：
 *   node scripts/wall_probe_recover.mjs --base http://127.0.0.1:4273/
 *
 * 退出码：0 通过；1 断言失败；2 环境问题。
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
  const response = await fetch(`http://127.0.0.1:${port}/json/new?${encodeURIComponent(url)}`, { method: 'PUT' });
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

async function evaluate(cdp, expression) {
  const result = await cdp.send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (result.exceptionDetails) {
    throw new Error('页面脚本异常：' + (result.exceptionDetails.exception?.description ?? result.exceptionDetails.text));
  }
  return result.result?.value;
}

async function main() {
  const base = (arg('base', 'http://127.0.0.1:4273/') + (arg('base', 'http://127.0.0.1:4273/').endsWith('/') ? '' : '/'));
  const chrome = CANDIDATES.find((path) => existsSync(path));
  if (!chrome) {
    console.error('找不到 Chrome/Edge');
    process.exit(2);
  }

  const profile = mkdtempSync(join(tmpdir(), 'kanalib-recover-'));
  const port = 9500 + Math.floor(Math.random() * 400);
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

  const report = { base, blocked: 0, allow: false, before: null, click: null, after: null, passed: false };
  let cdp = null;
  try {
    await waitForDevtools(port);
    const target = await newTarget(port, 'about:blank');
    cdp = new Cdp(target.webSocketDebuggerUrl);
    await cdp.connect();
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    await cdp.send('Emulation.setDeviceMetricsOverride', {
      width: 1440,
      height: 900,
      deviceScaleFactor: 1,
      mobile: false,
    });

    await cdp.send('Fetch.enable', {
      patterns: [{ urlPattern: '*/data/m/*', requestStage: 'Request' }],
    });
    cdp.on((message) => {
      if (message.method !== 'Fetch.requestPaused') return;
      const { requestId } = message.params;
      if (!report.allow) {
        report.blocked += 1;
        cdp.send('Fetch.failRequest', { requestId, errorReason: 'Failed' }).catch(() => {});
      } else {
        cdp.send('Fetch.continueRequest', { requestId }).catch(() => {});
      }
    });

    await cdp.send('Page.navigate', { url: base });
    // 初始请求 + 两次自动重试（800ms / 2000ms）都失败后才停在错误态。
    await sleep(4200);

    report.before = await evaluate(
      cdp,
      `(() => {
        const note = document.querySelector('.wall__note');
        const button = note?.querySelector('button');
        const rect = button ? button.getBoundingClientRect() : null;
        return {
          blockedPhase: !!note,
          noteText: note ? note.textContent.trim().slice(0, 60) : null,
          hasRetry: !!button,
          buttonLabel: button ? button.textContent.trim() : null,
          canvasErrorBars: document.querySelectorAll('.canvas__error').length,
          tiles: document.querySelectorAll('.tile').length,
          rect: rect ? { x: rect.x + rect.width / 2, y: rect.y + rect.height / 2 } : null,
        };
      })()`,
    );

    if (!report.before.rect) throw new Error('空态里没有可点的重试按钮');

    const { x, y } = report.before.rect;
    report.allow = true;
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseMoved', x, y });
    await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x, y, button: 'left', clickCount: 1 });
    await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x, y, button: 'left', clickCount: 1 });
    await sleep(3000);

    report.after = await evaluate(
      cdp,
      `(() => ({
        note: !!document.querySelector('.wall__note'),
        tiles: document.querySelectorAll('.tile').length,
        canvasErrorBars: document.querySelectorAll('.canvas__error').length,
        appErrorBar: !!document.querySelector('.app__error'),
        firstTile: document.querySelector('.tile')?.getAttribute('data-id') ?? null,
      }))()`,
    );

    report.passed =
      report.before.blockedPhase &&
      report.before.hasRetry &&
      report.before.tiles === 0 &&
      report.before.canvasErrorBars === 0 &&
      !report.after.note &&
      report.after.tiles > 0 &&
      report.after.canvasErrorBars === 0;
  } finally {
    if (cdp) cdp.close();
    try {
      child.kill();
    } catch {
      /* 已退出 */
    }
    hardDelete(profile);
  }

  console.log(JSON.stringify(report, null, 2));
  process.exit(report.passed ? 0 : 1);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
