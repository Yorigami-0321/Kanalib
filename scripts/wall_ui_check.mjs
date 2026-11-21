#!/usr/bin/env node
/**
 * UI 细节审查的一次性验收探针（2026-11-11）。
 * 产出 1440×900 / 390×844 的首页、标签、详情截图，并断言：
 *   - 卡面标题不含图片扩展名、不含完整 32 位哈希；
 *   - 标签名不含下划线（displayTag 已转空格）；
 *   - 检查器操作区（.inspector__actions）在首屏内可点：bottom <= innerHeight；
 *   - 390 下 documentElement.scrollWidth === innerWidth；
 *   - 390 下卡面标题封顶 1 行。
 * 用法：node scripts/wall_ui_check.mjs http://127.0.0.1:4318/ [outDir]
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

class Cdp {
  constructor(url) {
    this.url = url;
    this.nextId = 1;
    this.pending = new Map();
    this.listeners = [];
  }

  async connect() {
    this.ws = new WebSocket(this.url);
    await new Promise((ok, no) => {
      this.ws.addEventListener('open', ok, { once: true });
      this.ws.addEventListener('error', () => no(new Error('CDP 连接失败')), { once: true });
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
    return new Promise((done, no) => {
      this.pending.set(id, { resolve: done, reject: no });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  on(listener) {
    this.listeners.push(listener);
  }

  close() {
    try {
      this.ws.close();
    } catch {
      /* closed */
    }
  }
}

const base = (process.argv[2] ?? 'http://127.0.0.1:4318/').replace(/\/?$/, '/');
const outDir = resolve(process.argv[3] ?? 'web/screenshots/ui-review/ui');
mkdirSync(outDir, { recursive: true });
const chrome = CANDIDATES.find((path) => existsSync(path));
if (!chrome) {
  console.error('找不到 Chrome/Edge');
  process.exit(1);
}
const profile = mkdtempSync(join(tmpdir(), 'kanalib-ui-check-'));
const port = 9900 + Math.floor(Math.random() * 90);

const child = spawn(
  chrome,
  [
    '--headless=new',
    `--remote-debugging-port=${port}`,
    `--user-data-dir=${profile}`,
    '--no-first-run',
    '--no-default-browser-check',
    '--hide-scrollbars',
    '--window-size=1440,900',
    'about:blank',
  ],
  { stdio: 'ignore' },
);

const report = { shots: {}, consoleErrors: [], checks: [], failures: [] };
let cdp;

const check = (name, ok, detail) => {
  report.checks.push({ name, ok, detail });
  if (!ok) report.failures.push(name + (detail ? '：' + detail : ''));
};

