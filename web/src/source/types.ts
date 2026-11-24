/**
 * 契约 v1 的类型定义。
 * 唯一权威：docs/backlog/photo-wall-contract.md（字段/目录/不变量以它为准）。
 * 前端必须容忍缺失的可选字段与未知的新字段（契约 §4）。
 */

/** 缩略图/原图档位。契约 §1 的目录名固定，不可配置化。 */
export type TileTier = 't256' | 't512' | 't1024' | 't2048' | 'original';

export type TierPaths = Record<TileTier, string>;

/** 契约 §2：`paths` 缺省时的兜底值。 */
export const DEFAULT_TIER_PATHS: TierPaths = {
  t256: 't256/',
  t512: 't512/',
  t1024: 't1024/',
  t2048: 't2048/',
  original: 'o/',
};

/** `[name, category, score]` 三元组，score 0–1。 */
export type TagTriple = [name: string, category: string, score: number];

export interface WallItem {
  /** 契约 §5：`[0-9a-z]{40}` 或 `[0-9a-z]{64}`；同时是缩略图文件名主干。 */
  id: string;
  /** 像素宽（>0）。 */
  w: number;
  /** 像素高（>0）。 */
  h: number;
  /** 展示名（basename）。 */
  n: string;
  /** 分级：G/S/Q/E。 */
  rt: string;
  /** 星级 0–5。 */
  st: number;
  fav: boolean;
  tags: TagTriple[];
  /** 16 位小写 hex pHash，缺失则不存在。 */
  ph?: string;
  /** UTC 秒。 */
  mtime: number;
  /** 原图扩展名（小写、无点）；缺省表示无原图。 */
  orig?: string;
  /**
   * 契约 §4 可选字段 `sharp`：本条目可用的最高锐利档长边。
   * `0` = 没有锐利档，`1024`/`2048` = 有；缺失表示旧数据未声明（前端按未知试探）。
   */
  sharp?: number;
  /**
   * 契约 §4 可选字段 `g`：`assets.group` 原值；空/缺失 = 未分组。
   * 旧版发布器/mock 数据可能用 `group`/`gn`/`grp`，一并兼容（未知字段忽略，不报错）。
   */
  group?: string;
}

export interface ShardRef {
  i: number;
  path: string;
  count: number;
}

export interface TagFacet {
  name: string;
  category: string;
  count: number;
  /** API 模式下的代表图 id；静态索引可能没有。 */
  cover?: string;
}

export interface FacetGroup {
  name: string;
  count: number;
  cover?: string;
}

export interface IndexStats {
  favorite: number;
  /** 分级 → 张数；缺键按 0 处理。 */
  ratings: Record<string, number>;
}

export interface WallIndex {
  schema: number;
  generated_at?: string;
  count: number;
  shard_size: number;
  shards: ShardRef[];
  facets?: {
    tags?: TagFacet[];
    groups?: string[];
    /** API 模式附带分组计数与封面；静态索引没有。 */
    group_details?: FacetGroup[];
  };
  stats?: IndexStats;
  paths?: Partial<TierPaths>;
}

/** 服务端声明的能力；静态源也可以给出同样形状的默认值。 */
export interface SourceCapabilities {
  source: string;
  server_search: boolean;
  server_filter: boolean;
  can_write: boolean;
  has_works: boolean;
  has_authors: boolean;
  has_collections: boolean;
  media_tiers: string[];
  original_proxy: boolean;
  generated_at?: string | null;
}

/** `/api/items` 支持的排序键；与后端 `SortKind` 对齐。 */
export type WallSort =
  | 'updated'
  | 'posted'
  | 'name'
  | 'star'
  | 'favorite'
  | 'rating'
  | 'random';

/** 服务端筛选/排序参数；静态源忽略它（仍用内存筛选）。 */
export interface WallQuery {
  q?: string;
  tag?: string;
  group?: string;
  rating?: string;
  fav?: boolean;
  sort?: WallSort;
  order?: 'asc' | 'desc';
  seed?: number;
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

/**
 * 数据源抽象（契约 §6.3）。
 * 现在的实现是 `StaticSource`（拉静态分片）；将来动态后端加 `ApiSource`，
 * 只要保持这四个方法 + 分片顺序语义，墙/lightbox/详情页零改动。
 */
export interface WallSource {
  /** 数据根（相对站点根或绝对 URL），用于提示与错误信息。 */
  readonly dataRoot: string;
  /** 服务端能力；`init()` 后可用。静态源固定 `server_search: false`。 */
  readonly capabilities?: SourceCapabilities;
  /** 拉 `index.json`，重置已加载集合与分片游标。 */
  init(): Promise<WallIndex>;
  /** 顺序追加下一片；没有更多时返回空数组。并发调用合并为同一个请求。 */
  loadMore(): Promise<WallItem[]>;
  /** 已加载条目（只读视图）。 */
  items(): readonly WallItem[];
  /** 已成功拉取的分片数（用于状态提示）。 */
  readonly shardsLoaded: number;
  /** 被丢弃的不合约条目数（缺必填字段/非法 id）。 */
  readonly skipped: number;
  getItem(id: string): WallItem | undefined;
  index(): WallIndex | null;
  hasMore(): boolean;
  /** `index.paths` 合并默认值后的档位前缀。 */
  paths(): TierPaths;
  /** 单条补齐；API 源直接取 `/api/items/{id}`，静态源不实现。 */
  fetchItem?(id: string): Promise<WallItem | undefined>;
  /** 设置服务端查询并清空已加载集合；静态源不实现。 */
  setQuery?(query: WallQuery): void;
  /** 按需加载静态搜索索引；API 源不实现（服务端已搜索）。 */
  loadSearchIndex?(): Promise<SearchIndexData | null>;
}
