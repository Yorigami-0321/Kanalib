import { API_PAGE_SIZE, API_ROOT, apiUrl, mergePaths } from './config';
import { DataError, isValidItemId, normalizeItem } from './StaticSource';
import {
  type FacetGroup,
  type IndexStats,
  type SourceCapabilities,
  type TierPaths,
  type WallIndex,
  type WallItem,
  type WallQuery,
  type WallSource,
} from './types';

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function num(value: unknown, fallback: number): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : fallback;
}

function str(value: unknown, fallback: string): string {
  return typeof value === 'string' ? value : fallback;
}

function defaultCapabilities(): SourceCapabilities {
  return {
    source: 'api',
    server_search: true,
    server_filter: true,
    can_write: false,
    has_works: true,
    has_authors: true,
    has_collections: true,
    media_tiers: ['t256', 't512', 't1024', 't2048'],
    original_proxy: false,
    generated_at: null,
  };
}

function normalizeCapabilities(raw: unknown): SourceCapabilities {
  const fallback = defaultCapabilities();
  if (!isRecord(raw)) return fallback;
  return {
    source: str(raw.source, fallback.source),
    server_search: raw.server_search === true,
    server_filter: raw.server_filter === true,
    can_write: raw.can_write === true,
    has_works: raw.has_works === true,
    has_authors: raw.has_authors === true,
    has_collections: raw.has_collections === true,
    media_tiers: Array.isArray(raw.media_tiers)
      ? raw.media_tiers.filter((tier): tier is string => typeof tier === 'string')
      : fallback.media_tiers,
    original_proxy: raw.original_proxy === true,
    generated_at: typeof raw.generated_at === 'string' ? raw.generated_at : null,
  };
}

function queryParams(query: WallQuery): URLSearchParams {
  const params = new URLSearchParams();
  if (query.q && query.q.trim().length > 0) params.set('q', query.q.trim());
  if (query.tag) params.set('tag', query.tag);
  if (query.group) params.set('group', query.group);
  if (query.rating) params.set('rating', query.rating);
  if (query.fav !== undefined) params.set('fav', query.fav ? '1' : '0');
  if (query.sort) params.set('sort', query.sort);
  if (query.order) params.set('order', query.order);
  if (query.seed !== undefined) params.set('seed', String(query.seed));
  return params;
}

/**
 * 动态后端的 `WallSource`：`/api/index` 提供计数/facets/capabilities，
 * `/api/items` 用不透明游标顺序翻页。分片语义保留为“页”：
 * `shardsLoaded` 是已拉页数，`index.shards` 固定为空。
 */
export class ApiSource implements WallSource {
  readonly dataRoot: string;

  private _capabilities: SourceCapabilities | null = null;
  private _index: WallIndex | null = null;
  private _items: WallItem[] = [];
  private _paths: TierPaths = mergePaths(undefined);
  private byId = new Map<string, WallItem>();
  private next: string | null = null;
  /** 还没拉过第一页：此时没有游标，`hasMore()` 也必须为 true。 */
  private first = true;
  private inflight: Promise<WallItem[]> | null = null;
  private generation = 0;
  private query: WallQuery = {};
  skipped = 0;
  shardsLoaded = 0;

  constructor(dataRoot: string = API_ROOT) {
    this.dataRoot = dataRoot;
  }

  get capabilities(): SourceCapabilities | undefined {
    return this._capabilities ?? undefined;
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
    return this.byId.get(id);
  }

  hasMore(): boolean {
    return this.first || this.next !== null;
  }

  /** 记录服务端查询并清空集合；下一次 `init()` 会带上新参数。 */
  setQuery(query: WallQuery): void {
    this.query = { ...query };
    this.generation += 1;
    this.inflight = null;
    this._items = [];
    this.byId.clear();
    this.next = null;
    this.first = true;
    this.skipped = 0;
    this.shardsLoaded = 0;
  }

