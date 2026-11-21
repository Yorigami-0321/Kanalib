#!/usr/bin/env node
/**
 * 冷启动搜索探针：首屏 URL 就带 q 时，搜索索引请求必须等 init() 落定后再发。
 *
 * 用法（先起静态服务）：
 *   node scripts/wall_probe_cold_search.mjs --base http://127.0.0.1:4273/ --out report.json
 *
 * 断言：
 *  1) 冷缓存直接开 `#/?q=pink_hair`：筛选后 142 张，搜索提示 ready，不出现「搜索索引不可用」，
 *     search-index.json 请求 1–2 次，console error 0。
 *  2) 同页刷新：结果与冷开一致，请求 1–2 次。
 *  3) 复制链接新标签打开：结果与冷开一致（同 URL、同计数、ready）。
 *  4) 索引 404：静默退回文件名搜索（提示「搜索索引不可用」），不崩、console error 0。
 *  5) 索引 JSON 损坏：同上，静默降级。
 *
 * 退出码：0 通过；1 断言失败；2 环境问题。
 */
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as sleep } from 'node:timers/promises';

const CANDIDATES = [
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
  'C:/Program Files/Microsoft/Edge/Application/msedge.exe',
];

const COLD_HASH = '#/?q=pink_hair';
const EXPECT_COUNT = 142;
const READY_HINT = '名称 / 标签 / 分组 / 分级均参与';
const UNAVAILABLE_HINT = '搜索索引不可用';

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

/** 一个页面会话：统计 console 错误、search-index 请求次数，支持拦截降级模式。 */
class Session {
  constructor(cdp) {
    this.cdp = cdp;
    this.errors = [];
    this.searchReqs = [];
    this.intercept = null; // null | '404' | 'corrupt'
  }
  async attach({ width = 1440, height = 900 } = {}) {
    await this.cdp.send('Page.enable');
    await this.cdp.send('Runtime.enable');
    await this.cdp.send('Network.enable');
    await this.cdp.send('Emulation.setDeviceMetricsOverride', {
      width,
      height,
      deviceScaleFactor: 1,
      mobile: false,
    });
    this.cdp.on((message) => {
      if (message.method === 'Runtime.exceptionThrown') {
        this.errors.push('exception: ' + (message.params.exceptionDetails?.text ?? 'unknown'));
        return;
      }
      if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
        this.errors.push(
          message.params.args.map((entry) => entry.value ?? entry.description ?? '').join(' '),
        );
        return;
      }
      if (message.method === 'Network.requestWillBeSent') {
        const url = message.params.request.url;
        if (url.includes('search-index.json')) this.searchReqs.push(url);
        return;
      }
      if (message.method === 'Fetch.requestPaused') {
        const mode = this.intercept;
        const responseHeaders = [{ name: 'content-type', value: 'application/json' }];
        let responseCode = 200;
        let body = 'not-json';
        if (mode === '404') {
          responseCode = 404;
          body = '{"error":"missing"}';
        }
        void this.cdp
          .send('Fetch.fulfillRequest', {
            requestId: message.params.requestId,
            responseCode,
            responseHeaders,
            body: Buffer.from(body, 'utf8').toString('base64'),
          })
          .catch(() => undefined);
      }
    });
  }
  async evaluate(expression) {
    const result = await this.cdp.send('Runtime.evaluate', {
      expression,
      returnByValue: true,
      awaitPromise: true,
    });
    if (result.exceptionDetails) {
      throw new Error(
        '页面脚本异常：' +
          (result.exceptionDetails.exception?.description ?? result.exceptionDetails.text),
      );
    }
    return result.result?.value;
  }
  /** 打开「关于」读计数，再打开「搜索」读索引状态；最后把面板关掉。 */
  async readPanels() {
    await this.evaluate(`(() => {
      const about = document.querySelector('#seam-panel-trigger-about');
      if (about) about.click();
      return !!about;
    })()`);
    await sleep(220);
    const aboutHints = await this.evaluate(
      `[...document.querySelectorAll('#seam-panel .panel__hint')].map((p) => p.textContent.trim())`,
    );
    await this.evaluate(`document.querySelector('#seam-panel-trigger-about')?.click()`);
    await sleep(120);
    await this.evaluate(`document.querySelector('#seam-panel-trigger-search')?.click()`);
    await sleep(220);
    const searchHints = await this.evaluate(
      `[...document.querySelectorAll('#seam-panel .panel__hint')].map((p) => p.textContent.trim())`,
    );
    await this.evaluate(`document.querySelector('#seam-panel-trigger-search')?.click()`);
    await sleep(120);
    return {
      aboutHints: Array.isArray(aboutHints) ? aboutHints : [],
      searchHints: Array.isArray(searchHints) ? searchHints : [],
    };
  }
  async waitPanels(predicate, timeoutMs = 12000) {
    const deadline = Date.now() + timeoutMs;
    let panels = null;
    while (Date.now() < deadline) {
      panels = await this.readPanels();
      if (predicate(panels)) return panels;
      await sleep(300);
    }
    return panels;
  }
  async waitSlideSettled(extraMs = 400) {
    await sleep(extraMs);
  }
}

function countHint(panels) {
  const joined = panels.aboutHints.join(' ');
  const match = joined.match(/筛选后\s*([\d,]+)\s*张/);
  return match ? Number(match[1].replace(/,/g, '')) : null;
}

function searchState(panels) {
  const joined = panels.searchHints.join(' ');
  if (joined.includes(UNAVAILABLE_HINT)) return 'unavailable';
  if (joined.includes(READY_HINT)) return 'ready';
  if (joined.includes('搜索索引加载中')) return 'loading';
  return 'filename-only';
}

