import { useCallback, useEffect, useRef, useState } from 'react';
import { gridSources } from '../lib/assetUrl';
import type { TierPaths, WallItem } from '../source/types';

/** 单击延时：等过双击判定窗口再开 lightbox，避免双击时闪一下大图。 */
export const CLICK_DELAY_MS = 220;

/** 贴片跨度（列/行）：由宽高比推出，行高等于列宽，所以 c/r 就是贴片宽高比。 */
export interface TileSpan {
  c: number;
  r: number;
}

const MAX_ROWS = 8;
const MIN_ROWS = 2;

/**
 * 契约 §4 的宽高比 → 栅格跨度：横图占 4 列，其余占 3 列；
 * 行数按比例反推并夹在 2..8，避免极端比例把墙拉花。
 */
export function spanOf(item: Pick<WallItem, 'w' | 'h'>): TileSpan {
  const ratio = item.w > 0 && item.h > 0 ? item.w / item.h : 1;
  const c = ratio >= 1.6 ? 4 : 3;
  const r = Math.min(MAX_ROWS, Math.max(MIN_ROWS, Math.round(c / ratio)));
  return { c, r };
}

function starsOf(stars: number): string {
  const count = Math.min(5, Math.max(0, Math.round(stars)));
  return count > 0 ? '★'.repeat(count) : '';
}

export interface WallCardProps {
  item: WallItem;
  index: number;
  paths: TierPaths;
  span: TileSpan;
  onOpen: (item: WallItem, index: number) => void;
  onDetail: (id: string) => void;
}

/** 墙上的单片贴片：只负责渲染，位置（grid-column/row）由 Wall 决定。 */
export function WallCard({ item, index, paths, span, onOpen, onDetail }: WallCardProps) {
  const sources = gridSources(paths, item);
  const [tierIndex, setTierIndex] = useState(0);
  const [failed, setFailed] = useState(false);
  const clickTimer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (clickTimer.current !== null) window.clearTimeout(clickTimer.current);
    },
    [],
  );

  const handleClick = useCallback(() => {
    if (clickTimer.current !== null) window.clearTimeout(clickTimer.current);
    clickTimer.current = window.setTimeout(() => {
      clickTimer.current = null;
      onOpen(item, index);
    }, CLICK_DELAY_MS);
  }, [item, index, onOpen]);

  const handleDoubleClick = useCallback(() => {
    if (clickTimer.current !== null) {
      window.clearTimeout(clickTimer.current);
      clickTimer.current = null;
    }
    onDetail(item.id);
  }, [item.id, onDetail]);

  const handleError = useCallback(() => {
    setTierIndex((current) => {
      if (current + 1 < sources.length) return current + 1;
      setFailed(true);
      return current;
    });
  }, [sources.length]);

  const stars = starsOf(item.st);

  return (
    <button
      type="button"
      className="tile"
      style={{ gridColumn: 'span ' + span.c, gridRow: 'span ' + span.r }}
      title={item.n + '\n点击看大图 · 双击看详情'}
      onClick={handleClick}
      onDoubleClick={handleDoubleClick}
    >
      {failed ? (
        <span className="tile__fallback">缩略图缺失</span>
      ) : (
        <img
          className="tile__img"
          src={sources[tierIndex]}
          alt={item.n}
          loading="lazy"
          decoding="async"
          draggable={false}
          onError={handleError}
        />
      )}
      <span className="tile__shade" />
      <span className="tile__meta">
        <span className="tile__name">{item.n}</span>
        {item.fav || stars || item.rt ? (
          <span className="tile__badges">
            {item.fav ? <span className="tile__badge--fav">♥</span> : null}
            {stars ? <span>{stars}</span> : null}
            {item.rt ? <span>{item.rt}</span> : null}
          </span>
        ) : null}
      </span>
    </button>
  );
}
