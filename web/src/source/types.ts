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
  };
  paths?: Partial<TierPaths>;
}

export interface ShardFile {
  schema?: number;
  index?: number;
  items: WallItem[];
}

/**
 * 数据源抽象（契约 §6.3）。
 * 现在的实现是 `StaticSource`（拉静态分片）；将来动态后端加 `ApiSource`，
 * 只要保持这四个方法 + 分片顺序语义，墙/lightbox/详情页零改动。
 */
export interface WallSource {
  /** 数据根（相对站点根或绝对 URL），用于提示与错误信息。 */
  readonly dataRoot: string;
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
}
