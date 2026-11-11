import { useCallback, useEffect, useRef, useState } from 'react';
import { detailSharpSources, originalUrl, tierUrl } from '../lib/assetUrl';
import { expanding, expandRevealDelay } from '../lib/expand';
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

/** 省流/弱网下不做后台 2048 升级：第一屏的 1024 已经够看，别用带宽换边缘细节。 */
function slowNetwork(): boolean {
  const connection = (
    navigator as Navigator & {
      connection?: { saveData?: boolean; effectiveType?: string };
    }
  ).connection;
  if (!connection) return false;
  if (connection.saveData) return true;
  const type = connection.effectiveType;
  return type === 'slow-2g' || type === '2g' || type === '3g';
}

export interface DetailProps {
  item: WallItem;
  items: readonly WallItem[];
  paths: TierPaths;
  /** 信息检查器是否展开（桌面会让出 300px，窄屏是底部 sheet）。 */
  infoOpen: boolean;
  /** 自动放映中：每张停留 4.2s，侧栏 ▶ 切成 ❚❚。 */
  playing: boolean;
  /** 留白视图：图片收进画框，环境层压暗；收起信息时底部出现极简图注。 */
  matte: boolean;
  onBack: () => void;
  onOpenDetail: (id: string) => void;
  onToggleInfo: () => void;
  onTogglePlay: () => void;
  onToggleMatte: () => void;
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
  // 底色永远是 t512：邻屏只拉这一档做占位，居中后才沿锐利档升档。
  // 环境层用的也是同一个 t512 URL，缓存里只有一份。
  const preview = tierUrl(paths, 't512', item.id);
  const sharpLadder = detailSharpSources(paths, item);

  // 重试计数跟 item.id 绑定：换图后旧 nonce 立即失效，
  // 新图的首个请求不会被无意义地加上 ?r=N。
  const [reload, setReload] = useState<{ id: string; nonce: number }>({ id: '', nonce: 0 });
  const reloadNonce = reload.id === item.id ? reload.nonce : 0;
  // 浏览器可能把上一轮的加载失败留在内存缓存里，只换 <img> 节点、不改 URL
  // 会立刻再次失败（实测），重试时给 URL 加一次性参数。
  const retrySuffix = reloadNonce > 0 ? '?r=' + reloadNonce : '';

  // 底色加载态与 id 绑定：缓存图可能在 effect 之前就触发 onLoad，
  // 用 effect 重置会把 is-loaded 永久清掉（v4 实测过一次）。
  const [baseState, setBaseState] = useState<{ id: string; loaded: boolean; failed: boolean }>({
    id: '',
    loaded: false,
    failed: false,
  });
  const base = baseState.id === item.id ? baseState : { id: item.id, loaded: false, failed: false };

  // 锐利档逐级升：index 前进到下一档；全部失败就保持 t512，不再盖错误卡片。
  const [sharpState, setSharpState] = useState<{ id: string; index: number; loaded: boolean; failed: boolean }>({
    id: '',
    index: 0,
    loaded: false,
    failed: false,
  });
  const sharp = sharpState.id === item.id ? sharpState : { id: item.id, index: 0, loaded: false, failed: false };
  const sharpIndex = sharpLadder.length === 0 ? 0 : Math.min(sharp.index, sharpLadder.length - 1);
  // 邻屏不拉锐利档：只有居中这一屏升档。
  const sharpSrc = active && sharpLadder.length > 0 ? sharpLadder[sharpIndex] : '';
  const sharpLoaded = sharp.loaded && sharp.index === sharpIndex;
  // 升档时旧锐利层继续兜底，等新档加载完成再交接，避免中间闪回 t512。
  const [lastSharp, setLastSharp] = useState<{ id: string; src: string }>({ id: '', src: '' });
  const keepSharp =
    active && lastSharp.id === item.id && lastSharp.src !== '' && lastSharp.src !== sharpSrc
      ? lastSharp.src
      : '';

