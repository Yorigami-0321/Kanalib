#!/usr/bin/env node
/**
 * 详情内翻页回归探针：URL 与画面必须同步。
 *
 * 背景：`replaceNavigate` 走 `history.replaceState`，不会触发 hashchange；
 * 若路由只订阅 hashchange，详情内 ←/→、侧栏上下张、自动放映、相似条点击
 * 都会出现「地址换了、画面没换」。本探针把「→ 后 500ms 画面变化」固化成断言。
 *
 * 用法（先在 web/ 里起服务）：
 *   node scripts/wall_probe_detail_nav.mjs --base http://127.0.0.1:4288/
 *
 * 断言：
 *  1) 墙 → 双击进详情：hash 带 q/sort/mono/matte。
 *  2) → 后 500ms 内标题/计数变化、有 pane 切换；hash 与画面同一 id。
 *  3) ← 回到上一张，同口径。
 *  4) 侧栏「下一张 / 上一张」同口径。
 *  5) 自动放映 P：约 4.2s 后画面变化并跟随 hash，再暂停。
 *  6) 检查器「相似 / 同标签」缩略图：URL 与画面都切到目标 id。
 *  7) 翻页后一次浏览器后退直接回列表；刷新详情深链恢复同一张。
 *  8) 全流程 console error 0、无 Maximum update depth、翻页不再拉分片。
 *
 * 退出码：0 通过；1 断言失败；2 环境问题。
 */
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
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

/** 数据侧只用来把 hash 里的 id 翻成标题，判断「URL 与画面同一张」。 */
function loadNames() {
  const dataRoot = arg('data', 'D:/Code/Kanalib/web/public/data');
  try {
    const index = JSON.parse(readFileSync(join(dataRoot, 'index.json'), 'utf8'));
    const names = new Map();
    for (const shard of index.shards) {
      const payload = JSON.parse(readFileSync(join(dataRoot, shard.path), 'utf8'));
      for (const item of payload.items) names.set(item.id, item.n);
    }
    return names;
  } catch {
    return new Map();
  }
}

