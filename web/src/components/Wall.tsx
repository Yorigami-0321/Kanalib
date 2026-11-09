import { useEffect, useMemo, useRef, useState } from 'react';
import { formatCount } from '../lib/format';
import type { TierPaths, WallItem } from '../source/types';
import type { WallPhase } from '../state/useWall';
import { spanOf, WallCard } from './WallCard';

/** 一列的目标宽度：墙的实际密度由它决定，窗口越宽列越多。 */
const TARGET_CELL = 76;
const MIN_COLUMNS = 8;
const MAX_COLUMNS = 26;
/** 墙层左右留白 + 贴片间隙，与 styles.css 的 .wall / --gap 对齐。 */
const WALL_PAD = 10;
const WALL_GAP = 6;

interface Metrics {
  columns: number;
  cell: number;
}

function measure(hostWidth: number): Metrics {
  const usable = Math.max(hostWidth - WALL_PAD * 2, 120);
  const columns = Math.min(MAX_COLUMNS, Math.max(MIN_COLUMNS, Math.round(usable / TARGET_CELL)));
  const cell = Math.floor((usable - WALL_GAP * (columns - 1)) / columns);
  return { columns, cell: Math.max(24, cell) };
}

export interface WallViewProps {
  items: WallItem[];
  paths: TierPaths;
  phase: WallPhase;
  hasMore: boolean;
  resetKey: string;
  loadedCount: number;
  totalCount: number;
  filtered: boolean;
  onLoadMore: () => void;
  onOpen: (item: WallItem, index: number) => void;
  onDetail: (id: string) => void;
  onRetry: () => void;
}

/**
 * 密集拼贴墙：固定列数的 CSS grid + 按宽高比给每片算跨度，
 * 渲染量交给浏览器，追加由墙尾哨兵的 IntersectionObserver 触发。
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
  const hostRef = useRef<HTMLDivElement | null>(null);
  const sentinelRef = useRef<HTMLDivElement | null>(null);
  const [metrics, setMetrics] = useState<Metrics>({ columns: 10, cell: 120 });

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;
    const apply = (width: number) => {
      setMetrics((current) => {
        const next = measure(width);
        return current.columns === next.columns && current.cell === next.cell ? current : next;
      });
    };
    apply(host.clientWidth);
    if (typeof ResizeObserver === 'undefined') {
      const onResize = () => apply(host.clientWidth);
      window.addEventListener('resize', onResize);
      return () => window.removeEventListener('resize', onResize);
    }
    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (entry) apply(entry.contentRect.width);
    });
    observer.observe(host);
    return () => observer.disconnect();
  }, []);

  // 墙尾哨兵：进入视口前 800px 就预取下一片，滚到底时不会等。
  useEffect(() => {
    const sentinel = sentinelRef.current;
    if (!sentinel || !hasMore) return;
    if (typeof IntersectionObserver === 'undefined') return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) onLoadMore();
      },
      { rootMargin: '800px 0px' },
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [hasMore, onLoadMore, resetKey, items.length]);

  const spans = useMemo(() => items.map((item) => spanOf(item)), [items]);

  const gridStyle = useMemo(
    () => ({
      gridTemplateColumns: 'repeat(' + metrics.columns + ', minmax(0, 1fr))',
      gridAutoRows: metrics.cell + 'px',
    }),
    [metrics],
  );

  const isEmpty = items.length === 0;
  const showEmpty = isEmpty && phase !== 'boot' && phase !== 'loading';

  return (
    <div className="wall" ref={hostRef}>
      {showEmpty ? (
        <div className="wall__note">
          {phase === 'error' ? (
            <>
              <strong>数据源没加载成功</strong>
              检查一下 web/public/data 是否已导出、服务是否在站点根下运行。
              <button type="button" onClick={onRetry}>
                重试
              </button>
            </>
          ) : filtered ? (
            <>
              <strong>没有符合条件的图片</strong>
              换个标签或清掉筛选再看。
            </>
          ) : (
            <>
              <strong>还没有图片</strong>
              先在工作台里跑一次发布导出。
            </>
          )}
        </div>
      ) : (
        <div className="wall__grid" style={gridStyle}>
          {items.map((item, index) => (
            <WallCard
              key={item.id}
              item={item}
              index={index}
              paths={paths}
              span={spans[index]}
              onOpen={onOpen}
              onDetail={onDetail}
            />
          ))}
        </div>
      )}

      {hasMore ? <div className="wall__sentinel" ref={sentinelRef} /> : null}

      <div className="wall__status">
        <span>
          已加载 <b>{formatCount(loadedCount)}</b> / {formatCount(totalCount)}
          {filtered ? <>（筛后 <b>{formatCount(items.length)}</b>）</> : null}
        </span>
        <span>
          {hasMore
            ? phase === 'loading'
              ? '正在加载…'
              : '继续向下滚动'
            : phase === 'error'
              ? '加载中断'
              : '到底了'}
        </span>
      </div>
    </div>
  );
}