try {
  for (let i = 0; i < 80; i += 1) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (res.ok) break;
    } catch {
      /* wait */
    }
    await sleep(120);
  }
  const target = await (await fetch(`http://127.0.0.1:${port}/json/new?about:blank`, { method: 'PUT' })).json();
  cdp = new Cdp(target.webSocketDebuggerUrl);
  await cdp.connect();
  cdp.on((message) => {
    if (message.method === 'Runtime.exceptionThrown') {
      report.consoleErrors.push('exception: ' + (message.params.exceptionDetails?.text ?? 'unknown'));
    }
    if (message.method === 'Runtime.consoleAPICalled' && message.params.type === 'error') {
      report.consoleErrors.push(message.params.args.map((arg) => arg.value ?? arg.description ?? '').join(' '));
    }
  });
  await cdp.send('Page.enable');
  await cdp.send('Runtime.enable');

  const evaluate = async (expression) => {
    const result = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
    if (result.exceptionDetails) {
      throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    }
    return result.result.value;
  };
  const shot = async (name) => {
    const { data } = await cdp.send('Page.captureScreenshot', { format: 'png', fromSurface: true });
    const path = join(outDir, name + '.png');
    writeFileSync(path, Buffer.from(data, 'base64'));
    report.shots[name] = path;
  };
  const settle = async (limit = 12000) => {
    const started = Date.now();
    while (Date.now() - started < limit) {
      const state = await evaluate(
        `(() => ({ tiles: document.querySelectorAll('.tile').length, pending: [...document.querySelectorAll('.tile img')].filter((img) => !img.complete || !img.classList.contains('is-loaded')).length }))()`,
      ).catch(() => null);
      if (state && state.tiles > 0 && state.pending === 0) return state;
      await sleep(200);
    }
    return { timeout: true };
  };
  const go = async (hash, wait = 1800) => {
    await cdp.send('Page.navigate', { url: base + hash });
    await sleep(wait);
  };

  const TITLE_AUDIT = `(() => {
    const titles = [...document.querySelectorAll('.tile__copy strong')].map((node) => node.textContent.trim()).filter(Boolean);
    const ext = /\\.(?:avif|gif|heic|heif|jpe?g|png|tiff?|webp|bmp)$/i;
    const fullHash = /\\b[0-9a-f]{32}\\b/i;
    const samples = [...new Set(titles)].slice(0, 12);
    const nodes = [...document.querySelectorAll('.tile__copy strong')].slice(0, 24);
    return {
      count: titles.length,
      withExtension: titles.filter((text) => ext.test(text)).slice(0, 5),
      withFullHash: titles.filter((text) => fullHash.test(text)).slice(0, 5),
      samples,
      lineClamp: titles.length ? getComputedStyle(document.querySelector('.tile__copy strong')).webkitLineClamp : null,
      titleRects: nodes.map((node) => {
        const box = node.getBoundingClientRect();
        return { text: node.textContent.trim(), rect: [box.left, box.top, box.right, box.bottom].map(Math.round) };
      }),
    };
  })()`;

  const TAG_AUDIT = `(() => {
    const names = [...document.querySelectorAll('.tagcard__name')].map((node) => node.textContent.trim()).filter(Boolean);
    return {
      count: names.length,
      withUnderscore: names.filter((text) => text.includes('_')).slice(0, 5),
      samples: [...new Set(names)].slice(0, 12),
    };
  })()`;

  const INSPECTOR_AUDIT = `(() => {
    const inspector = document.querySelector('.inspector');
    const actions = document.querySelector('.inspector__actions');
    const scroll = document.querySelector('.inspector__scroll');
    if (!inspector || !actions) return { ok: false };
    const box = actions.getBoundingClientRect();
    return {
      ok: true,
      actions: [Math.round(box.left), Math.round(box.top), Math.round(box.right), Math.round(box.bottom)],
      bottomInViewport: Math.round(box.bottom) <= innerHeight,
      width: Math.round(box.width),
      scroll: scroll ? { flex: getComputedStyle(scroll).flex, overflowY: getComputedStyle(scroll).overflowY } : null,
      primaryVisible: !![...actions.querySelectorAll('.inspector__primary')].find((node) => {
        const b = node.getBoundingClientRect();
        return b.bottom <= innerHeight && b.width > 0;
      }),
      links: [...actions.querySelectorAll('.inspector__link')].map((node) => {
        const b = node.getBoundingClientRect();
        return { text: node.textContent.trim(), inViewport: b.bottom <= innerHeight && b.top >= 0 };
      }),
      scrollW: document.documentElement.scrollWidth,
      innerW: innerWidth,
    };
  })()`;

  /* ===== 1440×900 ===== */
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
  await go('#/', 600);
  const homeSettle = await settle();
  await sleep(350);
  await shot('home-1440');
  const desktopTitles = await evaluate(TITLE_AUDIT);
  check('1440 卡面标题无扩展名', desktopTitles.withExtension.length === 0, JSON.stringify(desktopTitles.withExtension));
  check('1440 卡面标题无完整 32 位哈希', desktopTitles.withFullHash.length === 0, JSON.stringify(desktopTitles.withFullHash));
  report.desktopTitles = desktopTitles;
  report.homeSettle = homeSettle;

  const firstId = await evaluate(`document.querySelector('.tile')?.dataset.id ?? null`);
  await go('#/d/' + firstId, 2200);
  await sleep(400);
  await shot('detail-1440');
  const desktopInspector = await evaluate(INSPECTOR_AUDIT);
  check('1440 检查器操作区在首屏内', desktopInspector.ok && desktopInspector.bottomInViewport, JSON.stringify(desktopInspector.actions));
  check('1440 检查器返回按钮可见', desktopInspector.ok && desktopInspector.primaryVisible);
  check('1440 检查器复制/全屏链接可见', desktopInspector.ok && desktopInspector.links.filter((link) => link.inViewport).length >= 2, JSON.stringify(desktopInspector.links));
  report.desktopInspector = desktopInspector;

  await go('#/tags', 3000);
  await sleep(300);
  await shot('tags-1440');
  const desktopTags = await evaluate(TAG_AUDIT);
  check('1440 标签名不含下划线', desktopTags.withUnderscore.length === 0, JSON.stringify(desktopTags.withUnderscore));
  report.desktopTags = desktopTags;

  /* ===== 390×844 ===== */
  await cdp.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 2, mobile: true });
  await go('#/', 900);
  await settle();
  await sleep(400);
  await shot('home-390');
  const mobileHome = await evaluate(`(() => {
    const titles = [...document.querySelectorAll('.tile__copy strong')];
    return {
      scrollW: document.documentElement.scrollWidth,
      innerW: innerWidth,
      lineClamps: [...new Set(titles.map((node) => getComputedStyle(node).webkitLineClamp))],
      fontSizes: [...new Set(titles.map((node) => getComputedStyle(node).fontSize))],
      overflowing: titles.filter((node) => node.scrollWidth > node.clientWidth + 1).slice(0, 5).map((node) => node.textContent.trim()),
      titleRects: titles.slice(0, 24).map((node) => {
        const box = node.getBoundingClientRect();
        return { text: node.textContent.trim(), rect: [box.left, box.top, box.right, box.bottom].map(Math.round) };
      }),
    };
  })()`);
  check('390 无横向溢出', mobileHome.scrollW === mobileHome.innerW, `${mobileHome.scrollW}/${mobileHome.innerW}`);
  check('390 卡面标题封顶 1 行', mobileHome.lineClamps.every((value) => value === '1'), JSON.stringify(mobileHome.lineClamps));
  check('390 卡面标题不越出卡片', mobileHome.overflowing.length === 0, JSON.stringify(mobileHome.overflowing));
  report.mobileHome = mobileHome;

  await go('#/d/' + firstId, 2200);
  await sleep(500);
  await shot('detail-390');
  const mobileInspector = await evaluate(INSPECTOR_AUDIT);
  check('390 检查器操作区在首屏内', mobileInspector.ok && mobileInspector.bottomInViewport, JSON.stringify(mobileInspector.actions));
  check('390 检查器返回按钮可见', mobileInspector.ok && mobileInspector.primaryVisible);
  check('390 检查器滚动区可滚动', mobileInspector.ok && mobileInspector.scroll?.overflowY === 'auto', JSON.stringify(mobileInspector.scroll));
  check('390 详情无横向溢出', mobileInspector.scrollW === mobileInspector.innerW, `${mobileInspector.scrollW}/${mobileInspector.innerW}`);
  report.mobileInspector = mobileInspector;

  await go('#/tags', 3000);
  await sleep(300);
  await shot('tags-390');
  const mobileTags = await evaluate(TAG_AUDIT);
  const mobileTagsOverflow = await evaluate(
    `(() => ({ scrollW: document.documentElement.scrollWidth, innerW: innerWidth }))()`,
  );
  check('390 标签名不含下划线', mobileTags.withUnderscore.length === 0, JSON.stringify(mobileTags.withUnderscore));
  check('390 标签页无横向溢出', mobileTagsOverflow.scrollW === mobileTagsOverflow.innerW, `${mobileTagsOverflow.scrollW}/${mobileTagsOverflow.innerW}`);
  report.mobileTags = { ...mobileTags, ...mobileTagsOverflow };

  check('console 无报错', report.consoleErrors.length === 0, report.consoleErrors.join(' | '));

  writeFileSync(join(outDir, 'css-check.json'), JSON.stringify(report, null, 2) + '\n', 'utf8');
  console.log(JSON.stringify({ failures: report.failures, checks: report.checks.map((item) => (item.ok ? 'PASS ' : 'FAIL ') + item.name) }, null, 2));
  if (report.failures.length > 0) process.exitCode = 1;
} finally {
  cdp?.close();
  try {
    child.kill();
  } catch {
    /* ignore */
  }
  if (process.platform === 'win32') spawnSync('cmd', ['/c', 'rd', '/s', '/q', profile], { stdio: 'ignore' });
  else rmSync(profile, { recursive: true, force: true });
}