async function main() {
  const raw = arg('base', 'http://127.0.0.1:4288/');
  const base = raw.endsWith('/') ? raw : raw + '/';
  const out = arg('out', '');
  const chrome = CANDIDATES.find((path) => existsSync(path));
  if (!chrome) {
    console.error('找不到 Chrome/Edge');
    process.exit(2);
  }
  const names = loadNames();

  const profile = mkdtempSync(join(tmpdir(), 'kanalib-detailnav-'));
  const port = 9600 + Math.floor(Math.random() * 300);
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

  const report = { base, checks: {}, consoleErrors: [], passed: false, verifiedAt: new Date().toISOString() };
  let cdp = null;
  try {
    await waitForDevtools(port);
    const target = await newTarget(port, 'about:blank');
    cdp = new Cdp(target.webSocketDebuggerUrl);
    await cdp.connect();
    cdp.on((message) => {
      if (message.method === 'Runtime.exceptionThrown') {
        report.consoleErrors.push('exception: ' + (message.params.exceptionDetails?.text ?? 'unknown'));
      } else if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
        report.consoleErrors.push(message.params.args.map((item) => item.value ?? item.description ?? '').join(' '));
      }
    });
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    await cdp.send('Log.enable');
    await cdp.send('Page.bringToFront');
    await cdp.send('Emulation.setDeviceMetricsOverride', {
      width: 1440,
      height: 900,
      deviceScaleFactor: 1,
      mobile: false,
    });

    const evaluate = async (expression) => {
      const result = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
      if (result.exceptionDetails) {
        throw new Error('页面脚本异常：' + (result.exceptionDetails.exception?.description ?? result.exceptionDetails.text));
      }
      return result.result?.value;
    };

    const viewState = () =>
      evaluate(`(() => {
        const article = document.querySelector('.view.view--detail');
        const track = document.querySelector('.view__track');
        return {
          detail: Boolean(article),
          label: article ? article.getAttribute('aria-label') : null,
          hash: location.hash,
          hashId: /^#\\/d\\/([^?]+)/.exec(location.hash)?.[1] ?? null,
          count: document.querySelector('.seam__ops--detail .seam__count')?.textContent?.trim() ?? null,
          trackClass: track ? track.className : null,
        };
      })()`);

    /** 轮询等画面切换到「不是 prevLabel」的那一张；同时记录是否出现 pane 滑动。 */
    const waitViewChange = async (prevLabel, timeout = 1400) => {
      const started = Date.now();
      let slideSeen = false;
      let changed = null;
      let changedAt = 0;
      while (Date.now() - started < timeout) {
        const state = await viewState();
        if (state.trackClass && /view__track--(next|prev)/.test(state.trackClass)) slideSeen = true;
        if (!changed && state.label && state.label !== prevLabel) {
          changed = state;
          changedAt = Date.now() - started;
        }
        // 标签在 React 提交时就变了，轨道动画类可能晚一拍才挂上；
        // 命中变化后继续采样 ~160ms，别把「动画还没开始」误判成「没有动画」。
        // ms 仍记首次观察到换图的时间，不受加固采样影响。
        if (changed && (slideSeen || Date.now() - started >= changedAt + 160)) {
          return { ms: changedAt, state: changed, slideSeen };
        }
        await sleep(10);
      }
      return {
        ms: changed ? changedAt : -1,
        state: changed ?? (await viewState()),
        slideSeen,
      };
    };

    /** 滑动动画期间 Detail 不收新指令；等轨道类名回到常态再发下一次。 */
    const waitSlideDone = async (timeout = 2000) => {
      const started = Date.now();
      while (Date.now() - started < timeout) {
        const trackClass = await evaluate("document.querySelector('.view__track')?.className ?? ''");
        if (!/view__track--(next|prev)/.test(trackClass)) return true;
        await sleep(30);
      }
      return false;
    };

    const key = async (name, code, vk) => {
      await evaluate(
        `window.dispatchEvent(new KeyboardEvent('keydown', { key: ${JSON.stringify(name)}, code: ${JSON.stringify(code)}, windowsVirtualKeyCode: ${vk}, bubbles: true }));` +
          `window.dispatchEvent(new KeyboardEvent('keyup', { key: ${JSON.stringify(name)}, code: ${JSON.stringify(code)}, windowsVirtualKeyCode: ${vk}, bubbles: true })); true`,
      );
    };

    const clickByTitle = (prefix) =>
      evaluate(`(() => {
        const button = [...document.querySelectorAll('button')].find((node) => (node.getAttribute('title') ?? '').startsWith(${JSON.stringify(prefix)}));
        button?.click();
        return Boolean(button);
      })()`);

    // 1) 墙（带完整浏览状态）→ 聚焦第一张卡 → Enter 进详情。
    //    先落首页再改 hash：搜索索引在 init 完成后才可靠，冷启动深链带 q 的竞态
    //    不在本探针职责内（另行报告），这里要验证的是翻页路由事件。
    await cdp.send('Page.navigate', { url: base + '#/' });
    const shellDeadline = Date.now() + 20000;
    while (Date.now() < shellDeadline) {
      const ready = await evaluate("(document.querySelector('.seam__foot .seam__count')?.textContent ?? '').includes('937 / 937')");
      if (ready) break;
      await sleep(150);
    }
    await evaluate("location.hash = '#/?q=pink_hair&sort=name&mono=1&matte=1'");
    const tileDeadline = Date.now() + 8000;
    while (Date.now() < tileDeadline) {
      if (await evaluate("document.querySelectorAll('.tile').length > 0")) break;
      await sleep(100);
    }
    await sleep(400);
    // 卡片是 pointerup + 360ms 双击判定，不是原生 dblclick；走键盘路径最稳：
    // 聚焦第一张卡 → Enter。
    const focused = await evaluate(`(() => {
      const tile = document.querySelector('.tile');
      tile?.focus();
      return Boolean(tile);
    })()`);
    if (!focused) throw new Error('墙上没有可聚焦的卡片');
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
    await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
    const openDeadline = Date.now() + 5000;
    while (Date.now() < openDeadline) {
      if ((await viewState()).detail) break;
      await sleep(50);
    }
    const opened = await viewState();
    const params = { q: /(?:^|[?&])q=pink_hair(?:&|$)/.test(opened.hash), sort: /(?:^|[?&])sort=name(?:&|$)/.test(opened.hash), mono: /(?:^|[?&])mono=1(?:&|$)/.test(opened.hash), matte: /(?:^|[?&])matte=1(?:&|$)/.test(opened.hash) };
    report.checks.enterDetail = { detail: opened.detail, label: opened.label, hash: opened.hash, params, pass: opened.detail && Object.values(params).every(Boolean) };

    // 2) →：500ms 内画面换图，hash 与画面同一个 id
    const beforeRight = await viewState();
    const shardsBefore = await evaluate("performance.getEntriesByType('resource').filter((entry) => entry.name.includes('/data/m/')).length");
    await key('ArrowRight', 'ArrowRight', 39);
    const right = await waitViewChange(beforeRight.label);
    await sleep(250);
    await waitSlideDone();
    const rightSettled = await viewState();
    const rightExpected = rightSettled.hashId ? '图片详情：' + (names.get(decodeURIComponent(rightSettled.hashId)) ?? '') : null;
    report.checks.arrowRight = {
      ms: right.ms,
      slideSeen: right.slideSeen,
      labelChanged: Boolean(right.state.label) && right.state.label !== beforeRight.label,
      countChanged: right.state.count !== beforeRight.count,
      hashId: rightSettled.hashId,
      labelMatchesHash: rightExpected ? right.state.label === rightExpected : null,
      paramsKept: ['q=pink_hair', 'sort=name', 'mono=1', 'matte=1'].every((part) => rightSettled.hash.includes(part)),
      under500ms: right.ms > 0 && right.ms <= 500,
      pass: right.ms > 0 && right.ms <= 500 && right.slideSeen && right.state.label !== beforeRight.label && right.state.count !== beforeRight.count && rightExpected === right.state.label && ['q=pink_hair', 'sort=name', 'mono=1', 'matte=1'].every((part) => rightSettled.hash.includes(part)),
    };

    // 3) ←：回到上一张
    await waitSlideDone();
    const beforeLeft = await viewState();
    await key('ArrowLeft', 'ArrowLeft', 37);
    const left = await waitViewChange(beforeLeft.label);
    const leftSettled = await viewState();
    report.checks.arrowLeft = {
      ms: left.ms,
      slideSeen: left.slideSeen,
      label: left.state.label,
      restored: left.state.label === beforeRight.label && left.state.hashId === beforeRight.hashId,
      under500ms: left.ms > 0 && left.ms <= 500,
      pass: left.ms > 0 && left.ms <= 500 && left.slideSeen && left.state.label === beforeRight.label && left.state.hashId === beforeRight.hashId,
    };
    void leftSettled;

    // 4) 侧栏下一张 / 上一张
    const beforeSidebar = await viewState();
    await clickByTitle('下一张');
    const sidebarNext = await waitViewChange(beforeSidebar.label);
    await sleep(200);
    const afterSidebarNext = await viewState();
    await waitSlideDone();
    await clickByTitle('上一张');
    const sidebarPrev = await waitViewChange(afterSidebarNext.label);
    await sleep(200);
    const afterSidebarPrev = await viewState();
    report.checks.sidebar = {
      nextMs: sidebarNext.ms,
      prevMs: sidebarPrev.ms,
      nextSlide: sidebarNext.slideSeen,
      prevSlide: sidebarPrev.slideSeen,
      restored: afterSidebarPrev.label === beforeSidebar.label,
      pass:
        sidebarNext.ms > 0 &&
        sidebarNext.ms <= 500 &&
        sidebarPrev.ms > 0 &&
        sidebarPrev.ms <= 500 &&
        sidebarNext.slideSeen &&
        sidebarPrev.slideSeen &&
        afterSidebarPrev.label === beforeSidebar.label,
    };

    // 5) 自动放映：约 4.2s 一跳，暂停后不再跳
    const beforePlay = await viewState();
    await clickByTitle('自动放映');
    const play = await waitViewChange(beforePlay.label, 6500);
    await waitSlideDone();
    await clickByTitle('暂停放映');
    const paused = await viewState();
    await sleep(1200);
    const afterPause = await viewState();
    report.checks.autoplay = {
      firstJumpMs: play.ms,
      label: play.state.label,
      slideSeen: play.slideSeen,
      pausedHashStable: paused.hash === afterPause.hash && paused.label === afterPause.label,
      pass: play.ms > 3000 && play.ms < 6000 && play.state.label !== beforePlay.label && paused.hash === afterPause.hash && paused.label === afterPause.label,
    };

    // 6) 检查器「相似 / 同标签」缩略图：URL 与画面都切到目标
    let similarClick = { found: false, attempts: 0, ms: -1, pass: false };
    for (let attempt = 0; attempt < 6; attempt += 1) {
      const present = await evaluate("Boolean(document.querySelector('.inspector__similar-item'))");
      if (present) {
        const before = await viewState();
        const targetName = await evaluate("document.querySelector('.inspector__similar-item')?.getAttribute('title') ?? ''");
        await evaluate("document.querySelector('.inspector__similar-item')?.click()");
        const change = await waitViewChange(before.label);
        await sleep(250);
        await waitSlideDone();
        const settled = await viewState();
        similarClick = {
          found: true,
          attempts: attempt + 1,
          targetName,
          ms: change.ms,
          label: change.state.label,
          hashId: settled.hashId,
          labelMatches: change.state.label === '图片详情：' + targetName,
          pass: change.ms > 0 && change.ms <= 500 && change.state.label === '图片详情：' + targetName,
        };
        break;
      }
      similarClick.attempts = attempt + 1;
      await key('ArrowRight', 'ArrowRight', 39);
      await waitViewChange((await viewState()).label);
      await sleep(300);
    }
    report.checks.similar = similarClick;

    // 7) 一次后退回列表
    await evaluate('history.back()');
    const backDeadline = Date.now() + 5000;
    while (Date.now() < backDeadline) {
      if (!(await viewState()).detail) break;
      await sleep(50);
    }
    const back = await viewState();
    report.checks.backToList = {
      detail: back.detail,
      hash: back.hash,
      paramsKept: ['q=pink_hair', 'sort=name', 'mono=1', 'matte=1'].every((part) => back.hash.includes(part)),
      pass: !back.detail && ['q=pink_hair', 'sort=name', 'mono=1', 'matte=1'].every((part) => back.hash.includes(part)),
    };

    // 8) 刷新当前详情深链：恢复同一张
    const targetHash = rightSettled.hash;
    await cdp.send('Page.navigate', { url: base + targetHash });
    await sleep(2500);
    const reloaded = await viewState();
    report.checks.reloadDetail = {
      hash: reloaded.hash,
      label: reloaded.label,
      expected: '图片详情：' + (names.get(decodeURIComponent(targetHash.replace(/^#\/d\//, '').split('?')[0])) ?? ''),
      pass: reloaded.detail && reloaded.label?.startsWith('图片详情：') && reloaded.label === '图片详情：' + (names.get(decodeURIComponent(targetHash.replace(/^#\/d\//, '').split('?')[0])) ?? ''),
    };

    // 9) 翻页不拉分片（findItem 不风暴）
    const shardsAfter = await evaluate("performance.getEntriesByType('resource').filter((entry) => entry.name.includes('/data/m/')).length");
    report.checks.noShardStorm = {
      shardsBefore,
      shardsAfter,
      pass: shardsAfter <= shardsBefore,
    };

    report.checks.depth = await evaluate("document.body.innerText.includes('Maximum update depth')");
    report.passed =
      Object.values(report.checks).every((check) => check === false || check?.pass !== false) &&
      report.checks.enterDetail.pass &&
      report.checks.arrowRight.pass &&
      report.checks.arrowLeft.pass &&
      report.checks.sidebar.pass &&
      report.checks.autoplay.pass &&
      report.checks.similar.found &&
      report.checks.similar.pass &&
      report.checks.backToList.pass &&
      report.checks.reloadDetail.pass &&
      report.checks.noShardStorm.pass &&
      !report.checks.depth &&
      report.consoleErrors.length === 0;
  } catch (error) {
    report.error = String(error && error.stack ? error.stack : error);
    report.passed = false;
  } finally {
    if (cdp) cdp.close();
    child.kill();
    await sleep(200);
    hardDelete(profile);
  }

  const text = JSON.stringify(report, null, 2);
  if (out) writeFileSync(out, text);
  console.log(text);
  process.exit(report.passed ? 0 : 1);
}

main().catch((error) => {
  console.error(error);
  process.exit(2);
});
