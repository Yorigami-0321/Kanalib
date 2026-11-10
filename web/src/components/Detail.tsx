import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { detailSources, originalUrl, tierUrl } from '../lib/assetUrl';
import { expanding } from '../lib/expand';
import { categoryLabel, formatDateTime, formatSize, ratingLabel } from '../lib/format';
import { hammingHex } from '../lib/hash';
import type { TierPaths, WallItem } from '../source/types';
import './detail.css';

const SIMILAR_DISTANCE = 12;
const SIMILAR_LIMIT = 6;

/** 折叠时最多露出的标签数；展开后按类别铺满（容器自己滚）。 */
const COLLAPSED_TAGS = 18;
const COLLAPSED_PER_GROUP = 6;
const TAG_CATEGORY_ORDER = ['general', 'character', 'copyright', 'artist', 'manual', 'meta', 'rating'];

export interface DetailProps {
  item: WallItem;
  items: readonly WallItem[];
  paths: TierPaths;
  onBack: () => void;
  onOpenDetail: (id: string) => void;
  onTagClick: (tag: string) => void;
}

interface TagGroup {
  category: string;
  label: string;
  tags: { name: string; score: number }[];
}

function categoryRank(category: string): number {
  const index = TAG_CATEGORY_ORDER.indexOf(category);
  return index < 0 ? TAG_CATEGORY_ORDER.length : index;
}

/** 按类别分组，组内按置信度降序；类别顺序固定，未知类别殿后。 */
function groupTags(item: WallItem): TagGroup[] {
  const map = new Map<string, { name: string; score: number }[]>();
  for (const [name, category, score] of item.tags) {
    const list = map.get(category);
    if (list) list.push({ name, score });
    else map.set(category, [{ name, score }]);
  }
  return [...map.entries()]
    .sort(([left], [right]) => categoryRank(left) - categoryRank(right))
    .map(([category, tags]) => ({
      category,
      label: categoryLabel(category),
      tags: tags.sort((left, right) => right.score - left.score || left.name.localeCompare(right.name)),
    }));
}

function similarOf(item: WallItem, items: readonly WallItem[]): WallItem[] {
  if (!item.ph) return [];
  const scored: { entry: WallItem; distance: number }[] = [];
  for (const entry of items) {
    if (entry.id === item.id) continue;
    const distance = hammingHex(item.ph, entry.ph);
    if (distance !== null && distance <= SIMILAR_DISTANCE) scored.push({ entry, distance });
  }
  scored.sort((left, right) => left.distance - right.distance);
  return scored.slice(0, SIMILAR_LIMIT).map((entry) => entry.entry);
}

interface ZoomState {
  scale: number;
  x: number;
  y: number;
}

const NO_ZOOM: ZoomState = { scale: 1, x: 0, y: 0 };
const MAX_ZOOM = 4;

function clampZoom(scale: number, x: number, y: number): ZoomState {
  const nextScale = Math.min(MAX_ZOOM, Math.max(1, scale));
  if (nextScale <= 1.001) return NO_ZOOM;
  const maxX = (window.innerWidth * (nextScale - 1)) / 2;
  const maxY = (window.innerHeight * (nextScale - 1)) / 2;
  return {
    scale: nextScale,
    x: Math.min(maxX, Math.max(-maxX, x)),
    y: Math.min(maxY, Math.max(-maxY, y)),
  };
}

interface PaneProps {
  item: WallItem;
  items: readonly WallItem[];
  paths: TierPaths;
  /** 只有正中那一屏接手势；左右邻屏只是滑动时的视觉副本。 */
  active: boolean;
  onOpenDetail: (id: string) => void;
  onTagClick: (tag: string) => void;
  onNavigate: (direction: -1 | 1) => void;
}

/**
 * 一屏 = 一张图 + 它自己的信息层。切换时整个 pane 左右平移（niri 那种平滑滑动）。
 * 图片自己支持：双指/Ctrl+滚轮缩放、放大后拖拽平移、双击复位；未放大时左右拖拽换图。
 */
