import { RATING_LABELS } from '../lib/format';
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
  // 契约 §4：sharp 只认 0/1024/2048；缺省表示旧数据未声明。
  if (raw.sharp === 0 || raw.sharp === 1024 || raw.sharp === 2048) item.sharp = raw.sharp;
  if (typeof groupRaw === 'string' && groupRaw.length > 0) item.group = groupRaw;
  return item;
}

/**
 * search-index.json 的一条记录：字段全部折叠成小写、`_` 转空格，
 * 过滤时不再重复规整。`hay` 是名称/标签/分组/分级的合并草堆，普通词只扫它。
 */
export interface SearchRecord {
  id: string;
  name: string;
  tags: string[];
  group: string;
  rating: string;
  hay: string;
}

export interface SearchIndexData {
  version: number;
  count: number;
  tagTable: string[];
  groupTable: string[];
  records: Map<string, SearchRecord>;
}

/** 与 filters.ts 的折叠规则保持一致：小写、下划线转空格、压缩空白。 */
function fold(value: string): string {
  return value.toLowerCase().replace(/_+/g, ' ').replace(/\s+/g, ' ').trim();
}

function stringTable(raw: unknown): string[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .filter((entry): entry is string => typeof entry === 'string' && entry.length > 0)
    .map(fold);
}

function tableAt(table: readonly string[], index: unknown): string {
  if (typeof index !== 'number' || !Number.isInteger(index) || index < 0) return '';
  return table[index] ?? '';
}

/**
 * 标签字段兼容三种写法：索引数组（数字 → 标签表）、字符串数组、`[name, category, score]` 三元组。
 */
function tagNames(raw: unknown, table: readonly string[]): string[] {
  if (!Array.isArray(raw)) return [];
  const out: string[] = [];
  const seen = new Set<string>();
  for (const entry of raw) {
    let name = '';
    if (typeof entry === 'number') name = tableAt(table, entry);
    else if (typeof entry === 'string') name = entry;
    else if (Array.isArray(entry) && typeof entry[0] === 'string') name = entry[0];
    const folded = name.length > 0 ? fold(name) : '';
    if (folded.length === 0 || seen.has(folded)) continue;
    seen.add(folded);
    out.push(folded);
  }
  return out;
}

function buildRecord(
  id: string,
  nameRaw: unknown,
  tagsRaw: unknown,
  groupRaw: unknown,
  ratingRaw: unknown,
  tagTable: readonly string[],
  groupTable: readonly string[],
): SearchRecord {
  const name = fold(str(nameRaw, ''));
  const tags = tagNames(tagsRaw, tagTable);
  const group =
    typeof groupRaw === 'number' ? tableAt(groupTable, groupRaw) : fold(str(groupRaw, ''));
  const rating = fold(str(ratingRaw, ''));
  const ratingLabel = fold(RATING_LABELS[rating.toUpperCase()] ?? '');
  const hay = [name, tags.join(' '), group, rating, ratingLabel].filter(Boolean).join(' ');
  return { id, name, tags, group, rating, hay };
}

/**
 * 解析搜索索引。契约格式是
 * `{"v":1,"tags":[...],"groups":[...],"items":[["id","name",[tagIdx...],groupIdx,st,rt]]}`；
 * 同时兼容对象条目（`{id,n,tags,g,rt}`）与旧字段别名，未知字段忽略。
 */
export function normalizeSearchIndex(raw: unknown): SearchIndexData | null {
  if (!isRecord(raw)) return null;
  const tagTable = stringTable(raw.tags ?? raw.tag_table ?? raw.tagTable);
  const groupTable = stringTable(raw.groups ?? raw.group_table ?? raw.groupTable);
  const listRaw = raw.items ?? raw.entries ?? raw.records ?? raw.docs;
  if (!Array.isArray(listRaw)) return null;
  const records = new Map<string, SearchRecord>();
  for (const entry of listRaw) {
    if (Array.isArray(entry)) {
      // 元组：id, name, tagIdx[], groupIdx, st, rt（st 当前不参与搜索）。
      const id = str(entry[0], '');
      if (!ID_RE.test(id)) continue;
      records.set(
        id,
        buildRecord(id, entry[1], entry[2], entry[3], entry[5], tagTable, groupTable),
      );
      continue;
    }
    if (!isRecord(entry)) continue;
    const id = str(entry.id, '');
    if (!ID_RE.test(id)) continue;
    const tagsRaw = entry.tags ?? entry.t ?? entry.tag;
    const tags = typeof tagsRaw === 'string' ? [tagsRaw] : tagsRaw;
    records.set(
      id,
      buildRecord(
        id,
        entry.name ?? entry.n,
        tags,
        entry.g ?? entry.group,
        entry.rt ?? entry.r ?? entry.rating,
        tagTable,
        groupTable,
      ),
    );
  }
  if (records.size === 0) return null;
  return {
    version: num(raw.v ?? raw.version ?? raw.schema, 0),
    count: num(raw.count, records.size),
    tagTable,
    groupTable,
    records,
  };
}

