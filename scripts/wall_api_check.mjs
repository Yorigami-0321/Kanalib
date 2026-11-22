#!/usr/bin/env node
/**
 * 动态后端验收（只读）：
 * - `/api/index` 形状、capabilities、filters/counts/facets；
 * - `/api/items` 游标分页、400 invalid_cursor/invalid_limit；
 * - `/api/items/{id}` ETag；`/media/*` Range/304（给 --media-root 时）；
 * - `--ui` 时用 headless Chrome 断言页面真的把筛选/排序发成 setQuery 请求，
 *   且 API 模式不会再拉 search-index.json / 全量分片。
 *
 * 用法（先启动 kanalib-web，并把 web/dist 以 api 模式构建）：
 *   node scripts/wall_api_check.mjs --base http://127.0.0.1:8787/ \
 *     --media-root D:/Code/Kanalib/web/public/data --ui
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

function arg(name, fallback) {
  const index = process.argv.indexOf('--' + name);
  return index >= 0 ? process.argv[index + 1] : fallback;
}

function flag(name) {
  return process.argv.includes('--' + name);
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
    await new Promise((resolve, reject) => {
      this.ws.addEventListener('open', resolve, { once: true });
      this.ws.addEventListener('error', () => reject(new Error('CDP 连接失败')), { once: true });
    });
    this.ws.addEventListener('message', (event) => {
      const message = JSON.parse(event.data);
      if (message.id && this.pending.has(message.id)) {
        const { resolve, reject } = this.pending.get(message.id);
        this.pending.delete(message.id);
        if (message.error) reject(new Error(message.method + ': ' + message.error.message));
        else resolve(message.result);
        return;
      }
      for (const listener of this.listeners) listener(message);
    });
  }

  on(listener) {
    this.listeners.push(listener);
  }

  send(method, params = {}) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params }));
    });
  }

  close() {
    try {
      this.ws.close();
    } catch {
      /* noop */
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

function check(report, label, ok, detail = undefined) {
  report.checks[label] = ok ? { ok: true, ...detail } : { ok: false, ...detail };
  if (!ok) report.failures.push(label);
  return ok;
}

async function getJson(base, path) {
  const url = new URL(path, base).href;
  const response = await fetch(url, { cache: 'no-store' });
  let body = null;
  try {
    body = await response.json();
  } catch {
    body = null;
  }
  return { url, status: response.status, headers: response.headers, body };
}

