import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { gridImageAttrs, gridSources, spotlightSources } from '../lib/assetUrl';
import type { Rect } from '../lib/expand';
import {
  displayTag,
  displayTitle,
  formatCount,
  formatDateTime,
  formatSize,
  ratingLabel,
} from '../lib/format';
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
  /** 单击聚焦：卡片在布局里占更大的 rect（周围图被挤开），信息进侧栏。 */
  spotlight?: boolean;
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

/** 角标主标签的分类优先级：内容类优先，尺寸兜底；rating/meta/style 内部键不上卡面。 */
const CHIP_CATEGORY_ORDER = ['general', 'character', 'copyright', 'artist', 'manual'];
/** 概要标签只展示内容类，避免 rating:/style 之类内部键直接上屏。 */
const SUMMARY_EXCLUDED_CATEGORIES = new Set(['rating', 'style', 'meta']);

/** 角标文案：优先级最高的内容标签 · 张数；没有内容标签就退回 宽×高。 */
function chipTextOf(item: WallItem, tagCounts?: ReadonlyMap<string, number>): string {
  let primary: TagTriple | null = null;
  let primaryRank = CHIP_CATEGORY_ORDER.length;
  for (const tag of item.tags) {
    const rank = CHIP_CATEGORY_ORDER.indexOf(tag[1]);
    if (rank < 0) continue;
    if (!primary || rank < primaryRank || (rank === primaryRank && tag[2] > primary[2])) {
      primary = tag;
      primaryRank = rank;
    }
  }
  if (primary) {
    const count = tagCounts?.get(primary[0]);
    const label = displayTag(primary[0]);
    return count ? label + ' · ' + formatCount(count) : label;
  }
  if (item.w > 0 && item.h > 0) return Math.round(item.w) + '×' + Math.round(item.h);
  return '—';
}

/**
 * 画布上的一张卡片：绝对定位，标题常驻在左下角（不是 hover 才出现）。
 * 单击选中（内描边高亮）、双击展开全屏由画布统一判定；这里处理右键概要叠层与键盘激活。
 * 标题字号按卡宽分级（world.titleSizeForWidth），最多三行收尾（photo-wall-v3.md:72）。
 * memo：相机平移只改 .world transform，卡片 props 不变时必须跳过重渲染。
 */
