#!/usr/bin/env node
/**
 * 详情预加载窗口探针（±2/±3 占位 + 居中升档 + 弱网跳过）。
 *
 * 用法（先在 web/ 里起服务）：
 *   node scripts/wall_probe_detail_preload.mjs --base http://127.0.0.1:4273/ --out web/screenshots/round-a
 *
 * 断言（快网）：
 *  1) 打开详情（列表中部）后，正中前后各 3 张内的 t512 都已请求/已在缓存：
 *     ±1 由邻屏挂载，±2/±3 由预取补；预取请求的 id 不越出 ±3，也不含重复下载。
 *  2) 未居中的图不拉锐利档：t1024/t2048 的 id 只会是"停留过的正中那张"。
 *  3) 快速连按 → 5 次：每次换图都拉目标的 t1024；终点（sharp 2048）升到 t2048，
 *     锐利层 is-loaded 与 URL 一致；所有重复请求都是缓存命中，没有二次网络下载。
 * 断言（弱网，saveData / 3g）：
 *  4) 预取整段跳过（t512 只有三块邻屏），t2048 不升级；首屏字节与改动前一致。
 * 断言（A1 标题遮挡）：
 *  5) `#/t/solo` 静止时标题可读（截图），标题区域点击落到卡片上（不拦指针），
 *     点最顶卡片进入聚焦后 360° 补图仍成立。
 *
 * 退出码：0 通过；1 断言失败；2 环境问题。
 */
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { extname, join, resolve } from 'node:path';
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
    return new Promise((resolvePromise, reject) => {
      this.pending.set(id, { resolve: resolvePromise, reject });
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

/**
 * 数据侧只读：按 id 查标题与锐利档，用来判断"该不该有 t1024/t2048"；
 * 同时算出 `#/t/solo?sort=name` 的完整顺序（墙只渲染视口附近的卡，DOM 里凑不齐 ±3 窗口）。
 */
function loadMeta(tag = 'solo') {
  const dataRoot = arg('data', 'D:/Code/Kanalib/web/public/data');
  const meta = new Map();
  const all = [];
  const tagged = [];
  try {
    const index = JSON.parse(readFileSync(join(dataRoot, 'index.json'), 'utf8'));
    for (const shard of index.shards) {
      const payload = JSON.parse(readFileSync(join(dataRoot, shard.path), 'utf8'));
      for (const item of payload.items) {
        meta.set(item.id, { name: item.n, sharp: item.sharp ?? -1 });
        all.push(item);
        if (Array.isArray(item.tags) && item.tags.some((entry) => entry[0] === tag)) tagged.push(item);
      }
    }
  } catch {
    /* 读不到就只做结构断言 */
  }
  // 与 App.sortItems 的名序一致。
  const byName = (left, right) => left.n.localeCompare(right.n, 'zh-Hans-CN');
  tagged.sort(byName);
  all.sort(byName);
  // soloOrder 是墙上的 data-index 顺序；detailOrder 是详情序列——
  // 进入 #/d/ 后路由不再带标签，App 的详情列表会退回整库（再按当前排序），
  // 预取窗口必须按后者核对。
  return { meta, soloOrder: tagged.map((item) => item.id), detailOrder: all.map((item) => item.id) };
}

/**
 * 可选：直接对 dist 起一个带生产口径缓存的静态服务。
 * vite preview 对所有资源回 `Cache-Control: no-cache`，同一张图二次挂载会重新回源，
 * 没法区分"预取浪费"和"预览服务器的缓存头"。生产静态站给图片带 max-age，
 * 这里用同样的口径测"重复请求是否真的重复下载"。
 */
function startStaticServer(root) {
  const mime = {
    '.html': 'text/html; charset=utf-8',
    '.js': 'text/javascript; charset=utf-8',
    '.css': 'text/css; charset=utf-8',
    '.json': 'application/json; charset=utf-8',
    '.jpg': 'image/jpeg',
    '.jpeg': 'image/jpeg',
    '.png': 'image/png',
    '.webp': 'image/webp',
    '.gif': 'image/gif',
    '.svg': 'image/svg+xml',
  };
  const server = createServer(async (request, response) => {
    try {
      const pathname = decodeURIComponent(new URL(request.url, 'http://127.0.0.1').pathname);
      let relative = pathname === '/' ? '/index.html' : pathname;
      if (relative.endsWith('/')) relative += 'index.html';
      const file = resolve(root, '.' + relative);
      if (!file.startsWith(resolve(root))) {
        response.statusCode = 403;
        response.end('forbidden');
        return;
      }
      const data = await readFile(file);
      response.setHeader('Content-Type', mime[extname(file).toLowerCase()] ?? 'application/octet-stream');
      response.setHeader(
        'Cache-Control',
        relative.startsWith('/data/') ? 'public, max-age=3600, immutable' : 'no-cache',
      );
      response.end(data);
    } catch {
      response.statusCode = 404;
      response.end('not found');
    }
  });
  return new Promise((resolvePromise, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      resolvePromise({ server, port: server.address().port });
    });
  });
}

