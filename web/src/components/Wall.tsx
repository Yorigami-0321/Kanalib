import { MasonryInfiniteGrid } from '@egjs/react-infinitegrid';
import { useCallback, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { TierPaths, WallItem } from '../source/types';
import type { WallPhase } from '../state/useWall';
import { WallCard } from './WallCard';

export const WALL_GAP = 8;
/** 目标小图宽度：决定列数（窄屏保底 2 列）。 */
const TARGET_TILE = 320;
const MIN_COLUMNS = 2;
const MAX_COLUMNS = 6;
/** 每 N 条一组，给 infinitegrid 的回收（useRecycle）留分组边界。 */
const GROUP_SIZE = 24;

export interface WallViewProps {
  items: WallItem[];
  paths: TierPaths;
  phase: WallPhase;
  hasMore: boolean;
  /** 筛选（或列数档位）变化时整体重排。 */
  resetKey: string;
  loadedCount: number;
  totalCount: number;
  /** 是否处于筛选子集（文案提示“只在已加载范围内”）。 */
  filtered: boolean;
  onLoadMore: () => void;
  onOpen: (item: WallItem, index: number) => void;
  onDetail: (id: string) => void;
  onRetry: () => void;
}

interface AppendEventLike {
  reachEnd: () => void;
}

interface Metrics {
  columns: number;
  columnWidth: number;
}

/**
 * 混排无限墙：MasonryInfiniteGrid + 尺寸档（1x1/1x2/2x1/2x2）。
 * 列数按容器宽度算，item 宽度显式给出（与 columnSize 对齐），高度靠 aspect-ratio 撑出。
 */
export function WallView({
  items,
  paths,
  phase,
  hasMore,
  resetKey,
  loadedCount,
  totalCount,
  filtered,
  onLoadMore,
  onOpen,
  onDetail,
  onRetry,
}: WallViewProps) {
  const hostRef = useRef<HTMLDivElement>(null);
  const [metrics, setMetrics] = useState<Metrics | null>(null);

  useLayoutEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const measure = () => {
      const width = host.clientWidth;
      if (width <= 0) return;
      const columns = Math.min(
        MAX_COLUMNS,
        Math.max(MIN_COLUMNS, Math.round(width / TARGET_TILE)),
      );
      const columnWidth = Math.max(1, Math.floor((width - WALL_GAP * (columns - 1)) / columns));
      setMetrics((prev) =>
        prev && prev.columns === columns && prev.columnWidth === columnWidth
          ? prev
          : { columns, columnWidth },
      );
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(host);
    return () => observer.disconnect();
  }, []);

  // 只在这里响应 append 请求（不自己监听滚动，Lead 口径）。
  const handleRequestAppend = useCallback(
    (event: AppendEventLike) => {
      if (!hasMore) {
        event.reachEnd();
        return;
      }
      onLoadMore();
    },
    [hasMore, onLoadMore],
  );

  const gridKey = metrics ? `${resetKey}|${metrics.columns}` : resetKey;
  const cards = useMemo(
    () =>
      items.map((item, index) => (
        <WallCard
          key={item.id}
          item={item}
          index={index}
          paths={paths}
          columns={metrics ? metrics.columns : MIN_COLUMNS}
          columnWidth={metrics ? metrics.columnWidth : TARGET_TILE}
          gap={WALL_GAP}
          groupKey={Math.floor(index / GROUP_SIZE)}
          onOpen={onOpen}
          onDetail={onDetail}
        />
      )),
    [items, paths, metrics, onOpen, onDetail],
  );

  const empty = items.length === 0;

  return (
    <div className="wall-view" ref={hostRef}>
      {metrics && !empty ? (
        <MasonryInfiniteGrid
          key={gridKey}
          className="wall-grid"
          gap={WALL_GAP}
          column={metrics.columns}
          columnSize={metrics.columnWidth}
          align="start"
          isReachEnd={!hasMore}
          useRecycle
          threshold={800}
          onRequestAppend={handleRequestAppend}
        >
          {cards}
        </MasonryInfiniteGrid>
      ) : null}

      {empty ? (
        <div className="wall-empty">
          {phase === 'boot' || phase === 'loading' ? (
            <p>正在加载…</p>
          ) : phase === 'error' ? (
            <>
              <p className="wall-empty__title">数据加载失败</p>
              <p className="wall-empty__hint">检查数据目录是否包含发布器导出的 index.json 与分片。</p>
              <button type="button" className="button" onClick={onRetry}>
                重试
              </button>
            </>
          ) : filtered ? (
            <>
              <p className="wall-empty__title">当前筛选没有命中</p>
              <p className="wall-empty__hint">筛选只在已加载的 {loadedCount} 张里生效，可以放宽条件或继续往下滚动加载。</p>
            </>
          ) : (
            <>
              <p className="wall-empty__title">还没有图片</p>
              <p className="wall-empty__hint">发布器导出 0 条资产，或数据目录为空。</p>
            </>
          )}
        </div>
      ) : null}

      <div className="wall-footer">
        {phase === 'error' ? (
          <span className="wall-footer__error">
            加载出错：
            <button type="button" className="link-button" onClick={onRetry}>
              重试
            </button>
          </span>
        ) : hasMore ? (
          <span className="wall-footer__hint">
            {phase === 'loading' ? '加载中…' : '继续向下滚动加载更多'}
          </span>
        ) : (
          <span className="wall-footer__end">
            {filtered
              ? `筛选命中 ${items.length} 张（已加载 ${loadedCount} / 共 ${totalCount}）`
              : `已加载全部 ${items.length} 张`}
          </span>
        )}
      </div>
    </div>
  );
}
