import { useCallback, useMemo, useState } from 'react';
import { gridSources } from '../lib/assetUrl';
import { formatCount } from '../lib/format';
import { titleSizeForWidth } from '../lib/world';
import type { TagTriple, TierPaths, WallItem } from '../source/types';
import type { TileRect } from '../lib/world';

export interface WallCardProps {
  item: WallItem;
  index: number;
  paths: TierPaths;
  rect: TileRect;
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
  /** 键盘 Enter/Space 打开（和鼠标单击同一条路径：打开灯箱）。 */
  onOpen: (item: WallItem, index: number) => void;
  onSummary: (item: WallItem, clientX: number, clientY: number) => void;
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
 * 单击/双击由画布统一判定（拖拽与点击靠位移阈值区分），这里只处理右键概要与键盘激活。
 * 标题字号按卡宽分级（world.titleSizeForWidth），最多三行收尾（photo-wall-v3.md:72）。
 */
export function WallCard({
  item,
  index,
  paths,
  rect,
  current,
  enter = false,
  enterDelay = 0,
  tagCounts,
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
      onSummary(item, event.clientX, event.clientY);
    },
    [item, onSummary],
  );

  const chip = useMemo(() => chipTextOf(item, tagCounts), [item, tagCounts]);
  const titleSize = titleSizeForWidth(rect.w);

  return (
    <div
      className={'tile' + (enter ? ' tile--enter' : '')}
      data-index={index}
      data-id={item.id}
      data-current={current ? 'true' : undefined}
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
      aria-label={'打开图片：' + item.n}
      title={item.n}
    >
      {failed ? (
        <span className="tile__fallback">缩略图缺失</span>
      ) : (
        <img
          className={'tile__img' + (loaded ? ' is-loaded' : '')}
          src={sources[tierIndex]}
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
    </div>
  );
}