async function main() {
  const raw = arg('base', 'http://127.0.0.1:4273/');
  const base = raw.endsWith('/') ? raw : raw + '/';
  const out = arg('out', '');
  const chrome = CANDIDATES.find((path) => existsSync(path));
  if (!chrome) {
    console.error('找不到 Chrome/Edge');
    process.exit(2);
  }

  const profile = mkdtempSync(join(tmpdir(), 'kanalib-cold-search-'));
  const port = 10200 + Math.floor(Math.random() * 300);
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

  const report = {
    base,
    expectCount: EXPECT_COUNT,
    cases: {},
    consoleErrors: [],
    passed: false,
    verifiedAt: new Date().toISOString(),
  };
  const sessions = [];
  try {
    await waitForDevtools(port);

    const openSession = async () => {
      const target = await newTarget(port, 'about:blank');
      const cdp = new Cdp(target.webSocketDebuggerUrl);
      await cdp.connect();
      const session = new Session(cdp);
      await session.attach();
      sessions.push(session);
      return session;
    };

    const navigate = async (session, url) => {
      session.searchReqs.length = 0;
      session.errors.length = 0;
      await session.cdp.send('Page.navigate', { url });
    };

    const assertCold = async (session, label) => {
      // 搜索索引就绪前计数会短暂显示 0，必须等它变成期望值再断言。
      const panels = await session.waitPanels((entry) => countHint(entry) === EXPECT_COUNT);
      const state = searchState(panels);
      const hint = countHint(panels);
      const result = {
        hash: await session.evaluate('location.hash'),
        hint,
        state,
        searchRequests: session.searchReqs.length,
        errors: [...session.errors],
        depth: await session.evaluate(`document.body.innerText.includes('Maximum update depth')`),
      };
      result.pass =
        hint === EXPECT_COUNT &&
        state === 'ready' &&
        result.searchRequests >= 1 &&
        result.searchRequests <= 2 &&
        result.errors.length === 0 &&
        !result.depth;
      report.cases[label] = result;
      return result;
    };

    // 1) 冷缓存直接打开带 q 的深链（新 profile 的第一次导航）。
    const cold = await openSession();
    await navigate(cold, base + COLD_HASH);
    const coldCase = await assertCold(cold, 'coldDeepLink');

    // 2) 同页刷新：hash 不变，结果应一致。
    cold.searchReqs.length = 0;
    cold.errors.length = 0;
    await cold.cdp.send('Page.reload', { ignoreCache: true });
    const refresh = await assertCold(cold, 'refresh');
    refresh.hashStable = refresh.hash === COLD_HASH;
    refresh.pass = refresh.pass && refresh.hashStable;

    // 3) 复制链接新标签打开。
    const tab = await openSession();
    await navigate(tab, base + COLD_HASH);
    const newTab = await assertCold(tab, 'newTab');
    newTab.sameAsCold = newTab.hint === coldCase.hint;
    newTab.pass = newTab.pass && newTab.sameAsCold;

    // 4) 索引 404：静默降级。
    await cold.cdp.send('Fetch.enable', {
      patterns: [{ urlPattern: '*search-index.json*', requestStage: 'Request' }],
    });
    cold.intercept = '404';
    cold.searchReqs.length = 0;
    cold.errors.length = 0;
    await cold.cdp.send('Page.reload', { ignoreCache: true });
    const notFound = await cold.waitPanels((entry) => searchState(entry) === 'unavailable');
    report.cases.index404 = {
      state: searchState(notFound),
      hints: notFound.searchHints,
      searchRequests: cold.searchReqs.length,
      errors: [...cold.errors],
      depth: await cold.evaluate(`document.body.innerText.includes('Maximum update depth')`),
    };
    report.cases.index404.pass =
      report.cases.index404.state === 'unavailable' &&
      report.cases.index404.errors.length === 0 &&
      !report.cases.index404.depth;

    // 5) 索引 JSON 损坏：同样静默降级。
    cold.intercept = 'corrupt';
    cold.searchReqs.length = 0;
    cold.errors.length = 0;
    await cold.cdp.send('Page.reload', { ignoreCache: true });
    const corrupt = await cold.waitPanels((entry) => searchState(entry) === 'unavailable');
    report.cases.indexCorrupt = {
      state: searchState(corrupt),
      hints: corrupt.searchHints,
      searchRequests: cold.searchReqs.length,
      errors: [...cold.errors],
      depth: await cold.evaluate(`document.body.innerText.includes('Maximum update depth')`),
    };
    report.cases.indexCorrupt.pass =
      report.cases.indexCorrupt.state === 'unavailable' &&
      report.cases.indexCorrupt.errors.length === 0 &&
      !report.cases.indexCorrupt.depth;
    await cold.cdp.send('Fetch.disable');
    cold.intercept = null;

    report.consoleErrors = [
      ...new Set(
        Object.values(report.cases).flatMap((entry) => (Array.isArray(entry.errors) ? entry.errors : [])),
      ),
    ];
    report.passed =
      report.cases.coldDeepLink.pass &&
      report.cases.refresh.pass &&
      report.cases.newTab.pass &&
      report.cases.index404.pass &&
      report.cases.indexCorrupt.pass &&
      report.consoleErrors.length === 0;
  } finally {
    for (const session of sessions) session.cdp.close();
    try {
      child.kill();
    } catch {
      /* 已退出 */
    }
    hardDelete(profile);
  }

  const text = JSON.stringify(report, null, 2);
  if (out) writeFileSync(out, text, 'utf8');
  console.log(text);
  process.exit(report.passed ? 0 : 1);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