async function checkApi(report, base, mediaRoot) {
  const index = await getJson(base, 'api/index');
  check(report, 'index/status', index.status === 200, { status: index.status });
  const idx = index.body ?? {};
  check(report, 'index/schema', idx.schema === 1, { schema: idx.schema });
  check(report, 'index/api_version', idx.api_version === 1, { api_version: idx.api_version });
  check(report, 'index/count', typeof idx.count === 'number', { count: idx.count });
  check(
    report,
    'index/capabilities',
    idx.capabilities?.source === 'api' &&
      idx.capabilities?.server_search === true &&
      idx.capabilities?.server_filter === true,
    { capabilities: idx.capabilities },
  );
  check(report, 'index/counts', typeof idx.counts?.favorite === 'number', {
    counts: idx.counts,
  });
  check(
    report,
    'index/rating_counts',
    ['g', 's', 'q', 'e'].every((key) => typeof idx.counts?.ratings?.[key] === 'number'),
    { ratings: idx.counts?.ratings },
  );
  const tags = Array.isArray(idx.facets?.tags) ? idx.facets.tags : [];
  check(
    report,
    'index/tag_facets',
    tags.length > 0 &&
      tags.every((tag) => typeof tag.name === 'string' && typeof tag.count === 'number') &&
      tags.some((tag) => typeof tag.cover === 'string' && tag.cover.length > 0),
    { tags: tags.slice(0, 3) },
  );
  const groups = Array.isArray(idx.facets?.groups) ? idx.facets.groups : [];
  check(
    report,
    'index/group_facets',
    groups.every((group) => typeof group.name === 'string' && typeof group.count === 'number'),
    { groups: groups.slice(0, 3) },
  );

  const filtered = await getJson(base, 'api/index?q=1girl');
  check(report, 'index/filtered', filtered.status === 200 && typeof filtered.body?.count === 'number', {
    count: filtered.body?.count,
  });

  const page1 = await getJson(base, 'api/items?limit=1');
  const first = page1.body?.items?.[0];
  check(
    report,
    'items/page1',
    page1.status === 200 && Array.isArray(page1.body?.items) && page1.body.items.length === 1,
    { status: page1.status },
  );
  check(report, 'items/next_cursor', typeof page1.body?.next === 'string', {
    next: page1.body?.next,
  });
  // `PageDto.total` 是 Option + skip_serializing_if，API 翻页不返回计数字段。
  check(report, 'items/total_omitted', !Object.hasOwn(page1.body ?? {}, 'total'), {
    total: page1.body?.total,
  });

  if (typeof page1.body?.next === 'string') {
    const page2 = await getJson(
      base,
      'api/items?limit=1&cursor=' + encodeURIComponent(page1.body.next),
    );
    const second = page2.body?.items?.[0];
    check(
      report,
      'items/page2',
      page2.status === 200 && second && second.id !== first?.id,
      { status: page2.status, second: second?.id },
    );
  }

  const badLimit = await getJson(base, 'api/items?limit=abc');
  check(
    report,
    'items/invalid_limit',
    badLimit.status === 400 && badLimit.body?.error?.code === 'invalid_limit',
    { status: badLimit.status, body: badLimit.body },
  );

  const cursor = page1.body?.next;
  if (typeof cursor === 'string') {
    const mismatch = await getJson(
      base,
      'api/items?limit=1&sort=name&order=asc&cursor=' + encodeURIComponent(cursor),
    );
    check(
      report,
      'items/invalid_cursor',
      mismatch.status === 400 && mismatch.body?.error?.code === 'invalid_cursor',
      { status: mismatch.status, body: mismatch.body },
    );
  }

  if (first?.id) {
    const detail = await getJson(base, 'api/items/' + first.id);
    const etag = detail.headers.get('etag') ?? '';
    check(report, 'items/detail', detail.status === 200 && detail.body?.id === first.id, {
      status: detail.status,
    });
    check(report, 'items/detail_etag', etag.startsWith('W/"'), { etag });
  }

  if (mediaRoot) {
    const ids = new Set();
    const listing = await getJson(base, 'api/items?limit=10');
    for (const item of listing.body?.items ?? []) ids.add(item.id);
    if (first?.id) ids.add(first.id);
    const file = [...ids].find((id) => existsSync(join(mediaRoot, 't256', `${id}.jpg`)));
    check(report, 'media/sample', Boolean(file), { mediaRoot, ids: [...ids].slice(0, 3) });
    if (file) {
      const url = new URL(`media/t256/${file}.jpg`, base).href;
      const ranged = await fetch(url, { headers: { range: 'bytes=0-3' } });
      check(report, 'media/range', ranged.status === 206, { status: ranged.status });
      check(report, 'media/range_header', Boolean(ranged.headers.get('content-range')), {
        contentRange: ranged.headers.get('content-range'),
      });
      const etag = ranged.headers.get('etag') ?? '';
      check(report, 'media/etag', etag.startsWith('W/"'), { etag });
      if (etag) {
        const cached = await fetch(url, { headers: { 'if-none-match': etag } });
        check(report, 'media/304', cached.status === 304, { status: cached.status });
      }
    }
  }
}

