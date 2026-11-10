import { useCallback, useMemo, useState } from 'react';
import { gridImageAttrs, gridSources } from '../lib/assetUrl';
import type { Rect } from '../lib/expand';
import { formatCount, formatDateTime, formatSize, ratingLabel } from '../lib/format';
import { titleSizeForWidth } from '../lib/world';
import type { TagTriple, TierPaths, WallItem } from '../source/types';
import type { TileRect } from '../lib/world';

export interface WallCardProps {
  item: WallItem;
  index: number;
  paths: TierPaths;
  rect: TileRect;
  /** 画布相机缩放；用于按屏幕实际宽度选缩略档（srcset）。 */
  displayScale?: number;
  /** 这张卡是不是"当前项"（刚点开 / 刚右键看概要的那一张）。 */
  current?: boolean;
  /**
   * 入场动效（§2.5）：只在页面刚进入的窗口内为 true。
   * 虚拟化会在滚动时不断挂载新卡，动画必须靠这个开关关掉，否则滚一下就闪一片。
   */
  enter?: boolean;
  /** 对角线波次的延迟（ms）；首载不走波次时传 0。 */
  enterDelay?: number;
  /** 标签 → 该标签的张数，用来拼角标；拿不到就只显示标签名。 */
  tagCounts?: ReadonlyMap<string, number>;
  /** 右键 / 长按概要：信息直接叠在卡片上（Folia 图4 那种），不再弹浮层小卡。 */
  summary?: boolean;
  onCloseSummary?: () => void;
  /**
   * 双击 / Enter 展开全屏。origin 是卡片矩形，详情页用它做"图片从卡片长到全屏"的
   * 共享元素动画；键盘路径没有矩形，传 undefined 就直接切。
   */
  onOpen: (item: WallItem, index: number, origin?: Rect) => void;
  onSummary: (item: WallItem) => void;
}

/** 角标文案：主标签 · 张数；没有标签就退回 宽×高（B1：以前这里是占位的像素宽度）。 */
function chipTextOf(item: WallItem, tagCounts?: ReadonlyMap<string, number>): string {
  let primary: TagTriple | null = null;
  for (const tag of item.tags) {
    if (!primary || tag[2] > primary[2]) primary = tag;
  }
  if (primary) {
    const count = tagCounts?.get(primary[0]);
    return count ? primary[0] + ' · ' + formatCount(count) : primary[0];
  }
  if (item.w > 0 && item.h > 0) return Math.round(item.w) + '×' + Math.round(item.h);
  return '—';
}

/**
 * 画布上的一张卡片：绝对定位，标题常驻在左下角（不是 hover 才出现）。
 * 单击选中（内描边高亮）、双击展开全屏由画布统一判定；这里处理右键概要叠层与键盘激活。
 * 标题字号按卡宽分级（world.titleSizeForWidth），最多三行收尾（photo-wall-v3.md:72）。
 */
export function WallCard({
  item,
  index,
  paths,
  rect,
  displayScale = 1,
  current,
  enter = false,
  enterDelay = 0,
  tagCounts,
  summary = false,
  onCloseSummary,
  onOpen,
  onSummary,
}: WallCardProps) {
  const sources = gridSources(paths, item);
  const [tierIndex, setTierIndex] = useState(0);
  const [failed, setFailed] = useState(false);
  const [loaded, setLoaded] = useState(false);

  // updater 必须是纯函数（StrictMode 下会跑两次），降级判定放在外面。
  const handleError = useCallback(() => {
    if (tierIndex + 1 < sources.length) setTierIndex(tierIndex + 1);
    else setFailed(true);
  }, [sources.length, tierIndex]);

  const handleContextMenu = useCallback(
    (event: React.MouseEvent<HTMLDivElement>) => {
      event.preventDefault();
      event.stopPropagation();
      onSummary(item);
    },
    [item, onSummary],
  );

  const chip = useMemo(() => chipTextOf(item, tagCounts), [item, tagCounts]);
  // 没有回退到 t512 时才让浏览器按显示宽度选档；一旦降级就固定用 src，避免再次 404。
  const gridAttrs = useMemo(
    () => (tierIndex === 0 ? gridImageAttrs(paths, item, rect.w * displayScale) : null),
    [tierIndex, paths, item, rect.w, displayScale],
  );
  const titleSize = titleSizeForWidth(rect.w);

  return (
    <div
      className={'tile' + (enter ? ' tile--enter' : '')}
      data-index={index}
      data-id={item.id}
      data-current={current ? 'true' : undefined}
      data-summary={summary ? 'true' : undefined}
      style={{
        left: rect.x,
        top: rect.y,
        width: rect.w,
        height: rect.h,
        animationDelay: enter && enterDelay > 0 ? enterDelay + 'ms' : undefined,
      }}
      onContextMenu={handleContextMenu}
      onKeyDown={(event) => {
        if (event.key !== 'Enter' && event.key !== ' ') return;
        event.preventDefault();
        // 不让方向键/空格冒到画布去触发平移或页面滚动。
        event.stopPropagation();
        onOpen(item, index);
      }}
      role="button"
      tabIndex={0}
      aria-label={item.n}
      title={item.n}
    >
      {failed ? (
        <span className="tile__fallback">缩略图缺失</span>
      ) : (
        <img
          className={'tile__img' + (loaded ? ' is-loaded' : '')}
          src={sources[tierIndex]}
          srcSet={gridAttrs?.srcSet}
          sizes={gridAttrs?.sizes}
          alt={item.n}
          loading="lazy"
          decoding="async"
          draggable={false}
          onLoad={() => setLoaded(true)}
          onError={handleError}
        />
      )}

      <span className="tile__shade" />

      <span className="tile__chip">
        {item.fav ? '♥ ' : ''}
        {chip}
      </span>

      <span className="tile__copy">
        <strong style={{ fontSize: titleSize }}>{item.n}</strong>
        <small>
          {item.rt}
          {item.st > 0 ? ' · ' + '★'.repeat(Math.min(5, Math.round(item.st))) : ''}
        </small>
      </span>

      {summary ? (
        <div className="tile__info" onPointerDown={(event) => event.stopPropagation()}>
          <div className="tile__info-bar">
            <button
              type="button"
              className="icon-btn"
              onClick={() => {
                onCloseSummary?.();
                onOpen(item, index);
              }}
              title="打开详情"
              aria-label="打开详情"
            >
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M7 17 17 7" />
                <path d="M8 7h9v9" />
              </svg>
            </button>
            <button
              type="button"
              className="icon-btn"
              onClick={() => onCloseSummary?.()}
              title="关闭（Esc 或点别处）"
              aria-label="关闭概要"
            >
              <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" aria-hidden="true">
                <path d="M6 6l12 12" />
                <path d="M18 6 6 18" />
              </svg>
            </button>
          </div>
          <div className="tile__info-body">
            <strong className="tile__info-name">{item.n}</strong>
            <span className="tile__info-meta">
              {formatSize(item.w, item.h)}
              {item.mtime ? ' · ' + formatDateTime(item.mtime).slice(0, 10) : ''}
              {item.rt ? ' · ' + ratingLabel(item.rt) : ''}
            </span>
            {item.tags.length > 0 ? (
              <span className="tile__info-tags">
                {item.tags.slice(0, 4).map((tag) => tag[0]).join(' · ')}
                {item.tags.length > 4 ? ' …' : ''}
              </span>
            ) : null}
          </div>
        </div>
      ) : null}
    </div>
  );
}