  async init(): Promise<WallIndex> {
    const generation = (this.generation += 1);
    const params = queryParams(this.query);
    const suffix = params.toString();
    const payload = await this.fetchJson(apiUrl(suffix ? `index?${suffix}` : 'index'));
    if (!isRecord(payload)) throw new DataError('index 响应不是对象');

    const facetsRaw = isRecord(payload.facets) ? payload.facets : undefined;
    const groupsRaw = facetsRaw && Array.isArray(facetsRaw.groups) ? facetsRaw.groups : [];
    const groupDetails: FacetGroup[] = [];
    for (const entry of groupsRaw) {
      if (typeof entry === 'string' && entry.length > 0) {
        groupDetails.push({ name: entry, count: 0 });
        continue;
      }
      if (!isRecord(entry)) continue;
      const name = str(entry.name, '');
      if (name.length === 0) continue;
      const group: FacetGroup = { name, count: num(entry.count, 0) };
      if (typeof entry.cover === 'string' && entry.cover.length > 0) group.cover = entry.cover;
      groupDetails.push(group);
    }
    const index: WallIndex = {
      schema: num(payload.schema, 1),
      count: num(payload.count, 0),
      shard_size: API_PAGE_SIZE,
      shards: [],
      facets: facetsRaw
        ? {
            tags: Array.isArray(facetsRaw.tags)
              ? facetsRaw.tags
                  .filter(isRecord)
                  .map((tag) => {
                    const facet = {
                      name: str(tag.name, ''),
                      category: str(tag.category, 'general'),
                      count: num(tag.count, 0),
                    } as { name: string; category: string; count: number; cover?: string };
                    if (typeof tag.cover === 'string' && tag.cover.length > 0) {
                      facet.cover = tag.cover;
                    }
                    return facet;
                  })
                  .filter((tag) => tag.name.length > 0)
              : undefined,
            groups: groupDetails.map((group) => group.name),
            group_details: groupDetails,
          }
        : undefined,
      paths: isRecord(payload.paths) ? (payload.paths as Partial<TierPaths>) : undefined,
    };
    if (typeof payload.generated_at === 'string') index.generated_at = payload.generated_at;

    if (isRecord(payload.counts)) {
      const counts = payload.counts;
      const ratings = isRecord(counts.ratings) ? counts.ratings : {};
      const stats: IndexStats = {
        favorite: num(counts.favorite, 0),
        ratings: {
          G: num(ratings.g, 0),
          S: num(ratings.s, 0),
          Q: num(ratings.q, 0),
          E: num(ratings.e, 0),
        },
      };
      index.stats = stats;
    }

    // init 期间又发生了 setQuery/init：旧响应作废。
    if (generation !== this.generation) return index;

    this._capabilities = normalizeCapabilities(payload.capabilities);
    this._index = index;
    this._paths = mergePaths(index.paths);
    this._items = [];
    this.byId.clear();
    this.next = null;
    this.first = true;
    this.inflight = null;
    this.skipped = 0;
    this.shardsLoaded = 0;
    return index;
  }

  loadMore(): Promise<WallItem[]> {
    if (!this._index) {
      return Promise.reject(new DataError('ApiSource: 还没有 init()，无法加载条目'));
    }
    if (this.inflight) return this.inflight;
    if (!this.hasMore()) return Promise.resolve([] as WallItem[]);

    const generation = this.generation;
    const cursor = this.first ? null : this.next;
    const task: Promise<WallItem[]> = this.fetchPage(cursor, generation).finally(() => {
      if (this.inflight === task) this.inflight = null;
    });
    this.inflight = task;
    return task;
  }

  /** 单条补齐：未命中返回 undefined；其他网络/协议错误照常抛出。 */
  async fetchItem(id: string): Promise<WallItem | undefined> {
    const hit = this.byId.get(id);
    if (hit) return hit;
    if (!isValidItemId(id)) return undefined;
    const payload = await this.fetchJson(apiUrl(`items/${id}`), true);
    if (payload === undefined) return undefined;
    const item = normalizeItem(payload);
    if (!item) return undefined;
    this.byId.set(item.id, item);
    return item;
  }

  private async fetchPage(
    cursor: string | null,
    generation: number,
  ): Promise<WallItem[]> {
    const params = queryParams(this.query);
    params.set('limit', String(API_PAGE_SIZE));
    if (cursor) params.set('cursor', cursor);
    const payload = await this.fetchJson(apiUrl(`items?${params.toString()}`));
    // 请求期间重新 init/setQuery：这份结果属于上一代数据，直接丢弃。
    if (generation !== this.generation) return [];
    if (!isRecord(payload)) throw new DataError('items 响应不是对象');

    const rawItems: unknown[] = Array.isArray(payload.items) ? payload.items : [];
    const appended: WallItem[] = [];
    for (const raw of rawItems) {
      const item = normalizeItem(raw);
      if (!item) {
        this.skipped += 1;
        continue;
      }
      if (this.byId.has(item.id)) continue;
      this.byId.set(item.id, item);
      appended.push(item);
    }
    this._items = this._items.concat(appended);
    this.first = false;
    this.next = typeof payload.next === 'string' && payload.next.length > 0 ? payload.next : null;
    this.shardsLoaded += 1;
    return appended;
  }

  /** `allowMissing` 为 true 时把 404 当作“没有这条”，其余错误照常抛出。 */
  private async fetchJson(url: string, allowMissing = false): Promise<unknown> {
    let response: Response;
    try {
      response = await fetch(url, { cache: 'no-cache' });
    } catch (error) {
      throw new DataError(`无法访问 ${url}`, error);
    }
    if (allowMissing && response.status === 404) return undefined;
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
