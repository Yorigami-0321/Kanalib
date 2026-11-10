import { useCallback, useEffect, useRef, useState } from 'react';
import { detailSources } from '../lib/assetUrl';
import { expanding } from '../lib/expand';
import type { TierPaths, WallItem } from '../source/types';
import './detail.css';

interface ZoomState {
  scale: number;
  x: number;
  y: number;
}

const NO_ZOOM: ZoomState = { scale: 1, x: 0, y: 0 };
const MAX_ZOOM = 4;
const WHEEL_NAV_THRESHOLD = 80;
const WHEEL_NAV_COOLDOWN = 450;

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

function wheelUnit(event: WheelEvent, viewportHeight: number): number {
  if (event.deltaMode === WheelEvent.DOM_DELTA_LINE) return 16;
  if (event.deltaMode === WheelEvent.DOM_DELTA_PAGE) return viewportHeight;
  return 1;
}

export interface DetailProps {
  item: WallItem;
  items: readonly WallItem[];
  paths: TierPaths;
  /** 信息检查器是否展开（桌面会让出 300px，窄屏是底部 sheet）。 */
  infoOpen: boolean;
  onBack: () => void;
  onOpenDetail: (id: string) => void;
  onToggleInfo: () => void;
}

interface PaneProps {
  item: WallItem;
  paths: TierPaths;
  /** 只有正中那一屏接手势；左右邻屏只是滑动时的视觉副本。 */
  active: boolean;
  onNavigate: (direction: -1 | 1) => void;
}

/**
 * 一屏 = 一张图。信息不在这里——标题/元数据/标签/相似条在左侧检查器。
 * 图片自己支持：双指/Ctrl+滚轮缩放、放大后拖拽平移、双击复位；未放大时左右拖拽换图。
 */
function Pane({ item, paths, active, onNavigate }: PaneProps) {
  const sources = detailSources(paths, item);
  // 失败档位跟 item.id 绑在一起存，不在 effect 里重置 —— 换 item 的那一帧
  // effect 还没跑，旧 srcIndex 会去索引新的 sources（可能越界成 undefined）。
  const [loadStage, setLoadStage] = useState<{ id: string; index: number }>({ id: item.id, index: 0 });
  const srcIndex = loadStage.id === item.id ? Math.min(loadStage.index, sources.length - 1) : 0;

  const [zoom, setZoom] = useState<ZoomState>(NO_ZOOM);
  const zoomRef = useRef(zoom);
  zoomRef.current = zoom;
  const paneRef = useRef<HTMLDivElement | null>(null);
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  const pinchRef = useRef<{ dist: number; scale: number } | null>(null);
  const dragRef = useRef<{ startX: number; startY: number; panX: number; panY: number } | null>(null);
  const wheelDx = useRef(0);
  const wheelReset = useRef(0);
  const lastNav = useRef(0);

  // 滚轮：Ctrl/⌘ = 缩放；放大后 = 平移图片；1× 时横向 wheel = 上/下一张。
  // 触控板双指横滑在 Chrome/Edge 里就是带 deltaX 的 wheel，不能只处理 ctrlKey。
  useEffect(() => {
    const node = paneRef.current;
    if (!node || !active) return;
    const onWheel = (event: WheelEvent) => {
      if (event.ctrlKey || event.metaKey) {
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
        return;
      }

      const unit = wheelUnit(event, node.clientHeight);
      const dx = event.deltaX * unit;
      const dy = event.deltaY * unit;
      const horizontal = event.shiftKey && Math.abs(dx) < 0.5 ? dy : dx;
      const vertical = event.shiftKey && Math.abs(dx) < 0.5 ? 0 : dy;
      if (Math.abs(horizontal) + Math.abs(vertical) < 0.2) return;
      event.preventDefault();

      if (zoomRef.current.scale > 1) {
        const current = zoomRef.current;
        setZoom(clampZoom(current.scale, current.x + horizontal, current.y + vertical));
        return;
      }

      if (Math.abs(horizontal) < Math.abs(vertical) * 0.8) return;
      wheelDx.current += horizontal;
      window.clearTimeout(wheelReset.current);
      wheelReset.current = window.setTimeout(() => {
        wheelDx.current = 0;
      }, 260);
      const now = performance.now();
      if (Math.abs(wheelDx.current) >= WHEEL_NAV_THRESHOLD && now - lastNav.current > WHEEL_NAV_COOLDOWN) {
        lastNav.current = now;
        const direction = wheelDx.current > 0 ? 1 : -1;
        wheelDx.current = 0;
        onNavigate(direction);
      }
    };
    node.addEventListener('wheel', onWheel, { passive: false });
    return () => {
      node.removeEventListener('wheel', onWheel);
      window.clearTimeout(wheelReset.current);
    };
  }, [active, onNavigate]);

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

  return (
    <div className="view__pane" ref={paneRef}>
      <div
        className={'view__media' + (zoom.scale > 1 ? ' is-zoomed' : '')}
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
    </div>
  );
}

/**
 * 全屏作品页：整页就是那张图（contain、不裁切），信息全部在左侧检查器。
 * 左右切换走水平滑动（niri 式）：当前屏滑出去、下一屏从旁边滑进来。
 */
export function Detail({ item, items, paths, infoOpen, onBack, onOpenDetail, onToggleInfo }: DetailProps) {
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

  // ←/→/Esc/I。滑动进行中不接新指令。
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
      if (event.key === 'i' || event.key === 'I') {
        event.preventDefault();
        onToggleInfo();
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
  }, [items, onBack, onOpenDetail, onToggleInfo]);

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
    <article
      className={
        'view view--detail' + (infoOpen ? ' view--detail-info' : '') + (veiled ? ' view--expanding' : '')
      }
    >
      <div className="view__stage">
        <div className={trackClass} ref={trackRef} onAnimationEnd={commitSlide}>
          {panes.map((pane, paneIndex) =>
            pane ? (
              <Pane
                key={pane.id}
                item={pane}
                paths={paths}
                active={pane.id === item.id && !slide}
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
