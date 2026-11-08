import { useCallback, useEffect, useRef, useState } from 'react';
import { gridSources } from '../lib/assetUrl';
import { formatStars } from '../lib/format';
import type { TierPaths, WallItem } from '../source/types';

/** 单击/双击去抖窗口（契约 & 计划 §3.2：~220ms）。 */
export const CLICK_DELAY_MS = 220;

export type SizeTier = 'square' | 'tall' | 'wide' | 'big';

/**
 * 尺寸档按 w/h 比例算（不按索引奇偶）：
 * - 横图（ratio >= 1.7）在够宽的墙上占 2x1，做节奏点；
 * - 近方的大图（min 边 >= 1000）占 2x2，突出主图；
 * - 竖图天然 1x2，方图 1x1。
 */
export function sizeTierOf(item: WallItem, columns: number): { tier: SizeTier; span: number } {
  const ratio = item.w / item.h;
  const large = Math.min(item.w, item.h) >= 1000;
  if (columns >= 3 && ratio >= 1.7) return { tier: 'wide', span: 2 };
  if (columns >= 4 && large && ratio >= 0.8 && ratio <= 1.25) return { tier: 'big', span: 2 };
  if (ratio <= 0.68) return { tier: 'tall', span: 1 };
  return { tier: 'square', span: 1 };
}

export interface WallCardProps {
  item: WallItem;
  index: number;
  paths: TierPaths;
  columns: number;
  columnWidth: number;
  gap: number;
  groupKey: number;
  onOpen: (item: WallItem, index: number) => void;
  onDetail: (id: string) => void;
}

export function WallCard({
  item,
  index,
  paths,
  columns,
  columnWidth,
  gap,
  groupKey,
  onOpen,
  onDetail,
}: WallCardProps) {
  const { tier, span } = sizeTierOf(item, columns);
  const width = span * columnWidth + (span - 1) * gap;
  const sources = gridSources(paths, item);
  const [sourceIndex, setSourceIndex] = useState(0);
  const timer = useRef<number | null>(null);

  useEffect(() => {
    setSourceIndex(0);
  }, [item.id]);

  const clearTimer = useCallback(() => {
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
  }, []);

  useEffect(() => clearTimer, [clearTimer]);

  const handleClick = useCallback(() => {
    clearTimer();
    timer.current = window.setTimeout(() => {
      timer.current = null;
      onOpen(item, index);
    }, CLICK_DELAY_MS);
  }, [clearTimer, index, item, onOpen]);

  const handleDoubleClick = useCallback(() => {
    // 双击取消待触发的单击，直接路由到详情（#/a/<id>）。
    clearTimer();
    onDetail(item.id);
  }, [clearTimer, onDetail, item.id]);

  const handleError = useCallback(() => {
    setSourceIndex((prev) => prev + 1);
  }, []);

  const src = sources[sourceIndex];
  const broken = !src;

  return (
    <button
      type="button"
      className={`wall-card wall-card--${tier}`}
      style={{ width: `${width}px` }}
      data-grid-groupkey={groupKey}
      data-grid-column={span > 1 ? span : undefined}
      onClick={handleClick}
      onDoubleClick={handleDoubleClick}
      aria-label={`${item.n}，${item.w}×${item.h}${item.fav ? '，已收藏' : ''}`}
      title={item.n}
    >
      <span className="wall-card__frame" style={{ aspectRatio: `${item.w} / ${item.h}` }}>
        {broken ? (
          <span className="wall-card__fallback" aria-hidden="true">
            图片不可用
          </span>
        ) : (
          <img
            className="wall-card__img"
            src={src}
            alt={item.n}
            loading="lazy"
            decoding="async"
            draggable={false}
            onError={handleError}
          />
        )}
      </span>
      <span className="wall-card__overlay">
        <span className="wall-card__name">{item.n}</span>
        <span className="wall-card__badges">
          {item.fav ? (
            <span className="badge badge--fav" aria-label="已收藏">
              ♥
            </span>
          ) : null}
          {item.st > 0 ? (
            <span className="badge badge--stars" aria-label={formatStars(item.st)}>
              {formatStars(item.st)}
            </span>
          ) : null}
          {item.rt !== 'G' ? <span className="badge badge--rating">{item.rt}</span> : null}
        </span>
      </span>
    </button>
  );
}
