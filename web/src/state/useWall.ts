import { useCallback, useEffect, useRef, useState } from 'react';
import type { TierPaths, WallIndex, WallItem, WallSource } from '../source/types';

export type WallPhase = 'boot' | 'loading' | 'idle' | 'end' | 'error';

export interface WallSnapshot {
  phase: WallPhase;
  index: WallIndex | null;
  items: WallItem[];
  paths: TierPaths;
  error: string | null;
  skipped: number;
  loadedShards: number;
  totalShards: number;
  hasMore: boolean;
}

export interface WallController extends WallSnapshot {
  /** 顺序追加下一片；并发调用会被合并（不会拉起全部分片）。 */
  loadMore: () => void;
  /** 重新拉 index.json 并重载首片（错误态下的重试）。 */
  retry: () => void;
  /** 详情页深链：若该 id 尚未加载，顺序补拉分片直到命中（有上限）。 */
  findItem: (id: string) => Promise<WallItem | undefined>;
  /** 详情页找不到条目时手动继续拉片。 */
  loadMoreUntilFound: (id: string) => Promise<WallItem | undefined>;
}

/** 深链定位时一次最多补拉的分片数，避免一进来就把整库拉空。 */
const MAX_PROBE_SHARDS = 8;

function messageOf(error: unknown): string {
  if (error instanceof Error) return error.message;
  return String(error);
}

/**
 * 把 `WallSource` 接到 React 状态上：init() 之后自动取首片，
 * 之后由 `loadMore()` 顺序追加。所有阶段判断都从 source 的实时状态推导。
 */
export function useWall(source: WallSource): WallController {
  const [snapshot, setSnapshot] = useState<WallSnapshot>(() => ({
    phase: 'boot',
    index: null,
    items: [],
    paths: source.paths(),
    error: null,
    skipped: 0,
    loadedShards: 0,
    totalShards: 0,
    hasMore: false,
  }));
  const busy = useRef(false);
  const alive = useRef(true);

  const sync = useCallback(
    (phase: WallPhase, error: string | null = null) => {
      const index = source.index();
      setSnapshot({
        phase,
        index,
        items: source.items().slice(),
        paths: source.paths(),
        error,
        skipped: source.skipped,
        loadedShards: source.shardsLoaded,
        totalShards: index ? index.shards.length : 0,
        hasMore: source.hasMore(),
      });
    },
    [source],
  );

  const request = useCallback(async () => {
    if (busy.current) return;
    busy.current = true;
    setSnapshot((prev) => (prev.phase === 'loading' ? prev : { ...prev, phase: 'loading' }));
    try {
      await source.loadMore();
      if (!alive.current) return;
      sync(source.hasMore() ? 'idle' : 'end');
    } catch (error) {
      if (!alive.current) return;
      sync('error', messageOf(error));
    } finally {
      busy.current = false;
    }
  }, [source, sync]);

  const boot = useCallback(async () => {
    setSnapshot((prev) => ({ ...prev, phase: 'boot', error: null }));
    try {
      await source.init();
    } catch (error) {
      if (!alive.current) return;
      sync('error', messageOf(error));
      return;
    }
    if (!alive.current) return;
    await request();
  }, [source, request, sync]);

  useEffect(() => {
    alive.current = true;
    void boot();
    return () => {
      alive.current = false;
    };
  }, [boot]);

  const loadMore = useCallback(() => {
    void request();
  }, [request]);

  const retry = useCallback(() => {
    void boot();
  }, [boot]);

  const findItem = useCallback(
    async (id: string): Promise<WallItem | undefined> => {
      const direct = source.getItem(id);
      if (direct) return direct;
      for (let i = 0; i < MAX_PROBE_SHARDS && source.hasMore(); i += 1) {
        try {
          await source.loadMore();
        } catch {
          break;
        }
        if (!alive.current) return source.getItem(id);
        const hit = source.getItem(id);
        if (hit) {
          sync(source.hasMore() ? 'idle' : 'end');
          return hit;
        }
      }
      if (alive.current) sync(source.hasMore() ? 'idle' : 'end');
      return source.getItem(id);
    },
    [source, sync],
  );

  const loadMoreUntilFound = useCallback(
    async (id: string): Promise<WallItem | undefined> => {
      if (!source.hasMore()) return source.getItem(id);
      try {
        await source.loadMore();
      } catch (error) {
        if (alive.current) sync('error', messageOf(error));
        return undefined;
      }
      if (alive.current) sync(source.hasMore() ? 'idle' : 'end');
      return source.getItem(id);
    },
    [source, sync],
  );

  return { ...snapshot, loadMore, retry, findItem, loadMoreUntilFound };
}
