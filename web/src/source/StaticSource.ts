import { dataUrl, mergePaths } from './config';
import {
  type ShardRef,
  type TierPaths,
  type WallIndex,
  type WallItem,
  type WallSource,
  type TagTriple,
} from './types';

/** 契约 §5：id 只允许小写数字字母，长度 40 或 64。 */
const ID_RE = /^(?:[0-9a-z]{40}|[0-9a-z]{64})$/;

export class DataError extends Error {
  constructor(message: string, readonly cause_?: unknown) {
    super(message);
    this.name = 'DataError';
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function num(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

function str(value: unknown, fallback: string): string {
  return typeof value === 'string' ? value : fallback;
}

function normalizeTags(raw: unknown): TagTriple[] {
  if (!Array.isArray(raw)) return [];
  const out: TagTriple[] = [];
  for (const entry of raw) {
    if (!Array.isArray(entry) || entry.length < 2) continue;
    const [name, category, score] = entry as unknown[];
    if (typeof name !== 'string' || name.length === 0) continue;
    out.push([
      name,
      typeof category === 'string' ? category : 'general',
      typeof score === 'number' && Number.isFinite(score) ? score : 0,
    ]);
  }
  return out;
}

/**
 * 把一条原始 JSON 规范成 `WallItem`；不合约必填字段则返回 null（调用方跳过并计数）。
 * 未知字段一律忽略（契约 §4）。
 */
export function normalizeItem(raw: unknown): WallItem | null {
  if (!isRecord(raw)) return null;
  const id = str(raw.id, '');
  if (!ID_RE.test(id)) return null;
  const w = num(raw.w, 0);
  const h = num(raw.h, 0);
  if (w <= 0 || h <= 0) return null;
  // 契约 §4：`g` = assets.group 原值；`group`/`gn`/`grp` 是历史/兼容写法。
  const groupRaw = raw.g ?? raw.group ?? raw.gn ?? raw.grp;
  const item: WallItem = {
    id,
    w,
    h,
    n: str(raw.n, id.slice(0, 8)),
    rt: str(raw.rt, 'G') || 'G',
    st: Math.min(5, Math.max(0, Math.round(num(raw.st, 0)))),
    fav: raw.fav === true,
    tags: normalizeTags(raw.tags),
    mtime: num(raw.mtime, 0),
  };
  const ph = str(raw.ph, '');
  if (ph.length > 0) item.ph = ph.toLowerCase();
  const orig = str(raw.orig, '');
  if (orig.length > 0) item.orig = orig.replace(/^\./, '').toLowerCase();
  if (typeof groupRaw === 'string' && groupRaw.length > 0) item.group = groupRaw;
  return item;
}

/**
 * 静态分片数据源（契约 §6.1/§6.3）：
 * `init()` 取 index.json；`loadMore()` **按顺序**追加下一片，绝不并发拉全部分片。
 */
export class StaticSource implements WallSource {
  readonly dataRoot: string;

  private _index: WallIndex | null = null;
  private _items: WallItem[] = [];
  private _paths: TierPaths = mergePaths(undefined);
  private shards: ShardRef[] = [];
  private cursor = 0;
  private inflight: Promise<WallItem[]> | null = null;
  private seen = new Set<string>();
  /** 被丢弃的不合约条目数（用于状态栏提示，不打断渲染）。 */
  skipped = 0;
  /** 已成功拉取的分片数。 */
  shardsLoaded = 0;

  constructor(dataRoot: string) {
    this.dataRoot = dataRoot;
  }

  index(): WallIndex | null {
    return this._index;
  }

  paths(): TierPaths {
    return this._paths;
  }

  items(): readonly WallItem[] {
    return this._items;
  }

  getItem(id: string): WallItem | undefined {
    return this._items.find((item) => item.id === id);
  }

  hasMore(): boolean {
    return this.cursor < this.shards.length;
  }

  async init(): Promise<WallIndex> {
    const url = dataUrl('index.json');
    const payload = await this.fetchJson(url);
    if (!isRecord(payload)) throw new DataError(`index.json 不是对象：${url}`);

    const schema = num(payload.schema, 0);
    const shardsRaw = Array.isArray(payload.shards) ? payload.shards : [];
    const shards: ShardRef[] = [];
    for (const entry of shardsRaw) {
      if (!isRecord(entry)) continue;
      const path = str(entry.path, '');
      if (path.length === 0) continue;
      shards.push({ i: num(entry.i, shards.length), path, count: num(entry.count, 0) });
    }
    shards.sort((a, b) => a.i - b.i);

    const index: WallIndex = {
      schema,
      count: num(payload.count, 0),
      shard_size: num(payload.shard_size, 0),
      shards,
      facets: isRecord(payload.facets)
        ? {
            tags: Array.isArray(payload.facets.tags)
              ? (payload.facets.tags as unknown[])
                  .filter(isRecord)
                  .map((tag) => ({
                    name: str(tag.name, ''),
                    category: str(tag.category, 'general'),
                    count: num(tag.count, 0),
                  }))
                  .filter((tag) => tag.name.length > 0)
              : undefined,
            groups: Array.isArray(payload.facets.groups)
              ? (payload.facets.groups as unknown[]).filter(
                  (group): group is string => typeof group === 'string' && group.length > 0,
                )
              : undefined,
          }
        : undefined,
      paths: isRecord(payload.paths) ? (payload.paths as Partial<TierPaths>) : undefined,
    };
    if (typeof payload.generated_at === 'string') index.generated_at = payload.generated_at;

    this._index = index;
    this._paths = mergePaths(index.paths);
    this.shards = shards;
    this.cursor = 0;
    this._items = [];
    this.seen = new Set();
    this.skipped = 0;
    this.shardsLoaded = 0;
    return index;
  }

  loadMore(): Promise<WallItem[]> {
    if (!this._index) {
      return Promise.reject(new DataError('StaticSource: 还没有 init()，无法加载分片'));
    }
    if (this.inflight) return this.inflight;
    if (!this.hasMore()) return Promise.resolve([] as WallItem[]);
    const shard = this.shards[this.cursor];
    // 先推进游标再发请求：失败时这一片记为“已消费”，避免自动滚动无限重试同一片。
    this.cursor += 1;
    this.inflight = this.fetchShard(shard).finally(() => {
      this.inflight = null;
    });
    return this.inflight;
  }

  private async fetchShard(shard: ShardRef): Promise<WallItem[]> {
    const url = dataUrl(shard.path);
    const payload = await this.fetchJson(url);
    const rawList = isRecord(payload) ? payload.items : undefined;
    const rawItems: unknown[] = Array.isArray(rawList) ? rawList : [];
    const appended: WallItem[] = [];
    for (const raw of rawItems) {
      const item = normalizeItem(raw);
      if (!item) {
        this.skipped += 1;
        continue;
      }
      if (this.seen.has(item.id)) continue;
      this.seen.add(item.id);
      appended.push(item);
    }
    this._items = this._items.concat(appended);
    this.shardsLoaded += 1;
    return appended;
  }

  private async fetchJson(url: string): Promise<unknown> {
    let response: Response;
    try {
      response = await fetch(url, { cache: 'no-cache' });
    } catch (error) {
      throw new DataError(`无法访问 ${url}`, error);
    }
    if (!response.ok) {
      throw new DataError(`${url} 返回 HTTP ${response.status}`);
    }
    try {
      return await response.json();
    } catch (error) {
      throw new DataError(`${url} 不是合法 JSON`, error);
    }
  }
}
