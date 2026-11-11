#!/usr/bin/env node
/**
 * 浏览状态回归探针：无效深链、全库搜索、URL 状态恢复、小集合标签墙。
 *
 * 用法（先在 web/ 里起服务）：
 *   node scripts/wall_probe_state.mjs --base http://127.0.0.1:4273/
 *
 * 断言：
 *  1) `#/d/<40 个 0>`：8s 内 console error 0，出现缺失页与「回首页 / 用此 ID 搜索」两个动作。
 *  2) `#/?q=pink_hair`：打开搜索面板后显示「筛选后 142 张」。
 *  3) `#/?q=cat_ears&sort=name&mono=1&matte=1`：刷新后 hash 与状态保持。
 *  4) `#/t/rating:e`：只渲染 1 张卡，且没有重复 id。
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

async function main() {
  const raw = arg('base', 'http://127.0.0.1:4273/');
  const base = raw.endsWith('/') ? raw : raw + '/';
  const chrome = CANDIDATES.find((path) => existsSync(path));
  if (!chrome) {
    console.error('找不到 Chrome/Edge');
    process.exit(2);
  }

  const profile = mkdtempSync(join(tmpdir(), 'kanalib-state-'));
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

  const report = { base, errors: [], cases: {}, passed: false };
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
        report.errors.push(message.params.args.map((arg) => arg.value ?? arg.description ?? '').join(' '));
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
        throw new Error('页面脚本异常：' + (result.exceptionDetails.exception?.description ?? result.exceptionDetails.text));
      }
      return result.result?.value;
    };

    // 1) 无效深链
    await cdp.send('Page.navigate', { url: base + '#/d/' + '0'.repeat(40) });
    await sleep(8000);
    report.cases.invalidDeepLink = await evaluate(`(() => {
      const missing = document.querySelector('.stage__missing');
      return {
        missing: !!missing,
        actions: missing ? [...missing.querySelectorAll('button')].map((b) => b.textContent.trim()) : [],
        errorHint: document.body.innerText.includes('Maximum update depth'),
      };
    })()`);
    report.cases.invalidDeepLink.errors = report.errors.length;

    // 2) 全库搜索：结果计数在「关于」面板（搜索面板只给语法提示）。
    report.errors.length = 0;
    await cdp.send('Page.navigate', { url: base + '#/?q=pink_hair' });
    await sleep(3500);
    await evaluate(`(() => {
      const tab = document.querySelector('#seam-panel-trigger-about');
      if (tab) tab.click();
      return !!tab;
    })()`);
    await sleep(600);
    report.cases.search = await evaluate(`(() => {
      const panel = document.querySelector('#seam-panel');
      return {
        hints: panel ? [...panel.querySelectorAll('.panel__hint')].map((p) => p.textContent.trim()) : [],
        tiles: document.querySelectorAll('.tile').length,
      };
    })()`);
    report.cases.search.errors = report.errors.length;

    // 3) URL 状态恢复
    report.errors.length = 0;
    await cdp.send('Page.navigate', { url: base + '#/?q=cat_ears&sort=name&mono=1&matte=1' });
    await sleep(1500);
    await evaluate(`document.querySelector('#seam-panel-trigger-search')?.click()`);
    await sleep(400);
    const before = await evaluate(`({
      hash: location.hash,
      mono: document.querySelector('.app')?.classList.contains('is-mono') ?? false,
      query: document.querySelector('input[aria-label^="搜索图片"]')?.value ?? null,
    })`);
    await cdp.send('Page.reload', { ignoreCache: false });
    await sleep(3000);
    await evaluate(`document.querySelector('#seam-panel-trigger-search')?.click()`);
    await sleep(400);
    const after = await evaluate(`({
      hash: location.hash,
      mono: document.querySelector('.app')?.classList.contains('is-mono') ?? false,
      query: document.querySelector('input[aria-label^="搜索图片"]')?.value ?? null,
    })`);
    report.cases.urlState = { before, after, errors: report.errors.length };

    // 4) 小集合标签墙
    report.errors.length = 0;
    await cdp.send('Page.navigate', { url: base + '#/t/rating:e' });
    await sleep(3000);
    report.cases.tinyTag = await evaluate(`(() => {
      const tiles = [...document.querySelectorAll('.tile')];
      const ids = tiles.map((t) => t.getAttribute('data-id')).filter(Boolean);
      return { tiles: tiles.length, ids: ids.length, unique: new Set(ids).size };
    })()`);
    report.cases.tinyTag.errors = report.errors.length;

    const invalid = report.cases.invalidDeepLink;
    const search = report.cases.search;
    const urlState = report.cases.urlState;
    const tiny = report.cases.tinyTag;
    report.passed =
      invalid.missing &&
      invalid.actions.length === 2 &&
      invalid.errors === 0 &&
      !invalid.errorHint &&
      search.hints.some((hint) => hint.includes('筛选后 142 张')) &&
      search.errors === 0 &&
      urlState.after.hash === urlState.before.hash &&
      urlState.after.hash.includes('mono=1') &&
      urlState.after.hash.includes('matte=1') &&
      urlState.after.mono === true &&
      urlState.after.query === 'cat_ears' &&
      tiny.tiles === 1 &&
      tiny.unique === 1 &&
      tiny.errors === 0;
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