  // 首屏先给 1024，加载完成后再等一拍升 2048：不和首屏、也不和相邻图抢带宽。
  useEffect(() => {
    if (!active || !sharp.loaded || sharpIndex + 1 >= sharpLadder.length) return;
    if (slowNetwork()) return;
    const timer = window.setTimeout(() => {
      setSharpState({ id: item.id, index: sharpIndex + 1, loaded: false, failed: false });
    }, 700);
    return () => window.clearTimeout(timer);
  }, [active, item.id, sharpIndex, sharp.loaded, sharpLadder.length]);

  // 只有底色和锐利档都失败才是真的看不到图；锐利档失败还有 t512 兜底。
  const failed = active && base.failed && (sharpLadder.length === 0 || sharp.failed);
  const original = originalUrl(paths, item);

  const retry = () => {
    setBaseState({ id: item.id, loaded: false, failed: false });
    setSharpState({ id: item.id, index: 0, loaded: false, failed: false });
    setReload((prev) => ({ id: item.id, nonce: (prev.id === item.id ? prev.nonce : 0) + 1 }));
  };

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

  // 底色与锐利层同步缩放：两层用同一个变换，升级时不会有一层留在原位。
  const zoomStyle =
    zoom.scale > 1
      ? {
          transform: `translate3d(${zoom.x}px, ${zoom.y}px, 0) scale(${zoom.scale})`,
          transformOrigin: 'center center',
        }
      : undefined;

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
        <div className="view__layers">
          <img
            key={'base:' + item.id + ':' + reloadNonce}
            className={'view__img' + (base.loaded ? ' is-loaded' : '')}
            src={preview + retrySuffix}
            alt={item.n}
            decoding="async"
            fetchPriority="high"
            onLoad={() => setBaseState({ id: item.id, loaded: true, failed: false })}
            onError={() => setBaseState({ id: item.id, loaded: false, failed: true })}
            style={zoomStyle}
          />
          {keepSharp ? (
            <img
              className="view__img view__img--sharp is-loaded"
              src={keepSharp + retrySuffix}
              alt=""
              aria-hidden="true"
              decoding="async"
              style={zoomStyle}
            />
          ) : null}
          {sharpSrc ? (
            <img
              key={'sharp:' + item.id + ':' + sharpIndex + ':' + reloadNonce}
              className={'view__img view__img--sharp' + (sharpLoaded ? ' is-loaded' : '')}
              src={sharpSrc + retrySuffix}
              alt=""
              aria-hidden="true"
              decoding="async"
              onLoad={() => {
                setSharpState({ id: item.id, index: sharpIndex, loaded: true, failed: false });
                setLastSharp({ id: item.id, src: sharpSrc });
              }}
              onError={() => {
                if (sharpIndex + 1 >= sharpLadder.length) {
                  setSharpState({ id: item.id, index: sharpIndex, loaded: false, failed: true });
                  return;
                }
                setSharpState({ id: item.id, index: sharpIndex + 1, loaded: false, failed: false });
              }}
              style={zoomStyle}
            />
          ) : null}
        </div>
        {failed ? (
          <div
            className="view__error"
            role={active ? 'alert' : undefined}
            // 左右邻屏只是滑动时的视觉副本：别让它们的按钮进入 Tab 序列，
            // 也别让三块错误态同时被读屏播报。
            inert={!active}
            onPointerDown={(event) => event.stopPropagation()}
          >
            <p className="view__error-title">这张图暂时加载不出来</p>
            <p className="view__error-hint">网络或图片档位可能有问题。可以重试。</p>
            <div className="view__error-actions">
              <button type="button" className="view__error-retry" onClick={retry}>
                重试
              </button>
              {original ? (
                <a className="view__error-link" href={original} target="_blank" rel="noreferrer">
                  打开原图 ↗
                </a>
              ) : null}
            </div>
          </div>
        ) : null}
      </div>
    </div>
  );
}

/**
 * 全屏作品页：整页就是那张图（contain、不裁切），信息全部在左侧检查器。
 * 左右切换走水平滑动（niri 式）：当前屏滑出去、下一屏从旁边滑进来。
 */