const TIER_RE = /\/data\/(t256|t512|t1024|t2048|o)\/([^/?#]+)/;

function parseAsset(url) {
  try {
    const match = TIER_RE.exec(new URL(url).pathname);
    if (!match) return null;
    return { tier: match[1], id: decodeURIComponent(match[2]).replace(/\.[a-z0-9]+$/i, '') };
  } catch {
    return null;
  }
}

async function main() {
  const serveDist = arg('serve-dist', '');
  let staticServer = null;
  let base;
  if (serveDist) {
    const started = await startStaticServer(resolve(serveDist));
    staticServer = started.server;
    base = `http://127.0.0.1:${started.port}/`;
  } else {
    const raw = arg('base', 'http://127.0.0.1:4273/');
    base = raw.endsWith('/') ? raw : raw + '/';
  }
  const outDir = resolve(arg('out', 'web/screenshots/round-a'));
  const chrome = CANDIDATES.find((path) => existsSync(path));
  if (!chrome) {
    console.error('找不到 Chrome/Edge');
    process.exit(2);
  }
  mkdirSync(outDir, { recursive: true });
  const { meta, soloOrder, detailOrder } = loadMeta();

  const profile = mkdtempSync(join(tmpdir(), 'kanalib-preload-'));
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

  const report = { base, outDir, checks: {}, consoleErrors: [], network: {}, screenshots: {}, passed: false };
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
    await cdp.send('Network.enable');
    // 无头 Chrome 的 navigator.connection.effectiveType 默认报 3g，会让 slowNetwork()
    // 把每一轮都当弱网（预取/2048 全跳过）。快网轮显式覆盖成 4g，弱网轮再覆盖回去。
    const fastConnectionScript = await cdp.send('Page.addScriptToEvaluateOnNewDocument', {
      source: `(() => {
        const proto = Object.getPrototypeOf(navigator.connection || {});
        if (!proto) return;
        Object.defineProperty(proto, 'effectiveType', { configurable: true, get: () => '4g' });
        Object.defineProperty(proto, 'saveData', { configurable: true, get: () => false });
      })();`,
    });

    /** 网络流水：request 按 requestId 记录 URL / initiator；response 记缓存来源。 */
    let net = { requests: new Map(), responses: new Map(), servedFromCache: new Set(), finished: new Map() };
    let netMark = 0;
    cdp.on((message) => {
      if (message.method === 'Network.requestWillBeSent') {
        net.requests.set(message.params.requestId, {
          url: message.params.request.url,
          initiator: message.params.initiator?.type ?? 'unknown',
          wallTime: message.params.wallTime ?? 0,
        });
      } else if (message.method === 'Network.responseReceived') {
        const response = message.params.response;
        net.responses.set(message.params.requestId, {
          status: response.status,
          fromDiskCache: response.fromDiskCache === true,
          fromMemoryCache: response.fromMemoryCache === true,
          fromPrefetchCache: response.fromPrefetchCache === true,
        });
      } else if (message.method === 'Network.requestServedFromCache') {
        net.servedFromCache.add(message.params.requestId);
      } else if (message.method === 'Network.loadingFinished') {
        net.finished.set(message.params.requestId, message.params.encodedDataLength ?? 0);
      }
    });
    const resetNet = () => {
      netMark = Date.now() / 1000;
      net = { requests: new Map(), responses: new Map(), servedFromCache: new Set(), finished: new Map() };
    };
    const summary = () => {
      const tiers = {};
      for (const [requestId, request] of net.requests) {
        const asset = parseAsset(request.url);
        if (!asset) continue;
        const bucket = (tiers[asset.tier] ||= { events: 0, bytes: 0, initiators: {}, ids: new Map() });
        bucket.events += 1;
        bucket.initiators[request.initiator] = (bucket.initiators[request.initiator] ?? 0) + 1;
        const perId = bucket.ids.get(asset.id) || { events: 0, cacheHits: 0, bytes: 0, initiators: [], log: [] };
        perId.events += 1;
        perId.initiators.push(request.initiator);
        const response = net.responses.get(requestId);
        const cached = Boolean(
          net.servedFromCache.has(requestId) ||
            response?.fromDiskCache ||
            response?.fromMemoryCache ||
            response?.fromPrefetchCache,
        );
        if (cached) perId.cacheHits += 1;
        const bytes = net.finished.get(requestId) ?? 0;
        perId.bytes += bytes;
        perId.log.push({
          initiator: request.initiator,
          cached,
          status: response?.status ?? null,
          bytes,
          atMs: Math.round(((request.wallTime ?? 0) - netMark) * 1000),
        });
        bucket.ids.set(asset.id, perId);
        bucket.bytes += bytes;
      }
      const compact = {};
      for (const [tier, bucket] of Object.entries(tiers)) {
        compact[tier] = {
          events: bucket.events,
          unique: bucket.ids.size,
          bytes: bucket.bytes,
          initiators: bucket.initiators,
          ids: Object.fromEntries(
            [...bucket.ids].map(([id, value]) => [
              id,
              {
                events: value.events,
                cacheHits: value.cacheHits,
                networkFetch: value.events - value.cacheHits,
                bytes: value.bytes,
                initiators: value.initiators,
                log: value.log,
              },
            ]),
          ),
        };
      }
      return compact;
    };
    const idsOf = (tiers, tier) => Object.keys(tiers[tier]?.ids ?? {});
    const scriptPrefetched = (tiers) =>
      Object.entries(tiers.t512?.ids ?? {})
        .filter(([, value]) => value.initiators.includes('script'))
        .map(([id]) => id);

    const evaluate = async (expression) => {
      const result = await cdp.send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true });
      if (result.exceptionDetails) {
        throw new Error('页面脚本异常：' + (result.exceptionDetails.exception?.description ?? result.exceptionDetails.text));
      }
      return result.result?.value;
    };
    const shot = async (name, clip) => {
      const png = await cdp.send('Page.captureScreenshot', clip ? { format: 'png', clip } : { format: 'png' });
      const path = join(outDir, name);
      writeFileSync(path, Buffer.from(png.data, 'base64'));
      return path;
    };

    const waitWallReady = async (label, timeoutMs = 45000, minTiles = 12) => {
      const deadline = Date.now() + timeoutMs;
      let prevCount = -1;
      let stable = 0;
      while (Date.now() < deadline) {
        const state = await evaluate(`(() => {
          const countText = document.querySelector('.seam__foot .seam__count')?.textContent.trim() ?? '';
          const nums = (countText.match(/[\\d,]+/g) || []).map((s) => Number(s.replace(/,/g, '')));
          return {
            tiles: document.querySelectorAll('.tile').length,
            loaded: nums.length === 2 && nums[0] === nums[1] && nums[0] > 0,
          };
        })()`);
        if (state.loaded && state.tiles >= minTiles) {
          if (state.tiles === prevCount) stable += 1;
          else stable = 0;
          prevCount = state.tiles;
          if (stable >= 2) {
            await sleep(900);
            return;
          }
        } else {
          stable = 0;
          prevCount = -1;
        }
        await sleep(300);
      }
      throw new Error(`${label}: 墙没就绪`);
    };

    const wallOrder = () =>
      evaluate(`(() => {
        const list = [...document.querySelectorAll('.tile')]
          .filter((node) => node.dataset.index !== undefined && node.dataset.id)
          .map((node) => ({ index: Number(node.dataset.index), id: node.dataset.id }));
        list.sort((a, b) => a.index - b.index);
        return list;
      })()`);

    const openDetailAt = async (index) => {
      const focused = await evaluate(`(() => {
        const tile = document.querySelector('.tile[data-index="${index}"]');
        if (!tile) return false;
        tile.focus();
        return true;
      })()`);
      if (!focused) throw new Error(`没有 data-index=${index} 的卡片`);
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
      for (let attempt = 0; attempt < 100; attempt += 1) {
        const open = await evaluate(
          `Boolean(document.querySelector('.view.view--detail')) && /^#\\/d\\//.test(location.hash)`,
        );
        if (open) return;
        await sleep(60);
      }
      throw new Error('详情没有打开');
    };

    const viewState = () =>
      evaluate(`(() => {
        const panes = [...document.querySelectorAll('.view__pane')];
        const base = panes.map((pane) => pane.querySelector('img.view__img:not(.view__img--sharp)')?.getAttribute('src') ?? null);
        const sharp = [...document.querySelectorAll('.view__pane img.view__img--sharp.is-loaded')]
          .map((img) => img.getAttribute('src'));
        const track = document.querySelector('.view__track');
        return {
          detail: Boolean(document.querySelector('.view.view--detail')),
          hashId: /^#\\/d\\/([^?]+)/.exec(location.hash)?.[1] ?? null,
          bases: base,
          sharp,
          trackClass: track ? track.className : '',
          loadedBases: panes.map((pane) => Boolean(pane.querySelector('img.view__img.is-loaded'))),
        };
      })()`);

    const idOfSrc = (src) => (src ? parseAsset(src)?.id ?? null : null);

    const waitSlideDone = async (timeout = 2200) => {
      const deadline = Date.now() + timeout;
      while (Date.now() < deadline) {
        const state = await viewState();
        if (!/view__track--(next|prev)/.test(state.trackClass)) return state;
        await sleep(25);
      }
      return await viewState();
    };

    const pressRight = async () => {
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'ArrowRight', code: 'ArrowRight', windowsVirtualKeyCode: 39 });
      await cdp.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'ArrowRight', code: 'ArrowRight', windowsVirtualKeyCode: 39 });
    };

    // ---------- A1：标题衬底与顶卡聚焦（截图证据） ----------
    await cdp.send('Emulation.setDeviceMetricsOverride', { width: 1440, height: 900, deviceScaleFactor: 1, mobile: false });
    await cdp.send('Page.navigate', { url: base + '#/t/solo' });
    await waitWallReady('solo', 45000, 12);
    const titleState = await evaluate(`(() => {
      const head = document.querySelector('.wallhead');
      const title = document.querySelector('.wallhead__title');
      const rect = head?.getBoundingClientRect();
      const titleRect = title?.getBoundingClientRect();
      const probe = titleRect ? document.elementFromPoint(titleRect.left + 12, titleRect.top + titleRect.height / 2) : null;
      // 标题衬底必须压在至少一张卡上，且那张卡还能被点到：找与标题矩形相交面积最大的卡。
      const tiles = [...document.querySelectorAll('.tile')].map((node) => ({ node, rect: node.getBoundingClientRect() }));
      let tile = null;
      let bestArea = 0;
      if (rect) {
        for (const entry of tiles) {
          const w = Math.min(entry.rect.right, rect.right) - Math.max(entry.rect.left, rect.left);
          const h = Math.min(entry.rect.bottom, rect.bottom) - Math.max(entry.rect.top, rect.top);
          if (w > 12 && h > 12 && w * h > bestArea) {
            bestArea = w * h;
            tile = entry;
          }
        }
      }
      let overlapPoint = null;
      if (rect && tile) {
        const left = Math.max(tile.rect.left, rect.left);
        const right = Math.min(tile.rect.right, rect.right);
        const top = Math.max(tile.rect.top, rect.top);
        const bottom = Math.min(tile.rect.bottom, rect.bottom);
        overlapPoint = { x: (left + right) / 2, y: (top + bottom) / 2 };
      }
      const hit = overlapPoint ? document.elementFromPoint(overlapPoint.x, overlapPoint.y) : null;
      return {
        title: title?.textContent?.trim() ?? null,
        headRect: rect ? { x: rect.left, y: rect.top, w: rect.width, h: rect.height } : null,
        titleVisible: Boolean(titleRect && titleRect.width > 0 && titleRect.height > 0),
        probeTag: probe ? (probe.closest('.tile') ? 'tile' : probe.className || probe.tagName) : null,
        overlapArea: Math.round(bestArea),
        overlapPoint,
        overlapHitTag: hit ? (hit.closest('.tile') ? 'tile' : hit.className || hit.tagName) : null,
        topTileId: tile?.node.dataset.id ?? null,
      };
    })()`);
    const staticShot = await shot('tag-solo-static.png');
    const titleShot = await shot('tag-solo-title-clip.png', {
      x: 100,
      y: 0,
      width: 620,
      height: 130,
      scale: 2,
    });

    // 点最顶那张卡（若衬底覆盖到它，点击必须仍落在卡上），检查聚焦后 360° 补图。
    if (titleState.overlapPoint) {
      await cdp.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: titleState.overlapPoint.x, y: titleState.overlapPoint.y, button: 'left', buttons: 1, clickCount: 1 });
      await sleep(40);
      await cdp.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: titleState.overlapPoint.x, y: titleState.overlapPoint.y, button: 'left', buttons: 0, clickCount: 1 });
      await sleep(800);
    }
    const focusState = await evaluate(`(() => {
      const canvas = document.querySelector('.canvas');
      const cr = canvas?.getBoundingClientRect();
      const tiles = [...document.querySelectorAll('.tile')].map((node) => node.getBoundingClientRect());
      return {
        spotlight: canvas?.classList.contains('is-spotlight') ?? false,
        aboveTop: cr ? tiles.filter((rect) => rect.top < cr.top).length : 0,
        tiles: tiles.length,
      };
    })()`);
    const focusShot = await shot('tag-solo-top-card.png');
    report.checks.a1 = {
      ...titleState,
      staticShot,
      titleShot,
      focusShot,
      focus: focusState,
      pass:
        titleState.title === 'solo' &&
        titleState.titleVisible &&
        titleState.probeTag === 'tile' &&
        (titleState.overlapPoint ? titleState.overlapHitTag === 'tile' : true) &&
        focusState.spotlight === true &&
        focusState.aboveTop > 0,
    };
    report.screenshots = { staticShot, titleShot, focusShot };

    // ---------- 快网：±2/±3 预取 + 连按 5 次 ----------
    await cdp.send('Page.navigate', { url: base + '#/t/solo?sort=name' });
    await waitWallReady('sorted-solo', 60000, 12);
    const fastConnection = await evaluate(`(() => {
      const connection = navigator.connection;
      return { saveData: connection?.saveData ?? null, effectiveType: connection?.effectiveType ?? null };
    })()`);
    const wallDebug = await evaluate(`(() => ({
      hash: location.hash,
      title: document.querySelector('.wallhead__title')?.textContent?.trim() ?? null,
      count: document.querySelector('.wallhead__count')?.textContent?.trim() ?? null,
      tileCount: document.querySelectorAll('.tile').length,
      hasStray: Boolean(document.querySelector('.tile[data-id="fbd1cbaca30af0b865bd4275965a74d9005fc941"]')),
      foot: document.querySelector('.seam__foot .seam__count')?.textContent?.trim() ?? null,
    }))()`);
    const domOrder = await wallOrder();
    // 墙上 data-index = 标签筛后名序；详情序列 = 整库名序（见 loadMeta 注释）。
    const order = soloOrder.map((id, index) => ({ index, id }));
    const domMismatch = domOrder.filter((entry) => soloOrder[entry.index] !== entry.id).slice(0, 5);
    const byIndex = new Map(order.map((entry) => [entry.index, entry.id]));
    const detailPosition = new Map(detailOrder.map((id, index) => [id, index]));
    const detailIdAt = (position) => detailOrder[position] ?? null;
    const maxIndex = order.length > 0 ? order[order.length - 1].index : -1;
    let startIndex = -1;
    for (let candidate = 10; candidate + 8 <= maxIndex; candidate += 1) {
      const centerId = byIndex.get(candidate);
      const position = detailPosition.get(centerId);
      if (position === undefined || position < 3 || position + 8 > detailOrder.length) continue;
      const centerSharp = meta.get(centerId)?.sharp ?? -1;
      const finalSharp = meta.get(detailOrder[position + 5])?.sharp ?? -1;
      if (centerSharp >= 1024 && finalSharp >= 2048) {
        startIndex = candidate;
        break;
      }
    }
    if (startIndex < 0) {
      // 兜底：顺序里没有同时满足"正中可升档 + 终点 2048"的位置时，退到 20。
      startIndex = Math.min(20, Math.max(4, maxIndex - 7));
    }
    const centerId = byIndex.get(startIndex) ?? null;
    const centerPosition = detailPosition.get(centerId) ?? -1;
    const idAt = (offset) => detailIdAt(centerPosition + offset);
    const expectedWindow = [-3, -2, -1, 0, 1, 2, 3].map((offset) => idAt(offset)).filter(Boolean);
    const expectedPrefetch = [-3, -2, 2, 3].map((offset) => idAt(offset)).filter(Boolean);

    const prior = summary();
    const wallKnownT512 = new Set(idsOf(prior, 't512'));
    resetNet();
    await openDetailAt(startIndex);
    await sleep(2600);
    const openState = await viewState();
    const openNet = summary();
    const paneIds = [idAt(-1), idAt(0), idAt(1)].filter(Boolean);
    const openRequested = new Set(idsOf(openNet, 't512'));
    const openScriptRequests = scriptPrefetched(openNet);
    const openPrefetched = [...openRequested].filter((id) => !paneIds.includes(id));
    const openWindowCovered = expectedWindow.every(
      (id) => openRequested.has(id) || wallKnownT512.has(id),
    );
    const openExtra = [...openRequested].filter(
      (id) => !expectedWindow.includes(id) && !wallKnownT512.has(id),
    );
    const openPrefetchCovered = expectedPrefetch.every(
      (id) => openPrefetched.includes(id) || wallKnownT512.has(id),
    );
    const openPrefetchOutside = openPrefetched.filter((id) => !expectedWindow.includes(id));
    const openSharpOther = [...idsOf(openNet, 't1024'), ...idsOf(openNet, 't2048')].filter((id) => id !== centerId);
    report.checks.open = {
      startIndex,
      centerId,
      expectedWindow,
      expectedPrefetch,
      panes: paneIds.map((id) => idOfSrc(openState.bases.find((src) => idOfSrc(src) === id) ?? null)),
      paneSrcs: openState.bases,
      openRequested: [...openRequested],
      openPrefetched,
      openScriptRequests,
      openWindowCovered,
      openPrefetchCovered,
      openPrefetchOutside,
      openExtra,
      sharpIds: { t1024: idsOf(openNet, 't1024'), t2048: idsOf(openNet, 't2048') },
      openSharpOther,
      fastConnection,
      totalItems: order.length,
      domMismatch,
      wallDebug,
      t512: openNet.t512
        ? { events: openNet.t512.events, unique: openNet.t512.unique, bytes: openNet.t512.bytes }
        : null,
      t1024: openNet.t1024 ? { unique: openNet.t1024.unique, bytes: openNet.t1024.bytes } : null,
      t2048: openNet.t2048 ? { unique: openNet.t2048.unique, bytes: openNet.t2048.bytes } : null,
      pass:
        fastConnection.saveData === false &&
        fastConnection.effectiveType === '4g' &&
        domMismatch.length === 0 &&
        order.length > startIndex + 10 &&
        openWindowCovered &&
        openPrefetchCovered &&
        openPrefetchOutside.length === 0 &&
        openExtra.length === 0 &&
        openSharpOther.length === 0 &&
        paneIds.length === openState.bases.filter(Boolean).length,
    };

    // 连按 → 5 次：按一次 → 等 hash 真的换图 → 再等滑动轨道类名回落。
    // 只听轨道类名容易被"类名还没挂上就被当成动画结束"骗到，导致下一次按键被丢掉。
    const visited = [centerId];
    for (let step = 0; step < 5; step += 1) {
      const before = await viewState();
      await pressRight();
      let changed = null;
      const changeDeadline = Date.now() + 1600;
      while (Date.now() < changeDeadline) {
        const state = await viewState();
        if (state.hashId && state.hashId !== before.hashId) {
          changed = state;
          break;
        }
        await sleep(30);
      }
      if (!changed) throw new Error(`第 ${step + 1} 次 → 没有换图（hash 停在 ${before.hashId}）`);
      await sleep(80);
      await waitSlideDone();
      await sleep(150);
      visited.push(changed.hashId);
    }
    // 终点停留：700ms 升档 + 预取 800ms + 加载余量。
    await sleep(3200);
    const finalState = await viewState();
    const afterNet = summary();
    const sharp1024 = idsOf(afterNet, 't1024');
    const sharp2048 = idsOf(afterNet, 't2048');
    const visitedSharp = visited.filter((id) => (meta.get(id)?.sharp ?? 0) >= 1024);
    const sharpOnlyVisited = [...sharp1024, ...sharp2048].filter((id) => !visited.includes(id));
    const finalId = finalState.hashId;
    const finalMeta = finalId ? meta.get(finalId) : null;
    const finalSharpSrc = finalState.sharp.find((src) => idOfSrc(src) === finalId) ?? null;
    const finalSharpTier = finalSharpSrc ? parseAsset(finalSharpSrc)?.tier ?? null : null;
    const duplicateNetwork = [];
    for (const tier of ['t512', 't1024', 't2048']) {
      for (const [id, value] of Object.entries(afterNet[tier]?.ids ?? {})) {
        if (value.networkFetch > 1) duplicateNetwork.push({ tier, id, networkFetch: value.networkFetch, events: value.events });
      }
    }
    const prefetchIds = scriptPrefetched(afterNet);
    const visitedWindow = new Set(
      visited.flatMap((id) => {
        const position = detailPosition.get(id) ?? -1;
        return [-3, -2, -1, 0, 1, 2, 3].map((offset) => detailIdAt(position + offset)).filter(Boolean);
      }),
    );
    // 再往前按 3 步，拿到"终点之后会看到的三张"。预取 +=2/+3 应当正好命中后两张；
    // 比按数据推测顺序更可靠（详情序列由 App 的筛后列表决定，数据侧排序只是近似）。
    const ahead = [];
    for (let step = 0; step < 3; step += 1) {
      const before = await viewState();
      await pressRight();
      const deadline = Date.now() + 1600;
      let changed = null;
      while (Date.now() < deadline) {
        const state = await viewState();
        if (state.hashId && state.hashId !== before.hashId) {
          changed = state.hashId;
          break;
        }
        await sleep(30);
      }
      if (!changed) break;
      await sleep(80);
      await waitSlideDone();
      await sleep(120);
      ahead.push(changed);
    }
    // 预取窗口之外的 t512 只应该是墙阶段遗留的请求或终点后两步的邻图；
    // 其它 id 就是浪费的预取。
    const prefetchOutsideVisited = [...new Set([...prefetchIds, ...idsOf(afterNet, 't512')])].filter(
      (id) => !visitedWindow.has(id) && !wallKnownT512.has(id) && !ahead.includes(id),
    );
    const t512Network = afterNet.t512
      ? Object.values(afterNet.t512.ids).reduce((sum, value) => sum + value.networkFetch, 0)
      : 0;
    const finalSharpLoaded =
      finalMeta === null || finalMeta.sharp < 1024
        ? finalState.sharp.length === 0 || finalSharpSrc !== null
        : finalSharpTier === (finalMeta.sharp >= 2048 ? 't2048' : 't1024');
    report.checks.rapid = {
      visited,
      visitedSharp,
      sharp1024,
      sharp2048,
      sharpOnlyVisited,
      finalId,
      finalSharpTier,
      finalSharpLoaded,
      duplicateNetwork,
      ahead,
      prefetchOutsideVisited,
      t512NetworkFetches: t512Network,
      tiers: {
        t512: afterNet.t512 ? { unique: afterNet.t512.unique, bytes: afterNet.t512.bytes, events: afterNet.t512.events } : null,
        t1024: afterNet.t1024 ? { unique: afterNet.t1024.unique, bytes: afterNet.t1024.bytes, events: afterNet.t1024.events } : null,
        t2048: afterNet.t2048 ? { unique: afterNet.t2048.unique, bytes: afterNet.t2048.bytes, events: afterNet.t2048.events } : null,
      },
      pass:
        visited.length === 6 &&
        visited.every((id) => (meta.get(id)?.sharp ?? 0) < 1024 || sharp1024.includes(id) || sharp2048.includes(id)) &&
        visitedSharp.every((id) => sharp1024.includes(id) || sharp2048.includes(id)) &&
        sharpOnlyVisited.length === 0 &&
        finalSharpLoaded &&
        (finalMeta === null || finalMeta.sharp < 2048 || sharp2048.includes(finalId)) &&
        duplicateNetwork.length === 0 &&
        prefetchOutsideVisited.length === 0,
    };

    // ---------- 弱网：saveData / 3g 时预取与 2048 都要让路 ----------
    await cdp.send('Page.removeScriptToEvaluateOnNewDocument', { identifier: fastConnectionScript.identifier });
    await cdp.send('Page.addScriptToEvaluateOnNewDocument', {
      source: `(() => {
        const proto = Object.getPrototypeOf(navigator.connection || {});
        if (!proto) return;
        Object.defineProperty(proto, 'effectiveType', { configurable: true, get: () => '3g' });
        Object.defineProperty(proto, 'saveData', { configurable: true, get: () => true });
      })();`,
    });
    await cdp.send('Network.clearBrowserCache');
    // 只有 hash 变化不会重载文档，addScriptToEvaluateOnNewDocument 不会执行；
    // 在 hash 前加一个无害 query 强制整页重载，让弱网覆盖生效。
    await cdp.send('Page.navigate', { url: base + '?weak=1#/t/solo?sort=name' });
    await waitWallReady('weak-sorted-solo', 60000, 12);
    const weakById = byIndex;
    const weakStart =
      weakById.has(startIndex) && weakById.has(startIndex + 1)
        ? startIndex
        : Math.min(20, Math.max(4, order.length - 3));
    const weakCenter = weakById.get(weakStart) ?? null;
    const weakPanes = [weakById.get(weakStart - 1), weakCenter, weakById.get(weakStart + 1)].filter(Boolean);
    resetNet();
    await openDetailAt(weakStart);
    await sleep(3200);
    const weakState = await viewState();
    const weakNet = summary();
    const weakT512 = idsOf(weakNet, 't512');
    const weakExtra = weakT512.filter((id) => !weakPanes.includes(id));
    const weakPrefetch = scriptPrefetched(weakNet).filter((id) => !weakPanes.includes(id));
    const weakSettings = await evaluate(`(() => {
      const connection = navigator.connection;
      return { saveData: connection?.saveData ?? null, effectiveType: connection?.effectiveType ?? null };
    })()`);
    report.checks.weak = {
      weakStart,
      weakCenter,
      weakPanes,
      paneSrcs: weakState.bases,
      connection: weakSettings,
      t512Ids: weakT512,
      weakExtra,
      weakPrefetch,
      t512Bytes: weakNet.t512?.bytes ?? 0,
      t1024Bytes: weakNet.t1024?.bytes ?? 0,
      t2048Ids: idsOf(weakNet, 't2048'),
      t2048Bytes: weakNet.t2048?.bytes ?? 0,
      pass:
        (weakSettings.saveData === true || weakSettings.effectiveType === '3g' || weakSettings.effectiveType === '2g') &&
        weakState.detail === true &&
        weakExtra.length === 0 &&
        weakPrefetch.length === 0 &&
        idsOf(weakNet, 't2048').length === 0,
    };

    report.checks.depth = await evaluate("document.body.innerText.includes('Maximum update depth')");
    report.network = {
      open: report.checks.open.t512,
      rapid: report.checks.rapid.tiers,
      weak: {
        t512Bytes: report.checks.weak.t512Bytes,
        t1024Bytes: report.checks.weak.t1024Bytes,
        t2048Bytes: report.checks.weak.t2048Bytes,
      },
      full: { open: openNet, rapid: afterNet, weak: weakNet },
    };
    report.passed =
      report.checks.a1.pass &&
      report.checks.open.pass &&
      report.checks.rapid.pass &&
      report.checks.weak.pass &&
      !report.checks.depth &&
      report.consoleErrors.length === 0;
  } catch (error) {
    report.error = String(error && error.stack ? error.stack : error);
    report.passed = false;
  } finally {
    if (cdp) cdp.close();
    try {
      child.kill();
    } catch {
      /* 已退出 */
    }
    await sleep(200);
    hardDelete(profile);
    if (staticServer) staticServer.close();
  }

  const reportPath = join(outDir, 'detail-preload-report.json');
  writeFileSync(reportPath, JSON.stringify(report, null, 2));
  report.reportPath = reportPath;
  console.log(JSON.stringify(report, null, 2));
  process.exit(report.passed ? 0 : 1);
}

main().catch((error) => {
  console.error(error);
  process.exit(2);
});
