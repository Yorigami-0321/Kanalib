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
  /** 从失败分片续拉（不重载 index、不清空已加载）；index 本身失败时才会重新 boot。 */
  retry: () => void;
  /** 详情页深链：若该 id 尚未加载，顺序补拉分片直到命中（有上限）。 */
  findItem: (id: string) => Promise<WallItem | undefined>;
  /** 详情页找不到条目时手动继续拉片。 */
  loadMoreUntilFound: (id: string) => Promise<WallItem | undefined>;
}

/** 深链定位时一次最多补拉的分片数，避免一进来就把整库拉空。 */
const MAX_PROBE_SHARDS = 8;

/**
 * 分片失败后的自动补拉间隔。只补拉失败的那一片（游标未推进），
 * 两次仍失败就把错误交给界面，等用户点重试或切换路由。
 */
const AUTO_RETRY_DELAYS = [800, 2000];

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
  /** 自动补拉：当前已尝试次数与挂起的定时器。 */
  const autoRetry = useRef(0);
  const retryTimer = useRef(0);
  /** request 需要在自己的 catch 里调度下一次补拉，用 ref 断开自引用。 */
  const requestRef = useRef<(() => Promise<void>) | null>(null);

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
      autoRetry.current = 0;
      window.clearTimeout(retryTimer.current);
      retryTimer.current = 0;
      sync(source.hasMore() ? 'idle' : 'end');
    } catch (error) {
      if (!alive.current) return;
      sync('error', messageOf(error));
      // 失败分片没有被消费，隔一会儿自动补拉；两次仍失败就停下来等用户操作。
      const attempt = autoRetry.current;
      if (attempt < AUTO_RETRY_DELAYS.length) {
        autoRetry.current = attempt + 1;
        window.clearTimeout(retryTimer.current);
        retryTimer.current = window.setTimeout(() => {
          retryTimer.current = 0;
          if (alive.current) void requestRef.current?.();
        }, AUTO_RETRY_DELAYS[attempt]);
      }
    } finally {
      busy.current = false;
    }
  }, [source, sync]);

  // 自动补拉的定时器经 ref 回调 request；在提交后同步，避免渲染期写 ref。
  useEffect(() => {
    requestRef.current = request;
  }, [request]);

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
      window.clearTimeout(retryTimer.current);
      retryTimer.current = 0;
    };
  }, [boot]);

  const loadMore = useCallback(() => {
    void request();
  }, [request]);

  // 错误态重试：index 还在就从失败分片原样续拉（已加载的条目全部保留），
  // 只有 index.json 本身失败时才需要重新 boot。
  const retry = useCallback(() => {
    window.clearTimeout(retryTimer.current);
    retryTimer.current = 0;
    autoRetry.current = 0;
    if (!source.index()) {
      void boot();
      return;
    }
    void request();
  }, [boot, request, source]);

  const findItem = useCallback(
    async (id: string): Promise<WallItem | undefined> => {
      const direct = source.getItem(id);
      if (direct) return direct;
      // 没有更多分片：立刻返回，绝不再 sync。sync 会换掉 items 引用，
      // App 的定位 effect 会因此重跑，形成「找不到 → sync → 再找」的死循环。
      if (!source.hasMore()) return undefined;
      let progressed = false;
      for (let i = 0; i < MAX_PROBE_SHARDS && source.hasMore(); i += 1) {
        try {
          await source.loadMore();
          progressed = true;
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
      // 只有真的拉过分片才刷新快照；纯失败/无进展时保持 items 引用稳定。
      if (alive.current && progressed) sync(source.hasMore() ? 'idle' : 'end');
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