async function checkUi(report, base) {
  const chrome = CANDIDATES.find((path) => existsSync(path));
  if (!chrome) throw new Error('找不到 Chrome/Edge');

  const profile = mkdtempSync(join(tmpdir(), 'kanalib-api-'));
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
      '--disable-gpu',
      '--no-sandbox',
      '--hide-scrollbars',
      '--window-size=1440,900',
      'about:blank',
    ],
    { stdio: 'ignore' },
  );

  const errors = [];
  const requests = [];
  let cdp = null;
  try {
    await waitForDevtools(port);
    const target = await newTarget(port, 'about:blank');
    cdp = new Cdp(target.webSocketDebuggerUrl);
    await cdp.connect();
    cdp.on((message) => {
      if (message.method === 'Network.requestWillBeSent') {
        requests.push(message.params.request.url);
      } else if (message.method === 'Runtime.exceptionThrown') {
        errors.push(message.params.exceptionDetails?.text ?? 'exception');
      } else if (
        message.method === 'Runtime.consoleAPICalled' &&
        message.params.type === 'error'
      ) {
        errors.push(
          message.params.args.map((entry) => entry.value ?? entry.description ?? '').join(' '),
        );
      }
    });
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    await cdp.send('Network.enable');
    await cdp.send('Emulation.setDeviceMetricsOverride', {
      width: 1440,
      height: 900,
      deviceScaleFactor: 1,
      mobile: false,
    });

    const navigate = async (url, waitMs) => {
      requests.length = 0;
      await cdp.send('Page.navigate', { url });
      await sleep(waitMs);
    };
    const apiItemRequests = () => requests.filter((url) => url.includes('/api/items?'));
    const hasParam = (url, key, value) => {
      const parsed = new URL(url);
      return parsed.searchParams.get(key) === value;
    };

    await navigate(base + '#/?q=1girl', 4500);
    const searchCalls = apiItemRequests();
    check(
      report,
      'ui/search_request',
      searchCalls.some((url) => hasParam(url, 'q', '1girl')),
      { calls: searchCalls },
    );
    check(
      report,
      'ui/search_request_limit',
      searchCalls.every((url) => hasParam(url, 'limit', '60')),
      { calls: searchCalls },
    );
    check(
      report,
      'ui/no_search_index',
      !requests.some((url) => url.includes('search-index.json')),
      { requests: requests.filter((url) => url.includes('search-index')) },
    );
    check(
      report,
      'ui/no_full_load',
      searchCalls.length <= 5,
      { count: searchCalls.length },
    );

    await navigate(base + '#/?sort=name', 3500);
    const sortCalls = apiItemRequests();
    check(
      report,
      'ui/sort_request',
      sortCalls.some(
        (url) => hasParam(url, 'sort', 'name') && hasParam(url, 'order', 'asc'),
      ),
      { calls: sortCalls },
    );

    await navigate(base + '#/t/1girl', 3500);
    const tagCalls = apiItemRequests();
    check(
      report,
      'ui/tag_request',
      tagCalls.some((url) => hasParam(url, 'tag', '1girl')),
      { calls: tagCalls },
    );
    check(report, 'ui/no_console_errors', errors.length === 0, { errors });
  } finally {
    if (cdp) cdp.close();
    try {
      child.kill();
    } catch {
      /* 已退出 */
    }
    hardDelete(profile);
  }
}

async function main() {
  const raw = arg('base', 'http://127.0.0.1:8787/');
  const base = raw.endsWith('/') ? raw : raw + '/';
  const mediaRoot = arg('media-root', '');
  const out = arg('out', '');

  const report = { base, failures: [], checks: {}, passed: false };
  try {
    await checkApi(report, base, mediaRoot);
    if (flag('ui')) await checkUi(report, base);
  } catch (error) {
    report.failures.push('exception: ' + (error instanceof Error ? error.message : String(error)));
  }
  report.passed = report.failures.length === 0;

  if (out) writeFileSync(out, JSON.stringify(report, null, 2));
  console.log(JSON.stringify({ passed: report.passed, failures: report.failures, out: out || null }));
  process.exit(report.passed ? 0 : 1);
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