export const WallCard = memo(function WallCard({
  item,
  index,
  paths,
  rect,
  displayScale = 1,
  spotlight = false,
  current,
  enter = false,
  enterDelay = 0,
  tagCounts,
  summary = false,
  onCloseSummary,
  onOpen,
  onSummary,
}: WallCardProps) {
  // 网格用 t256/t512；聚焦时按 sharp 声明换锐利档（t2048/t1024 → t512）。
  const sources = useMemo(
    () => (spotlight ? spotlightSources(paths, item) : gridSources(paths, item)),
    [spotlight, paths, item],
  );
  const [tierIndex, setTierIndex] = useState(0);
  const [failed, setFailed] = useState(false);
  const [loaded, setLoaded] = useState(false);
  const imgRef = useRef<HTMLImageElement | null>(null);
  /** 聚焦换图窗口：锐利档解码完成前继续显示旧画面（holdSrc），避免基线 JPEG 半解码白带。 */
  const [spotReady, setSpotReady] = useState(false);
  const [holdSrc, setHoldSrc] = useState<string | null>(null);

  // 聚焦切换时换一套 sources，失败档归零；加载态不重置——已经显示的图继续可见。
  // 锐利档先离屏 decode()，整帧解码完才替换 src：data/t1024 是基线 JPEG，
  // 直接换 src 会把未解码的行渲染成白色（聚焦下半白带，反馈文档 §5）。
  useEffect(() => {
    setTierIndex(0);
    setFailed(false);
    if (!spotlight) {
      setSpotReady(false);
      setHoldSrc(null);
      return;
    }
    setHoldSrc(imgRef.current?.currentSrc || null);
    setSpotReady(false);
    let alive = true;
    const tryTier = (index: number) => {
      if (index >= sources.length) {
        if (alive) setFailed(true);
        return;
      }
      const probe = new Image();
      probe.setAttribute('fetchpriority', 'high');
      probe.src = sources[index];
      probe.decode().then(
        () => {
          if (!alive) return;
          setTierIndex(index);
          setSpotReady(true);
        },
        () => {
          if (alive) tryTier(index + 1);
        },
      );
    };
    tryTier(0);
    return () => {
      alive = false;
    };
  }, [spotlight, item.id, sources]);

  // updater 必须是纯函数（StrictMode 下会跑两次），降级判定放在外面。
  const handleError = useCallback(() => {
    if (tierIndex + 1 < sources.length) setTierIndex(tierIndex + 1);
    else setFailed(true);
  }, [sources.length, tierIndex]);

  const handleContextMenu = useCallback(
    (event: React.MouseEvent<HTMLDivElement>) => {
      event.preventDefault();
      event.stopPropagation();
      // 概要已开时再次右键 = 退出（反馈问题 4b）；App 的 pointerdown 关法
      // 对卡片上的右键让行，由这里统一判定开/关。
      if (summary) onCloseSummary?.();
      else onSummary(item);
    },
    [item, summary, onSummary, onCloseSummary],
  );

  // 概要退出：summary 变 false 后停留 160ms 播淡出再卸载（动画衔接，问题 6）。
  // wasSummary 防止没开过概要的卡在挂载时空跑一轮停留。
  const wasSummary = useRef(summary);
  const [infoLingering, setInfoLingering] = useState(false);
  useEffect(() => {
    if (summary) {
      wasSummary.current = true;
      setInfoLingering(false);
      return;
    }
    if (!wasSummary.current) return;
    wasSummary.current = false;
    setInfoLingering(true);
    const timer = window.setTimeout(() => setInfoLingering(false), 160);
    return () => window.clearTimeout(timer);
  }, [summary]);
  const infoMounted = summary || infoLingering;

  const chip = useMemo(() => chipTextOf(item, tagCounts), [item, tagCounts]);
  const summaryTags = useMemo(
    () =>
      item.tags
        .filter((tag) => !SUMMARY_EXCLUDED_CATEGORIES.has(tag[1]))
        .map((tag) => displayTag(tag[0])),
    [item],
  );
  // 没有回退到 t512 时才让浏览器按显示宽度选档；一旦降级就固定用 src，避免再次 404。
  const gridAttrs = useMemo(
    () => (!spotlight && tierIndex === 0 ? gridImageAttrs(paths, item, rect.w * displayScale) : null),
    [spotlight, tierIndex, paths, item, rect.w, displayScale],
  );
  const titleSize = titleSizeForWidth(rect.w);

  return (
    <div
      className={
        'tile' + (enter ? ' tile--enter' : '') + (spotlight ? ' tile--spotlight' : '')
      }
      data-index={index}
      data-id={item.id}
      data-current={current || spotlight ? 'true' : undefined}
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
          ref={imgRef}
          className={'tile__img' + (loaded ? ' is-loaded' : '')}
          src={spotlight && !spotReady && holdSrc ? holdSrc : sources[tierIndex]}
          srcSet={gridAttrs?.srcSet}
          sizes={gridAttrs?.sizes}
          alt={item.n}
          // 画布本身已经把 DOM 限制在视口附近，再叠加浏览器懒加载会因为 .world 的
          // translate 只触发左上角一小撮，其余卡一直是空的。这里全量即时加载。
          loading="eager"
          fetchPriority={spotlight ? 'high' : 'auto'}
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
        <strong style={{ fontSize: titleSize }}>{displayTitle(item.n)}</strong>
        <small>
          {ratingLabel(item.rt)}
          {item.st > 0 ? ' · ' + '★'.repeat(Math.min(5, Math.round(item.st))) : ''}
        </small>
      </span>

      {infoMounted ? (
        <div
          className={'tile__info' + (summary ? '' : ' tile__info--leave')}
          onPointerDown={(event) => event.stopPropagation()}
        >
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
            <strong className="tile__info-name">{displayTitle(item.n)}</strong>
            <span className="tile__info-meta">
              {formatSize(item.w, item.h)}
              {item.mtime ? ' · ' + formatDateTime(item.mtime).slice(0, 10) : ''}
              {' · ' + ratingLabel(item.rt)}
            </span>
            {summaryTags.length > 0 ? (
              <span className="tile__info-tags">
                {summaryTags.slice(0, 4).join(' · ')}
                {summaryTags.length > 4 ? ' …' : ''}
              </span>
            ) : null}
          </div>
        </div>
      ) : null}
    </div>
  );
});