/**
 * 静态分片数据源（契约 §6.1/§6.3）：
 * `init()` 取 index.json；`loadMore()` **按顺序**追加下一片，绝不并发拉全部分片。
 * `loadSearchIndex()` 按需取 search-index.json，缺失/404 时静默返回 null。
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
  /** `init()` 的代号：重载后丢弃旧请求的结果，避免旧分片追加到新集合。 */
  private generation = 0;
  /** 被丢弃的不合约条目数（用于状态栏提示，不打断渲染）。 */
  skipped = 0;
  /** 已成功拉取的分片数。 */
  shardsLoaded = 0;
  /** search-index.json 的缓存与其飞行中请求；失败只试一次。 */
  private _search: SearchIndexData | null = null;
  private searchInflight: Promise<SearchIndexData | null> | null = null;
  /** index.json 声明的搜索索引路径（可选），缺省按同目录约定探测。 */
  private searchPath: string | null = null;
  /** 进行中的 init 数（成功/失败都会减一）；搜索索引要等它归零再发请求。 */
  private initPending = 0;
  private initWaiters: Array<() => void> = [];

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
    this.initPending += 1;
    try {
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

      // index.json 可以声明搜索索引路径（字符串或 `{path}`）；缺省时前端按约定探测。
      const searchRef = payload.search ?? payload.search_index ?? payload.searchIndex;
      if (typeof searchRef === 'string' && searchRef.length > 0) this.searchPath = searchRef;
      else if (
        isRecord(searchRef) &&
        typeof searchRef.path === 'string' &&
        searchRef.path.length > 0
      ) {
        this.searchPath = searchRef.path;
      } else {
        this.searchPath = null;
      }

      this._search = null;
      this._index = index;
      this._paths = mergePaths(index.paths);
      this.shards = shards;
      this.cursor = 0;
      this.generation += 1;
      this.inflight = null;
      this._items = [];
      this.seen = new Set();
      this.skipped = 0;
      this.shardsLoaded = 0;
      return index;
    } finally {
      this.initPending -= 1;
      // 成功/失败都放行等待者：搜索索引请求不能因为 init 失败而永久挂起。
      if (this.initPending === 0) {
        const waiters = this.initWaiters;
        this.initWaiters = [];
        for (const wake of waiters) wake();
      }
    }
  }

  loadMore(): Promise<WallItem[]> {
    if (!this._index) {
      return Promise.reject(new DataError('StaticSource: 还没有 init()，无法加载分片'));
    }
    if (this.inflight) return this.inflight;
    if (!this.hasMore()) return Promise.resolve([] as WallItem[]);
    const shard = this.shards[this.cursor];
    const generation = this.generation;
    // 只有成功后才推进游标：失败的分片下次仍是同一片，可以原样补拉，
    // 不会像旧实现那样「先消费再请求」，把失败片永久跳过。
    const task: Promise<WallItem[]> = this.fetchShard(shard, generation)
      .then((appended) => {
        if (generation !== this.generation) return [] as WallItem[];
        this.cursor += 1;
        return appended;
      })
      .finally(() => {
        if (this.inflight === task) this.inflight = null;
      });
    this.inflight = task;
    return task;
  }

  /** 已就绪的搜索索引；未加载/不可用时为 null。 */
  searchIndex(): SearchIndexData | null {
    return this._search;
  }

  /** 等当前进行中的 init 落定（成功或失败）；没有 init 在跑时立即返回。 */
  private async waitForInit(): Promise<void> {
    while (this.initPending > 0) {
      await new Promise<void>((resolve) => this.initWaiters.push(resolve));
    }
  }

  private async loadSearchIndexOnce(): Promise<SearchIndexData | null> {
    // 首屏带 q 时，搜索 effect 可能早于 init() 完成。必须等 init 落定：
    // 既保证 searchPath 已从 index.json 读到，也避开 init 里 generation 自增把结果作废。
    await this.waitForInit();
    if (this._search) return this._search;
    // 请求期间又发生了一次 init：旧代数据作废，等新 init 落定后重试一次（有界，不无限循环）。
    for (let attempt = 0; attempt < 2; attempt += 1) {
      const generation = this.generation;
      const url = dataUrl(this.searchPath ?? 'search-index.json');
      let response: Response;
      try {
        response = await fetch(url, { cache: 'no-cache' });
      } catch {
        return null;
      }
      if (!response.ok) return null;
      let payload: unknown;
      try {
        payload = await response.json();
      } catch {
        return null;
      }
      const data = normalizeSearchIndex(payload);
      if (!data) return null;
      if (generation === this.generation) {
        this._search = data;
        return data;
      }
      await this.waitForInit();
      if (this._search) return this._search;
    }
    return null;
  }

  /**
   * 按需加载 search-index.json。404、离线、JSON 损坏、结构不合约都静默返回 null，
   * 由调用方退回文件名搜索；不写 console，也不抛出。
   */
  loadSearchIndex(): Promise<SearchIndexData | null> {
    // 重新 init 期间旧缓存已不代表当前数据，不能直接返回；等 init 落定后再取。
    if (this.initPending === 0 && this._search) return Promise.resolve(this._search);
    if (this.searchInflight) return this.searchInflight;
    const task: Promise<SearchIndexData | null> = this.loadSearchIndexOnce().finally(() => {
      if (this.searchInflight === task) this.searchInflight = null;
    });
    this.searchInflight = task;
    return task;
  }

  private async fetchShard(shard: ShardRef, generation: number): Promise<WallItem[]> {
    const url = dataUrl(shard.path);
    const payload = await this.fetchJson(url);
    // 请求期间重新 init 过：这份结果属于上一代数据，直接丢弃。
    if (generation !== this.generation) return [];
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