export function Detail({
  item,
  items,
  paths,
  infoOpen,
  playing,
  matte,
  onBack,
  onOpenDetail,
  onToggleInfo,
  onTogglePlay,
  onToggleMatte,
}: DetailProps) {
  // 屏幕正中正在展示的那张：滑动动画结束后才提交，连按时以上一次的目标为基准。
  const [center, setCenter] = useState<WallItem>(() => items.find((e) => e.id === item.id) ?? item);
  const [slide, setSlide] = useState<{ dir: 1 | -1; from: WallItem; to: WallItem } | null>(null);
  const centerRef = useRef(center);
  centerRef.current = center;
  const slideRef = useRef(slide);
  slideRef.current = slide;
  const trackRef = useRef<HTMLDivElement | null>(null);
  const shellRef = useRef<HTMLElement | null>(null);

  // 打开详情时把焦点从 BODY 接进这张"全屏作品页"：
  // 路由切换会卸载原来的卡片，若不做这一步，键盘与读屏用户会丢掉位置。
  // 只接管仍停在 BODY 的情况，不抢用户已经主动建立的焦点。
  useEffect(() => {
    const node = shellRef.current;
    if (!node) return;
    const active = document.activeElement;
    if (!active || active === document.body) node.focus({ preventScroll: true });
  }, []);

  // 邻图预取已并入邻屏本身：邻屏 Pane 挂载时就只拉 t512 占位，
  // 这里再 new Image() 拉同一张只会制造重复请求。

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
      if (event.key === 'f' || event.key === 'F') {
        event.preventDefault();
        if (document.fullscreenElement) void document.exitFullscreen();
        else void document.documentElement.requestFullscreen().catch(() => undefined);
        return;
      }
      if (event.key === 'p' || event.key === 'P') {
        event.preventDefault();
        onTogglePlay();
        return;
      }
      if (event.key === 'm' || event.key === 'M') {
        event.preventDefault();
        onToggleMatte();
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
  }, [items, onBack, onOpenDetail, onToggleInfo, onToggleMatte, onTogglePlay]);

  // 双击展开的窗口期内先把真实内容蒙住，等克隆图飞到位再露出。
  const [veiled, setVeiled] = useState(() => expanding());
  useEffect(() => {
    if (!veiled) return;
    const timer = window.setTimeout(() => setVeiled(false), 520);
    return () => window.clearTimeout(timer);
  }, [veiled]);

  // 揭示与克隆飞行共用 expand.ts 的时钟：飞行 380ms 起淡入 160ms，
  // 克隆 420ms 落位时真实内容已有可见透明度，交接不留空窗。
  useEffect(() => {
    if (!veiled) return;
    const track = trackRef.current;
    if (!track) return;
    const animation = track.animate([{ opacity: 0 }, { opacity: 1 }], {
      duration: 160,
      delay: expandRevealDelay(),
      easing: 'cubic-bezier(0.2, 0.7, 0.25, 1)',
      fill: 'both',
    });
    return () => animation.cancel();
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

  // 轨道常驻 -100%：pane1 是当前屏。往下一张（dir=1）动画到 -200%，
  // 新图必须放在 pane2；往上一张（dir=-1）动画到 0，新图放在 pane0。
  const panes: (WallItem | undefined)[] = slide
    ? slide.dir === 1
      ? [undefined, slide.from, slide.to]
      : [slide.to, slide.from, undefined]
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
        'view view--detail' +
        (infoOpen ? ' view--detail-info' : '') +
        (veiled ? ' view--expanding' : '') +
        (matte ? ' view--matte' : '')
      }
      ref={shellRef}
      tabIndex={-1}
      role="dialog"
      aria-label={`图片详情：${item.n}`}
    >
      {/* 环境层：同一张图的低分辨率副本，模糊 + 缓漂移，给 contain 留下的黑边一点空气。 */}
      <div
        className="view__ambient"
        style={{ backgroundImage: `url("${tierUrl(paths, 't512', item.id)}")` }}
        aria-hidden="true"
      />
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
      {playing ? <div className="view__playbar" key={item.id} aria-hidden="true" /> : null}
      {matte && !infoOpen ? (
        <div className="view__caption" key={item.id} aria-hidden="true">
          <span className="view__caption-name">{item.n}</span>
          <span>{Math.round(item.w)} × {Math.round(item.h)}</span>
        </div>
      ) : null}
    </article>
  );
}
