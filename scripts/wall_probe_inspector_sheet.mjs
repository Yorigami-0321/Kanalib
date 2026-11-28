#!/usr/bin/env node
/**
 * 移动端检查器底部 sheet 回归探针：把手可收起/展开，桌面端不受影响。
 *
 * 用法（先在 web/ 里起服务）：
 *   node scripts/wall_probe_inspector_sheet.mjs --base http://127.0.0.1:4273/
 *
 * 断言：
 *  1) 390×844 详情页：默认折叠，`aria-expanded=false`、高度 ≤130px、操作区不可见。
 *  2) 点把手后展开：`aria-expanded=true`、高度 >200px 且 ≤45vh，footer 不被底栏覆盖，
 *     主图可见比例 ≥50%。
 *  3) 再点一次恢复折叠；换图后 sheet 仍折叠；隐藏信息后换图仍隐藏，重新显示时仍折叠。
 *  4) 移动详情 `--seam-h` 解析为 56px，`.view` 底边让出底栏，底栏隐藏全局导航。
 *  5) 桌面 1440×900：把手隐藏（display:none），检查器仍是 300px 左侧栏。
 *  6) 全流程 console error 0。
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

function firstId(dataRoot) {
  try {
    const index = JSON.parse(readFileSync(join(dataRoot, 'index.json'), 'utf8'));
    const shard = JSON.parse(readFileSync(join(dataRoot, index.shards[0].path), 'utf8'));
    return shard.items[0].id;
  } catch {
    return null;
  }
}

async function main() {
  const raw = arg('base', 'http://127.0.0.1:4273/');
  const base = raw.endsWith('/') ? raw : raw + '/';
  const out = arg('out', '');
  const dataRoot = arg('data', 'D:/Code/Kanalib/web/public/data');
  const id = firstId(dataRoot);
  if (!id) {
    console.error('读不到数据 id');
    process.exit(2);
  }
  const chrome = CANDIDATES.find((path) => existsSync(path));
  if (!chrome) {
    console.error('找不到 Chrome/Edge');
    process.exit(2);
  }

  const profile = mkdtempSync(join(tmpdir(), 'kanalib-inspector-'));
  const port = 10100 + Math.floor(Math.random() * 300);
  const child = spawn(
    chrome,
    [
      '--headless=new',
      `--remote-debugging-port=${port}`,
      `--user-data-dir=${profile}`,
      '--no-first-run',
      '--no-default-browser-check',
      '--disable-extensions',
      '--disable-gpu',
      '--no-sandbox',
      '--hide-scrollbars',
      '--window-size=1440,900',
      'about:blank',
    ],
    { stdio: 'ignore' },
  );

  const report = { base, id, cases: {}, consoleErrors: [], passed: false };
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
        report.consoleErrors.push(
          message.params.args.map((item) => item.value ?? item.description ?? '').join(' '),
        );
      }
    });
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    await cdp.send('Emulation.setFocusEmulationEnabled', { enabled: true });

    const evaluate = async (expression) => {
      const result = await cdp.send('Runtime.evaluate', {
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
    };

    const setViewport = async (width, height, mobile) => {
      await cdp.send('Emulation.setDeviceMetricsOverride', {
        width,
        height,
        deviceScaleFactor: 1,
        mobile,
      });
    };

    const measure = () =>
      evaluate(`(() => {
        const aside = document.querySelector('.inspector');
        const handle = document.querySelector('.inspector__handle');
        const actions = document.querySelector('.inspector > .inspector__actions');
        const rail = document.querySelector('.seam.rail');
        const nav = document.querySelector('.seam.rail .seam__nav');
        const view = document.querySelector('.view');
        const rect = aside ? aside.getBoundingClientRect() : null;
        const actionsRect = actions ? actions.getBoundingClientRect() : null;
        const railRect = rail ? rail.getBoundingClientRect() : null;
        const viewRect = view ? view.getBoundingClientRect() : null;
        const handleStyle = handle ? getComputedStyle(handle) : null;
        const railStyle = rail ? getComputedStyle(rail) : null;
        const navStyle = nav ? getComputedStyle(nav) : null;
        const pane = document.querySelectorAll('.view__pane')[1] ?? null;
        const img = pane ? pane.querySelector('.view__img') : null;
        let image = null;
        let visibleRatio = null;
        if (pane && img && img.naturalWidth > 0 && img.naturalHeight > 0) {
          const box = pane.getBoundingClientRect();
          const ratio = img.naturalWidth / img.naturalHeight;
          const renderWidth = Math.min(box.width, box.height * ratio);
          const renderHeight = Math.min(box.height, box.width / ratio);
          const top = box.top + (box.height - renderHeight) / 2;
          image = {
            top: Math.round(top),
            height: Math.round(renderHeight),
            width: Math.round(renderWidth),
          };
          if (rect) {
            const shownBottom = Math.min(top + renderHeight, rect.top);
            visibleRatio = Math.max(0, shownBottom - top) / renderHeight;
          }
        }
        return {
          present: Boolean(aside),
          collapsed: Boolean(aside && aside.classList.contains('inspector--collapsed')),
          expanded: handle ? handle.getAttribute('aria-expanded') : null,
          handleDisplay: handleStyle ? handleStyle.display : null,
          height: rect ? Math.round(rect.height) : null,
          width: rect ? Math.round(rect.width) : null,
          top: rect ? Math.round(rect.top) : null,
          actionsVisible: Boolean(actions && actions.getBoundingClientRect().height > 0),
          actionsBottom: actionsRect ? Math.round(actionsRect.bottom) : null,
          navTop: railRect ? Math.round(railRect.top) : null,
          navDisplay: navStyle ? navStyle.display : null,
          seamHeight: railStyle ? railStyle.height : null,
          viewBottom: viewRect ? Math.round(viewRect.bottom) : null,
          image,
          visibleRatio: visibleRatio === null ? null : Math.round(visibleRatio * 1000) / 1000,
          title: document.querySelector('.inspector__title')?.textContent?.trim() ?? null,
        };
      })()`);

    const waitImage = async (timeoutMs = 8000) => {
      const deadline = Date.now() + timeoutMs;
      while (Date.now() < deadline) {
        const state = await measure();
        if (state.image) return state;
        await sleep(200);
      }
      throw new Error('详情主图没有加载');
    };

    const waitInspector = async (timeoutMs = 12000) => {
      const deadline = Date.now() + timeoutMs;
      while (Date.now() < deadline) {
        const state = await measure();
        if (state.present) return state;
        await sleep(250);
      }
      throw new Error('检查器没有出现');
    };

    // ---- 移动端 ----
    await setViewport(390, 844, true);
    await cdp.send('Page.navigate', { url: base + '#/d/' + id });
    await sleep(900);
    const open = await waitInspector();
    await sleep(400);
    const initiallyCollapsed = await waitImage();
    await evaluate(`document.querySelector('.inspector__handle')?.click()`);
    await sleep(500);
    const expanded = await waitImage();
    await evaluate(`document.querySelector('.inspector__handle')?.click()`);
    await sleep(500);
    const collapsedAgain = await measure();

    const clickNav = () =>
      evaluate(`(() => {
        const buttons = [...document.querySelectorAll('.seam__ops--detail button')];
        const next = buttons.find((b) => (b.title || '').startsWith('下一张') && !b.disabled);
        const prev = buttons.find((b) => (b.title || '').startsWith('上一张') && !b.disabled);
        const pick = next || prev;
        if (!pick) return null;
        const direction = pick === next ? 'next' : 'prev';
        pick.click();
        return direction;
      })()`);

    const navDirection = await clickNav();
    await sleep(1200);
    const afterNav = await measure();

    const hidInfo = await evaluate(`(() => {
      const button = [...document.querySelectorAll('.seam__ops--detail button')]
        .find((b) => (b.title || '').startsWith('收起信息'));
      button?.click();
      return Boolean(button);
    })()`);
    await sleep(600);
    const infoHidden = await measure();
    const navAgain = await clickNav();
    await sleep(1200);
    const afterHiddenNav = await measure();
    await evaluate(`(() => {
      const button = [...document.querySelectorAll('.seam__ops--detail button')]
        .find((b) => (b.title || '').startsWith('显示信息'));
      button?.click();
      return Boolean(button);
    })()`);
    await sleep(800);
    const infoRestored = await measure();

    report.cases.mobile = {
      open,
      initiallyCollapsed,
      expanded,
      collapsedAgain,
      navDirection,
      afterNav,
      hidInfo,
      infoHidden,
      navAgain,
      afterHiddenNav,
      infoRestored,
    };

    // ---- 桌面 ----
    await setViewport(1440, 900, false);
    await cdp.send('Page.navigate', { url: base + '#/d/' + id });
    await sleep(900);
    const desktopOpen = await waitInspector();
    await sleep(300);
    report.cases.desktop = await measure();

    const m = report.cases.mobile;
    const d = report.cases.desktop;
    report.passed =
      report.consoleErrors.length === 0 &&
      m.initiallyCollapsed.expanded === 'false' &&
      m.initiallyCollapsed.collapsed === true &&
      m.initiallyCollapsed.height > 0 &&
      m.initiallyCollapsed.height <= 130 &&
      m.initiallyCollapsed.actionsVisible === false &&
      m.expanded.expanded === 'true' &&
      !m.expanded.collapsed &&
      m.expanded.height > 200 &&
      m.expanded.height <= Math.round(844 * 0.45) &&
      m.expanded.actionsVisible === true &&
      m.expanded.actionsBottom <= m.expanded.navTop + 1 &&
      m.expanded.visibleRatio >= 0.5 &&
      m.collapsedAgain.expanded === 'false' &&
      m.collapsedAgain.collapsed === true &&
      m.collapsedAgain.height <= 130 &&
      m.collapsedAgain.actionsVisible === false &&
      m.initiallyCollapsed.seamHeight === '56px' &&
      m.initiallyCollapsed.navDisplay === 'none' &&
      m.initiallyCollapsed.viewBottom <= m.initiallyCollapsed.navTop + 1 &&
      m.navDirection !== null &&
      m.afterNav.title !== m.collapsedAgain.title &&
      m.afterNav.expanded === 'false' &&
      m.afterNav.collapsed === true &&
      m.hidInfo === true &&
      m.infoHidden.present === false &&
      m.navAgain !== null &&
      m.afterHiddenNav.present === false &&
      m.infoRestored.present === true &&
      m.infoRestored.expanded === 'false' &&
      m.infoRestored.collapsed === true &&
      d.handleDisplay === 'none' &&
      d.width === 300 &&
      d.height > 500 &&
      d.top === 0;
  } finally {
    if (cdp) cdp.close();
    try {
      child.kill();
    } catch {
      /* 已退出 */
    }
    hardDelete(profile);
  }

  if (out) writeFileSync(out, JSON.stringify(report, null, 2));
  console.log(
    JSON.stringify({
      passed: report.passed,
      mobile: report.cases.mobile?.initiallyCollapsed,
      desktop: report.cases.desktop,
      errors: report.consoleErrors,
    }),
  );
  process.exit(report.passed ? 0 : 1);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
