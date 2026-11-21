#!/usr/bin/env node
/**
 * 边界态回归探针：未知 hash 缺失页 + 各空态动作。
 *
 * 用法（先在 web/ 里起 preview）：
 *   node scripts/wall_probe_edge.mjs --base http://127.0.0.1:4281/ --out web/screenshots/b-edge
 *
 * 断言：
 *  1) `#/definitely-not-a-route`：出现缺失页（标题 + 两个动作），不渲染画布，console error 0。
 *  2) 缺失页「回首页」可用键盘 Enter 触发，落到 `#/` 且出现卡片；后退回到缺失页，前进回首页。
 *  3) 缺失页「去搜索」：回首页并打开搜索面板，输入框获得焦点。
 *  4) 无匹配筛选：`.wall__note` 出现「清空搜索」，点击后 q 从 URL 消失、卡片回来。
 *  5) 空标签/分级/分组/收藏：各有动作，点击后到达对应总览（或首页）。
 *  6) 有效路由（#/tags、#/t/pink_hair）不受影响。
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
      if (response.ok) return;
    } catch {
      /* 还没起 */
    }
    await sleep(120);
  }
  throw new Error('DevTools 端口未就绪');
}

async function newTarget(port) {
  const response = await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, { method: 'PUT' });
  return response.json();
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
  const raw = arg('base', 'http://127.0.0.1:4281/');
  const base = raw.endsWith('/') ? raw : raw + '/';
  const outDir = resolve(arg('out', 'web/screenshots/b-edge'));
  mkdirSync(outDir, { recursive: true });
  const chrome = CANDIDATES.find((path) => existsSync(path));
  if (!chrome) {
    console.error('找不到 Chrome/Edge');
    process.exit(2);
  }

  const profile = mkdtempSync(join(tmpdir(), 'kanalib-edge-'));
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

  const report = { base, verifiedAt: new Date().toISOString(), errors: [], cases: {}, passed: false };
  let cdp = null;
  try {
    await waitForDevtools(port);
    const target = await newTarget(port);
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
    // headless 下页面默认不算“有焦点”，键盘默认动作（按钮 Enter 激活）要靠它开启。
    await cdp.send('Emulation.setFocusEmulationEnabled', { enabled: true });
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

    const goto = async (hash, waitMs = 2600) => {
      await evaluate(`location.hash = ${JSON.stringify(hash)}`);
      await sleep(waitMs);
    };
    const shot = async (name) => {
      const { data } = await cdp.send('Page.captureScreenshot', { format: 'png' });
      writeFileSync(join(outDir, name + '.png'), Buffer.from(data, 'base64'));
    };
    const clickText = async (selector, text) =>
      evaluate(`(() => {
        const node = [...document.querySelectorAll(${JSON.stringify(selector)})].find(
          (entry) => entry.textContent.trim() === ${JSON.stringify(text)},
        );
        if (!node) return false;
        node.click();
        return true;
      })()`);
    const state = async () =>
      evaluate(`(() => ({
        hash: location.hash,
        tiles: document.querySelectorAll('.tile').length,
        note: !!document.querySelector('.wall__note'),
        noteButtons: [...document.querySelectorAll('.wall__note-actions button')].map((n) => n.textContent.trim()),
        bucketButtons: [...document.querySelectorAll('.tags__empty-actions button')].map((n) => n.textContent.trim()),
        missing: !!document.querySelector('.stage__missing'),
        missingTitle: document.querySelector('.stage__missing-title')?.textContent.trim() ?? null,
        missingActions: [...document.querySelectorAll('.stage__missing-actions button')].map((n) => n.textContent.trim()),
        allButtons: [...document.querySelectorAll('.stage__missing-actions button')].every(
          (n) => n.tagName === 'BUTTON' && n.type === 'button',
        ),
        panel: !!document.querySelector('#seam-panel'),
        focusTag: document.activeElement?.tagName ?? null,
        focusPlaceholder: document.activeElement?.getAttribute?.('placeholder') ?? null,
      }))()`);

    await cdp.send('Page.navigate', { url: base });
    await sleep(3000);

    // 1) 未知 hash：缺失页 + 两个动作，不渲染画布
    report.errors.length = 0;
    await goto('#/definitely-not-a-route', 1800);
    const notFound = await state();
    await shot('edge-404-unknown-hash');
    report.cases.notFound = { ...notFound, errors: report.errors.length };

    // 2) 键盘 Enter 触发「回首页」，再验证后退/前进
    await evaluate(`document.querySelector('.stage__missing-actions button')?.focus()`);
    const focusedBefore = await evaluate('document.activeElement?.textContent?.trim() ?? null');
    // 原生按钮的 Enter 激活在 keydown 默认动作里；补 text 让 headless 走完整输入链路。
    await cdp.send('Input.dispatchKeyEvent', {
      type: 'keyDown',
      key: 'Enter',
      code: 'Enter',
      text: '\r',
      unmodifiedText: '\r',
      windowsVirtualKeyCode: 13,
      nativeVirtualKeyCode: 13,
    });
    await cdp.send('Input.dispatchKeyEvent', {
      type: 'keyUp',
      key: 'Enter',
      code: 'Enter',
      windowsVirtualKeyCode: 13,
      nativeVirtualKeyCode: 13,
    });
    await sleep(1400);
    const afterEnter = await state();
    await evaluate('history.back()');
    await sleep(1200);
    const afterBack = await state();
    await evaluate('history.forward()');
    await sleep(1200);
    const afterForward = await state();
    report.cases.keyboardAndHistory = {
      focusedBefore,
      afterEnter,
      afterBack,
      afterForward,
      errors: report.errors.length,
    };

    // 3) 去搜索：回首页并打开侧栏搜索面板
    report.errors.length = 0;
    await goto('#/definitely-not-a-route', 800);
    await clickText('.stage__missing-actions button', '去搜索');
    await sleep(1400);
    report.cases.goSearch = { ...(await state()), errors: report.errors.length };
    await shot('edge-404-go-search');

    // 4) 无匹配：清空搜索
    report.errors.length = 0;
    await goto('#/?q=zzzz_no_match_20261111', 3600);
    const searchEmpty = await state();
    await shot('edge-empty-search');
    const cleared = await clickText('.wall__note-actions button', '清空搜索');
    await sleep(1500);
    report.cases.emptySearch = { clickable: cleared, before: searchEmpty, after: await state(), errors: report.errors.length };

    // 5) 空标签 / 空分级 / 空分组 / 空收藏（二级墙）
    const wallCases = [
      { key: 'emptyTag', hash: '#/t/__no_such_tag__', label: '回标签总览', expect: '#/tags', shot: 'edge-empty-tag' },
      { key: 'emptyRating', hash: '#/r/X', label: '回分级总览', expect: '#/ratings', shot: 'edge-empty-rating' },
      { key: 'emptyGroup', hash: '#/g/__no_such_group__', label: '回分组总览', expect: '#/groups', shot: 'edge-empty-group' },
      { key: 'emptyFav', hash: '#/fav', label: '回首页', expect: '#/', shot: 'edge-empty-fav' },
    ];
    for (const entry of wallCases) {
      report.errors.length = 0;
      await goto(entry.hash, 3000);
      const before = await state();
      await shot(entry.shot);
      const clickable = await clickText('.wall__note-actions button', entry.label);
      await sleep(1600);
      report.cases[entry.key] = {
        clickable,
        expected: entry.expect,
        before,
        after: { hash: await evaluate('location.hash'), tiles: await evaluate(`document.querySelectorAll('.tile').length`) },
        errors: report.errors.length,
      };
    }

    // 6) 收藏总览空态：BucketGallery 的动作
    report.errors.length = 0;
    await goto('#/favs', 2400);
    const favsBefore = await state();
    await shot('edge-empty-favs');
    const favsClickable = await clickText('.tags__empty-actions button', '回首页');
    await sleep(1400);
    report.cases.emptyFavsOverview = {
      clickable: favsClickable,
      before: favsBefore,
      after: { hash: await evaluate('location.hash'), tiles: await evaluate(`document.querySelectorAll('.tile').length`) },
      errors: report.errors.length,
    };

    // 7) 有效路由不受影响
    report.errors.length = 0;
    await goto('#/tags', 2200);
    const tagsRoute = await evaluate(`({ tags: !!document.querySelector('.tags'), missing: !!document.querySelector('.stage__missing') })`);
    await goto('#/t/pink_hair', 2600);
    const tagRoute = await evaluate(`({
      title: document.querySelector('.wallhead__title')?.textContent.trim() ?? null,
      missing: !!document.querySelector('.stage__missing'),
      note: !!document.querySelector('.wall__note'),
    })`);
    report.cases.validRoutes = { tagsRoute, tagRoute, errors: report.errors.length };

    const nf = report.cases.notFound;
    const kb = report.cases.keyboardAndHistory;
    const gs = report.cases.goSearch;
    const se = report.cases.emptySearch;
    const et = report.cases.emptyTag;
    const er = report.cases.emptyRating;
    const eg = report.cases.emptyGroup;
    const ef = report.cases.emptyFav;
    const efo = report.cases.emptyFavsOverview;
    const vr = report.cases.validRoutes;
    report.passed =
      nf.missing &&
      nf.missingTitle?.includes('没有对应的页面') &&
      nf.missingActions.join('|') === '回首页|去搜索' &&
      nf.allButtons &&
      nf.tiles === 0 &&
      nf.errors === 0 &&
      kb.focusedBefore === '回首页' &&
      kb.afterEnter.hash === '#/' &&
      kb.afterEnter.tiles > 0 &&
      kb.afterBack.missing &&
      kb.afterBack.hash === '#/definitely-not-a-route' &&
      kb.afterForward.hash === '#/' &&
      kb.errors === 0 &&
      gs.hash === '#/' &&
      gs.panel &&
      gs.focusTag === 'INPUT' &&
      (gs.focusPlaceholder ?? '').includes('搜索') &&
      gs.errors === 0 &&
      se.before.noteButtons.join('|') === '清空搜索' &&
      se.clickable &&
      !se.after.hash.includes('q=') &&
      se.after.tiles > 0 &&
      se.errors === 0 &&
      et.clickable && et.after.hash === et.expected &&
      er.clickable && er.after.hash === er.expected &&
      eg.clickable && eg.after.hash === eg.expected &&
      ef.clickable && ef.after.hash === ef.expected &&
      et.before.noteButtons.length === 1 && er.before.noteButtons.length === 1 &&
      eg.before.noteButtons.length === 1 && ef.before.noteButtons.length === 1 &&
      et.errors === 0 && er.errors === 0 && eg.errors === 0 && ef.errors === 0 &&
      efo.clickable && efo.before.bucketButtons.join('|') === '回首页' && efo.after.hash === '#/' && efo.errors === 0 &&
      vr.tagsRoute.tags && !vr.tagsRoute.missing && vr.tagRoute.title === 'pink hair' && !vr.tagRoute.missing && vr.errors === 0;
  } finally {
    if (cdp) cdp.close();
    try {
      child.kill();
    } catch {
      /* 已退出 */
    }
    hardDelete(profile);
  }

  writeFileSync(join(outDir, 'edge-report.json'), JSON.stringify(report, null, 2));
  console.log(JSON.stringify(report, null, 2));
  process.exit(report.passed ? 0 : 1);
}

main().catch((error) => {
  console.error(error);
  process.exit(2);
});