function Pane({ item, items, paths, active, onOpenDetail, onTagClick, onNavigate }: PaneProps) {
  const sources = detailSources(paths, item);
  // 失败档位跟 item.id 绑在一起存，不在 effect 里重置 —— 换 item 的那一帧
  // effect 还没跑，旧 srcIndex 会去索引新的 sources（可能越界成 undefined）。
  const [loadStage, setLoadStage] = useState<{ id: string; index: number }>({ id: item.id, index: 0 });
  const srcIndex = loadStage.id === item.id ? Math.min(loadStage.index, sources.length - 1) : 0;

  const similar = useMemo(() => similarOf(item, items), [items, item.id, item.ph]);
  const original = originalUrl(paths, item);
  const groups = useMemo(() => groupTags(item), [item]);
  const [expanded, setExpanded] = useState(false);

  const [zoom, setZoom] = useState<ZoomState>(NO_ZOOM);
  const zoomRef = useRef(zoom);
  zoomRef.current = zoom;
  const paneRef = useRef<HTMLDivElement | null>(null);
  const mediaRef = useRef<HTMLDivElement | null>(null);
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const pinchRef = useRef<{ dist: number; scale: number } | null>(null);
  const dragRef = useRef<{ startX: number; startY: number; panX: number; panY: number } | null>(null);

  // Ctrl/⌘+滚轮以光标为锚缩放；普通滚轮留给信息层。
  // 监听挂在整屏 pane 上：鼠标停在底部信息区时也要能缩放图，而不是只有图片露出的那一条能缩。
  useEffect(() => {
    const node = paneRef.current;
    if (!node || !active) return;
    const onWheel = (event: WheelEvent) => {
      if (!(event.ctrlKey || event.metaKey)) return;
      event.preventDefault();
      const box = node.getBoundingClientRect();
      const px = event.clientX - (box.left + box.width / 2);
      const py = event.clientY - (box.top + box.height / 2);
      const current = zoomRef.current;
      const nextScale = Math.min(
        MAX_ZOOM,
        Math.max(1, current.scale * Math.exp(-event.deltaY * 0.002)),
      );
      const ratio = nextScale / current.scale;
      setZoom(clampZoom(nextScale, px - (px - current.x) * ratio, py - (py - current.y) * ratio));
    };
    node.addEventListener('wheel', onWheel, { passive: false });
    return () => node.removeEventListener('wheel', onWheel);
  }, [active]);

  const onPointerDown = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      if (!active) return;
      const node = event.currentTarget;
      node.setPointerCapture?.(event.pointerId);
      pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
      if (pointers.current.size >= 2) {
        const [first, second] = [...pointers.current.values()];
        pinchRef.current = {
          dist: Math.max(1, Math.hypot(first.x - second.x, first.y - second.y)),
          scale: zoomRef.current.scale,
        };
        dragRef.current = null;
        return;
      }
      dragRef.current = {
        startX: event.clientX,
        startY: event.clientY,
        panX: zoomRef.current.x,
        panY: zoomRef.current.y,
      };
    },
    [active],
  );

  const onPointerMove = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      if (!active || !pointers.current.has(event.pointerId)) return;
      pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });

      const pinch = pinchRef.current;
      if (pinch && pointers.current.size >= 2) {
        const [first, second] = [...pointers.current.values()];
        const distance = Math.max(1, Math.hypot(first.x - second.x, first.y - second.y));
        const current = zoomRef.current;
        setZoom(clampZoom(pinch.scale * (distance / pinch.dist), current.x, current.y));
        return;
      }

      const drag = dragRef.current;
      if (!drag) return;
      const dx = event.clientX - drag.startX;
      const dy = event.clientY - drag.startY;
      if (zoomRef.current.scale > 1) {
        setZoom(clampZoom(zoomRef.current.scale, drag.panX + dx, drag.panY + dy));
      }
    },
    [active],
  );

  const onPointerUp = useCallback(
    (event: React.PointerEvent<HTMLDivElement>) => {
      if (!active) return;
      pointers.current.delete(event.pointerId);
      const wasPinch = !!pinchRef.current;
      if (pointers.current.size < 2) pinchRef.current = null;
      const drag = dragRef.current;
      dragRef.current = null;
      if (wasPinch || !drag || zoomRef.current.scale > 1) return;
      const dx = event.clientX - drag.startX;
      const dy = event.clientY - drag.startY;
      if (Math.abs(dx) > 70 && Math.abs(dx) > Math.abs(dy) * 1.2) {
        onNavigate(dx < 0 ? 1 : -1);
      }
    },
    [active, onNavigate],
  );

  const onPointerCancel = useCallback(() => {
    pointers.current.clear();
    pinchRef.current = null;
    dragRef.current = null;
  }, []);

  const onDoubleClick = useCallback(
    (event: React.MouseEvent<HTMLDivElement>) => {
      if (!active) return;
      const current = zoomRef.current;
      if (current.scale > 1) {
        setZoom(NO_ZOOM);
        return;
      }
      const box = event.currentTarget.getBoundingClientRect();
      const px = event.clientX - (box.left + box.width / 2);
      const py = event.clientY - (box.top + box.height / 2);
      setZoom(clampZoom(2, -px, -py));
    },
    [active],
  );

  const totalTags = item.tags.length;
  let remaining = COLLAPSED_TAGS;
  const collapsedGroups = groups
    .map((group) => {
      const take = Math.min(COLLAPSED_PER_GROUP, remaining, group.tags.length);
      remaining -= take;
      return {
        ...group,
        tags: group.tags.slice(0, Math.max(0, take)),
        hidden: Math.max(0, group.tags.length - take),
      };
    })
    .filter((group) => group.tags.length > 0);
  const visibleGroups: (TagGroup & { hidden?: number })[] = expanded ? groups : collapsedGroups;

  return (
    <div className="view__pane" ref={paneRef}>
      <div
        className={'view__media' + (zoom.scale > 1 ? ' is-zoomed' : '')}
        ref={mediaRef}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerCancel}
        onDoubleClick={onDoubleClick}
        role="presentation"
      >
        <img
          className="view__img"
          src={sources[srcIndex]}
          alt={item.n}
          decoding="async"
          style={
            zoom.scale > 1
              ? {
                  transform: `translate3d(${zoom.x}px, ${zoom.y}px, 0) scale(${zoom.scale})`,
                  transformOrigin: 'center center',
                }
              : undefined
          }
          onError={() =>
            setLoadStage((current) => {
              const base = current.id === item.id ? current.index : 0;
              return { id: item.id, index: Math.min(base + 1, sources.length - 1) };
            })
          }
        />
      </div>
      <div className="view__scroll">
        <div className="view__inner">
          <div className="view__info">
            <div className="view__text">
              <h1 className="view__title">{item.n}</h1>
              <p className="view__meta">
                <span>{formatSize(item.w, item.h)}</span>
                {item.mtime ? <span>{formatDateTime(item.mtime)}</span> : null}
                {item.rt ? <span>{ratingLabel(item.rt)}</span> : null}
                {item.st > 0 ? <span>{'★'.repeat(Math.min(5, Math.round(item.st)))}</span> : null}
                {original ? (
                  <a className="view__original" href={original} target="_blank" rel="noreferrer">
                    原图
                  </a>
                ) : null}
              </p>

              {visibleGroups.length > 0 ? (
                <div className={'view__taggroups' + (expanded ? ' is-expanded' : '')}>
                  <div className="view__taggroup-list">
                    {visibleGroups.map((group) => (
                      <div className="view__taggroup" key={group.category}>
                        <span className="view__taggroup-label">{group.label}</span>
                        <div className="view__taggroup-chips">
                          {group.tags.map((tag) => (
                            <button
                              key={tag.name + '|' + group.category}
                              type="button"
                              className="chip"
                              title={categoryLabel(group.category)}
                              onClick={() => onTagClick(tag.name)}
                            >
                              {tag.name}
                            </button>
                          ))}
                          {!expanded && (group.hidden ?? 0) > 0 ? (
                            <span className="chip chip--more" aria-hidden="true">
                              +{group.hidden ?? 0}
                            </span>
                          ) : null}
                        </div>
                      </div>
                    ))}
                  </div>
                  {totalTags > COLLAPSED_TAGS ? (
                    <button type="button" className="view__tags-toggle" onClick={() => setExpanded((v) => !v)}>
                      {expanded ? '收起标签' : `全部 ${totalTags} 个标签`}
                    </button>
                  ) : null}
                </div>
              ) : null}
            </div>

            {similar.length > 0 ? (
              <div className="view__similar">
                <span className="view__similar-label">相似</span>
                <div className="view__similar-strip">
                  {similar.map((entry) => (
                    <button
                      key={entry.id}
                      type="button"
                      className="similar__item"
                      onClick={() => onOpenDetail(entry.id)}
                      title={entry.n}
                      aria-label={entry.n}
                    >
                      <img src={tierUrl(paths, 't256', entry.id)} alt="" loading="lazy" decoding="async" />
                    </button>
                  ))}
                </div>
              </div>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
}

/**
 * 全屏作品页：整页就是那张图（contain、不裁切），信息压在底部一块。
 * 左右切换走水平滑动（niri 式）：当前屏滑出去、下一屏从旁边滑进来。
 */
export function Detail({ item, items, paths, onBack, onOpenDetail, onTagClick }: DetailProps) {
  // 屏幕正中正在展示的那张：滑动动画结束后才提交，连按时以上一次的目标为基准。
  const [center, setCenter] = useState<WallItem>(() => items.find((e) => e.id === item.id) ?? item);
  const [slide, setSlide] = useState<{ dir: 1 | -1; from: WallItem; to: WallItem } | null>(null);
  const centerRef = useRef(center);
  centerRef.current = center;
  const slideRef = useRef(slide);
  slideRef.current = slide;
  const trackRef = useRef<HTMLDivElement | null>(null);

  // 路由换了图：与"正中那张"比索引定方向。不相邻（相似条跳转 / 深链）就直接落位。
  useEffect(() => {
    const base = slideRef.current ? slideRef.current.to : centerRef.current;
    if (item.id === base.id) return;
    const fromIndex = items.findIndex((entry) => entry.id === base.id);
    const toIndex = items.findIndex((entry) => entry.id === item.id);
    if (fromIndex < 0 || toIndex < 0 || Math.abs(toIndex - fromIndex) !== 1) {
      setCenter(item);
      setSlide(null);
      return;
    }
    setSlide({ dir: toIndex > fromIndex ? 1 : -1, from: base, to: item });
  }, [item, items]);

  // 滑动动画走完：提交中心、卸下旧屏。target 必须是 track 本身。
  const commitSlide = useCallback((event: React.AnimationEvent<HTMLDivElement>) => {
    if (event.target !== trackRef.current) return;
    const active = slideRef.current;
    if (!active) return;
    setCenter(active.to);
    setSlide(null);
  }, []);

  // ←/→/Esc。滑动进行中不接新指令（连点会跳帧，直接忽略比排队稳）。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target;
      if (target instanceof Element && target.closest('input, textarea, select')) return;
      if (event.key === 'Escape') {
        event.preventDefault();
        onBack();
        return;
      }
      if (slideRef.current) return;
      const index = items.findIndex((entry) => entry.id === centerRef.current.id);
      if (event.key === 'ArrowLeft' && index > 0) {
        event.preventDefault();
        onOpenDetail(items[index - 1].id);
      } else if (event.key === 'ArrowRight' && index + 1 < items.length) {
        event.preventDefault();
        onOpenDetail(items[index + 1].id);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [items, onBack, onOpenDetail]);

  // 双击展开的窗口期内先把真实内容蒙住，等克隆图飞到位再露出。
  const [veiled, setVeiled] = useState(() => expanding());
  useEffect(() => {
    if (!veiled) return;
    const timer = window.setTimeout(() => setVeiled(false), 520);
    return () => window.clearTimeout(timer);
  }, [veiled]);

  const routeIndex = items.findIndex((entry) => entry.id === item.id);

  const navigateDir = useCallback(
    (direction: -1 | 1) => {
      if (slideRef.current) return;
      const index = items.findIndex((entry) => entry.id === centerRef.current.id);
      const next = items[index + direction];
      if (next) onOpenDetail(next.id);
    },
    [items, onOpenDetail],
  );

  const panes: (WallItem | undefined)[] = slide
    ? slide.dir === 1
      ? [slide.from, slide.to, undefined]
      : [undefined, slide.to, slide.from]
    : [
        routeIndex > 0 ? items[routeIndex - 1] : undefined,
        item,
        routeIndex >= 0 && routeIndex + 1 < items.length ? items[routeIndex + 1] : undefined,
      ];

  const trackClass =
    'view__track' + (slide ? (slide.dir === 1 ? ' view__track--next' : ' view__track--prev') : '');

  return (
    <article className={'view view--detail' + (veiled ? ' view--expanding' : '')}>
      <div className="view__stage">
        <div className={trackClass} ref={trackRef} onAnimationEnd={commitSlide}>
          {panes.map((pane, paneIndex) =>
            pane ? (
              <Pane
                key={pane.id}
                item={pane}
                items={items}
                paths={paths}
                active={pane.id === item.id && !slide}
                onOpenDetail={onOpenDetail}
                onTagClick={onTagClick}
                onNavigate={navigateDir}
              />
            ) : (
              <div className="view__pane" key={'empty-' + paneIndex} aria-hidden="true" />
            ),
          )}
        </div>
      </div>
      <div className="view__scrim" aria-hidden="true" />
    </article>
  );
}
