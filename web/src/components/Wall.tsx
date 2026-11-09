import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { TierPaths, WallItem } from '../source/types';
import type { WallPhase } from '../state/useWall';
import { cellTilesInView, layoutWorld, visibleSlice, type CellSpec } from '../lib/world';
import { WallCard } from './WallCard';

/** 世界比视口宽多少：留出横向可平移的余量。 */
const WORLD_OVERSCAN = 1.28;
const WORLD_MIN_WIDTH = 1180;
const GAP = 8;
const TARGET_ROW_HEIGHT = 268;
const ZOOM_MIN = 0.5;
const ZOOM_MAX = 1.6;
const LOAD_AHEAD = 900;
/** 左键按下后位移超过这个距离（CSS px）就不再算单击。 */
const CLICK_SLOP = 6;
/** 相机屏幕坐标超过这个量级就 rebase（§1.1 坐标防漂移）。 */
const REBASE_LIMIT = 1e7;
/** 这些元素自己处理手势，画布不接管。 */
const INTERACTIVE = 'button, a, input, select, textarea, [role="button"]';

export interface WallViewProps {
  items: WallItem[];
  paths: TierPaths;
  phase: WallPhase;
  hasMore: boolean;
  resetKey: string;
  filtered: boolean;
  /** 条目只会向后追加、顺序不会被重排时为 true：布局可以暂扣不满的行，已渲染的卡永不移动。 */
  stableAppend?: boolean;
  /**
   * 无限空间（§1.1 / 规则 11）：上/下/左/右都能一直走，没有边界。
   * 只有"全库随机墙"才开；标签墙/筛选后的列表条目少，仍然用有限世界（否则三张图会无限重复）。
   */
  infinite?: boolean;
  /** 无限空间要求条目全量到齐后再生成，否则分片到达会让整屏重排。 */
  ready?: boolean;
  /** 世界种子：决定每个格子拿哪批图（刷新即换）。 */
  worldSeed?: number;
  /** 标签 → 张数，透传给卡片角标。 */
  tagCounts?: ReadonlyMap<string, number>;
  onLoadMore: () => void;
  onOpen: (item: WallItem, index: number) => void;
  onSummary: (item: WallItem, clientX: number, clientY: number) => void;
  onFocusItems?: (items: WallItem[]) => void;
  onRetry: () => void;
}

interface Camera {
  x: number;
  y: number;
  zoom: number;
}

interface PointerState {
  active: boolean;
  id: number;
  startX: number;
  startY: number;
  camX: number;
  camY: number;
  moved: boolean;
  lastX: number;
  lastY: number;
  vx: number;
  vy: number;
  lastT: number;
  /** 按下时命中的卡片：setPointerCapture 之后 pointerup 的 target 会被重定向到画布，必须提前记住。 */
  downTile: HTMLElement | null;
}

const IDLE_POINTER: PointerState = {
  active: false,
  id: -1,
  startX: 0,
  startY: 0,
  camX: 0,
  camY: 0,
  moved: false,
  lastX: 0,
  lastY: 0,
  vx: 0,
  vy: 0,
  lastT: 0,
  downTile: null,
};

/**
 * 空间层：一面比视口大的墙，左键拖拽平移、滚轮纵向滚动、Ctrl+滚轮缩放。
 * 只渲染视野内的卡片（按 y 二分），所以一万条也能拖得动。
 */
export function WallView({
  items,
  paths,
  phase,
  hasMore,
  resetKey,
  filtered,
  stableAppend = false,
  infinite = false,
  ready = true,
  worldSeed = 1,
  tagCounts,
  onLoadMore,
  onOpen,
  onSummary,
  onFocusItems,
  onRetry,
}: WallViewProps) {
  const viewportRef = useRef<HTMLDivElement | null>(null);
  const [size, setSize] = useState({ w: 1440, h: 900 });
  const [camera, setCamera] = useState<Camera>({ x: 0, y: 0, zoom: 1 });
  const [dragging, setDragging] = useState(false);
  const [activeId, setActiveId] = useState<string | null>(null);
  const cameraRef = useRef<Camera>(camera);
  cameraRef.current = camera;
  const sizeRef = useRef(size);
  sizeRef.current = size;

  // 窄屏（<=760）：导航缝收成 64px，世界也从缝的右缘开始 —— 否则第一列会被缝压掉一半，
  // 390 屏上看着像坏掉。桌面保持"世界铺满、图从缝底下穿过"的空间感。
  const narrow = size.w <= 760;
  const worldInset = narrow ? 64 : 0;
  const viewWidth = Math.max(200, size.w - worldInset);
  const worldWidth = narrow
    ? viewWidth
    : Math.max(WORLD_MIN_WIDTH, Math.round(size.w * WORLD_OVERSCAN));
  // 窄屏把世界行高收到可见宽度的 0.55 倍：一张卡大致正好放进一列，不再横向切卡。
  const rowTargetHeight = narrow
    ? Math.round(Math.min(TARGET_ROW_HEIGHT, viewWidth * 0.55))
    : TARGET_ROW_HEIGHT;
  // 顺序稳定 + 还有下一片：末尾不满的行先不进布局，这样追加分片时
  // 已经渲染出来的卡片一个像素都不会动（P0：滚到底加载后"已看内容不抖动"）。
  // 筛选/排序/标签墙的列表会因为新数据整体重排，那些场景必须把不满的行画出来，
  // 否则可能整页空白；数据全部到齐后再拉伸最后一行，视觉上收口。
  const includePartialRow = !(stableAppend && hasMore);
  const stretchPartialRow = !hasMore;
  const layout = useMemo(
    () =>
      // 无限空间不排整墙：一万条跑一遍 justified rows 纯属浪费，格子按需生成就够了。
      infinite
        ? { tiles: [], height: 0 }
        : layoutWorld(items, {
            width: worldWidth,
            gap: GAP,
            targetHeight: rowTargetHeight,
            includePartialRow,
            stretchPartialRow,
          }),
    [infinite, items, worldWidth, rowTargetHeight, includePartialRow, stretchPartialRow],
  );
  const layoutRef = useRef(layout);
  layoutRef.current = layout;

  // 无限空间（§1.1 / 规则 11）：世界没有尺寸，空间切成格子按需生成。
  // 格子边长跟着视口走（一屏 4–6 格）；窄屏必须收窄到可见宽度，
  // 否则 1080 的桌面地板会让 390 屏上每张卡都大得只剩半张。
  const cellWidth = narrow
    ? Math.max(260, viewWidth)
    : Math.max(1080, Math.round(viewWidth * 1.15));
  // 窄屏格子按"两列三行"反推：行高 ≈ 可见宽/2/1.33，三层加起来约等于可见宽 × 1.15。
  const cellHeight = narrow
    ? Math.max(320, Math.round(viewWidth * 1.15))
    : Math.max(720, Math.round(size.h * 1.15));
  const cellSpec = useMemo<CellSpec>(
    () => ({ items, seed: worldSeed, cellWidth, cellHeight, gap: GAP }),
    [items, worldSeed, cellWidth, cellHeight],
  );
  // DOM 坐标不随相机无限增长：origin 是一段世界偏移，rebase 时整体平移。
  const [origin, setOrigin] = useState({ x: 0, y: 0 });
  // 相机换算成"世界坐标里的可见矩形"（未乘缩放）。
  const viewRect = {
    x: camera.x / camera.zoom,
    y: camera.y / camera.zoom,
    w: viewWidth / camera.zoom,
    h: size.h / camera.zoom,
  };
  const cellTiles = useMemo(
    () =>
      infinite && ready
        ? // 缓冲区按格子尺度取（约 1/3 格）：固定 420px 在窄屏上等于多预取两屏，
          // DOM 卡片数会翻倍，违反"数量恒定"。
          cellTilesInView(cellSpec, viewRect, Math.round(Math.min(cellWidth, cellHeight) * 0.35))
        : [],
    // viewRect 是每帧新建的对象，依赖拆成 4 个数字，避免无谓重算。
    [infinite, ready, cellSpec, cellHeight, cellWidth, viewRect.x, viewRect.y, viewRect.w, viewRect.h],
  );

  const clampCamera = useCallback(
    (cam: Camera): Camera => {
      // 无限空间不夹取：任一方向都能一直走（规则 11）。
      if (infinite) return cam;
      const maxX = Math.max(0, worldWidth * cam.zoom - viewWidth);
      const maxY = Math.max(0, layoutRef.current.height * cam.zoom - size.h);
      return {
        zoom: cam.zoom,
        x: Math.min(maxX, Math.max(0, cam.x)),
        y: Math.min(maxY, Math.max(0, cam.y)),
      };
    },
    [infinite, size.h, viewWidth, worldWidth],
  );

  const moveCamera = useCallback(
    (next: Camera) => {
      const clamped = clampCamera(next);
      cameraRef.current = clamped;
      setCamera(clamped);
    },
    [clampCamera],
  );
  const moveCameraRef = useRef(moveCamera);
  moveCameraRef.current = moveCamera;

  /** 让世界坐标 (wx, wy) 落在视口中心。 */
  const centerOnWorld = useCallback(
    (wx: number, wy: number, zoom = cameraRef.current.zoom) => {
      moveCameraRef.current({ zoom, x: wx * zoom - viewWidth / 2, y: wy * zoom - size.h / 2 });
    },
    [size.h, viewWidth],
  );

  useEffect(() => {
    const host = viewportRef.current;
    if (!host) return;
    const apply = (width: number, height: number) => {
      setSize((current) =>
        current.w === width && current.h === height ? current : { w: width, h: height },
      );
    };
    apply(host.clientWidth, host.clientHeight);
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver((entries) => {
      const entry = entries[0];
      if (entry) apply(Math.round(entry.contentRect.width), Math.round(entry.contentRect.height));
    });
    observer.observe(host);
    return () => observer.disconnect();
  }, []);

  // 换筛选/换页：屏幕中央回到世界起点（格子 0,0 的中心）。
  useEffect(() => {
    setActiveId(null);
    if (infinite) {
      setOrigin({ x: 0, y: 0 });
      centerOnWorld(cellWidth / 2, cellHeight / 2, cameraRef.current.zoom);
      return;
    }
    const cam = cameraRef.current;
    moveCamera({ x: Math.max(0, (worldWidth * cam.zoom - viewWidth) / 2), y: 0, zoom: cam.zoom });
  }, [resetKey]);

  // 视口变窄/旋转/世界重排后，相机可能留在越界位置（典型症状：390 宽时右侧空一大条）。
  // 这里把相机夹回合法区间——只在真的越界时才写状态，避免自激重渲染。
  useEffect(() => {
    if (infinite) return;
    const cam = cameraRef.current;
    const clamped = clampCamera(cam);
    if (clamped.x === cam.x && clamped.y === cam.y && clamped.zoom === cam.zoom) return;
    cameraRef.current = clamped;
    setCamera(clamped);
  }, [clampCamera, infinite, layout.height]);

  // 坐标防漂移（§1.1）：屏幕坐标量级超过 1e7 就把世界整体平移整数个格子。
  // origin 与 camera 同步加同一个量，画面一个像素都不动，DOM 里的数却回到小区间。
  useEffect(() => {
    if (!infinite) return;
    if (Math.abs(camera.x) < REBASE_LIMIT && Math.abs(camera.y) < REBASE_LIMIT) return;
    const zoom = camera.zoom || 1;
    const kx = Math.round(camera.x / (cellWidth * zoom));
    const ky = Math.round(camera.y / (cellHeight * zoom));
    if (kx === 0 && ky === 0) return;
    setOrigin((current) => ({ x: current.x + kx * cellWidth, y: current.y + ky * cellHeight }));
    moveCamera({
      ...camera,
      x: camera.x + kx * cellWidth * zoom,
      y: camera.y + ky * cellHeight * zoom,
    });
  }, [camera, cellHeight, cellWidth, infinite, moveCamera]);

  // 到墙尾前预取下一片（只在有限世界里有"墙尾"）。
  useEffect(() => {
    if (infinite || !hasMore) return;
    const bottom = (camera.y + size.h) / camera.zoom;
    if (bottom > layout.height - LOAD_AHEAD) onLoadMore();
  }, [camera, hasMore, infinite, layout.height, onLoadMore, size.h]);

  const pointer = useRef<PointerState>({ ...IDLE_POINTER });
  const inertia = useRef(0);
  /** 当前按在画布上的所有指针（触屏双指缩放要同时跟踪两根）。 */
  const pointers = useRef(new Map<number, { x: number; y: number }>());
  /** 双指缩放基准：起始指距、起始缩放，以及基准时中点的世界坐标。 */
  const pinch = useRef<{ dist: number; zoom: number; worldX: number; worldY: number } | null>(null);

  // ===== 入场动效（§2.5）=====
  // 首载只做淡入 + 上浮 12px；换页签/换筛选走对角线波次。
  // 关键约束：虚拟化会在滚动中不断挂载新卡，动画必须限制在"进入窗口"内，
  // 否则滚一下就闪一片——所以用 enterOn 开关 + 定时收口，而不是纯 CSS 的 mount 动画。
  const firstEnter = useRef(true);
  const [enter, setEnter] = useState({ on: true, wave: false, token: 0 });
  useEffect(() => {
    const wave = !firstEnter.current;
    firstEnter.current = false;
    setEnter((current) => ({ on: true, wave, token: current.token + 1 }));
    const timer = window.setTimeout(
      () => setEnter((current) => ({ ...current, on: false })),
      wave ? 900 : 520,
    );
    return () => window.clearTimeout(timer);
  }, [resetKey]);

  const stopInertia = useCallback(() => {
    if (inertia.current) cancelAnimationFrame(inertia.current);
    inertia.current = 0;
  }, []);

  // P1-1：切 #/tags、开详情都会卸载 WallView，惯性 rAF 必须在这里停掉，
  // 否则它会在已卸载的组件上继续 moveCamera → setState（真泄漏）。
  useEffect(() => stopInertia, [stopInertia]);

  /** 手势收尾：无论正常抬起、被取消还是丢了指针捕获，都走这里。 */
  const endPointer = (host: HTMLElement | null, pointerId: number) => {
    pointer.current = { ...pointer.current, active: false };
    setDragging(false);
    if (host && pointerId >= 0 && host.hasPointerCapture?.(pointerId)) {
      host.releasePointerCapture(pointerId);
    }
  };

  /** 双指结束：清基准，剩下那根手指重新起一个拖拽基线（否则抬手后画面推不动）。 */
  const endPinch = (host: HTMLElement | null, pointerId: number) => {
    pinch.current = null;
    setDragging(false);
    if (host && pointerId >= 0 && host.hasPointerCapture?.(pointerId)) {
      host.releasePointerCapture(pointerId);
    }
    const rest = pointers.current.entries().next();
    if (rest.done) {
      pointer.current = { ...IDLE_POINTER };
      return;
    }
    const [id, position] = rest.value;
    const cam = cameraRef.current;
    pointer.current = {
      ...IDLE_POINTER,
      active: true,
      id,
      startX: position.x,
      startY: position.y,
      camX: cam.x,
      camY: cam.y,
      lastX: position.x,
      lastY: position.y,
      lastT: performance.now(),
    };
  };

  const onPointerDown = (event: React.PointerEvent<HTMLDivElement>) => {
    // 鼠标只认左键；触摸/手写笔的每个 pointerId 都要收（第二根手指是缩放）。
    if (event.pointerType === 'mouse' && event.button !== 0) return;
    const element = event.target as Element | null;
    // .tile 现在也是 role="button"（键盘可达），但它照样要能被拖：
    // 只跳过真正的交互控件，卡片本身继续走画布手势。
    const interactive = element?.closest(INTERACTIVE);
    if (interactive && !interactive.classList.contains('tile')) return;
    stopInertia();
    const host = event.currentTarget as HTMLElement;
    // 捕获在画布上：指针移出画布后 pointermove 仍然到画布，拖拽不会断。
    host.setPointerCapture?.(event.pointerId);
    pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });

    if (pointers.current.size >= 2) {
      // 第二根手指落下：单指平移立刻让位给缩放，且不再算"点击"。
      pointer.current = { ...pointer.current, active: false, moved: true };
      setDragging(true);
      const [first, second] = [...pointers.current.values()];
      const box = host.getBoundingClientRect();
      const midX = (first.x + second.x) / 2 - box.left;
      const midY = (first.y + second.y) / 2 - box.top;
      const cam = cameraRef.current;
      pinch.current = {
        dist: Math.max(1, Math.hypot(first.x - second.x, first.y - second.y)),
        zoom: cam.zoom,
        worldX: (cam.x + midX) / cam.zoom,
        worldY: (cam.y + midY) / cam.zoom,
      };
      return;
    }

    const cam = cameraRef.current;
    pointer.current = {
      active: true,
      id: event.pointerId,
      startX: event.clientX,
      startY: event.clientY,
      camX: cam.x,
      camY: cam.y,
      moved: false,
      lastX: event.clientX,
      lastY: event.clientY,
      vx: 0,
      vy: 0,
      lastT: performance.now(),
      downTile: element?.closest<HTMLElement>('.tile') ?? null,
    };
  };

  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    if (pointers.current.has(event.pointerId)) {
      pointers.current.set(event.pointerId, { x: event.clientX, y: event.clientY });
    }

    // 双指缩放：指距比例映射到缩放倍数，锚点固定在双指中点。
    const pinchState = pinch.current;
    if (pinchState && pointers.current.size >= 2) {
      const [first, second] = [...pointers.current.values()];
      const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
      const midX = (first.x + second.x) / 2 - box.left;
      const midY = (first.y + second.y) / 2 - box.top;
      const distance = Math.max(1, Math.hypot(first.x - second.x, first.y - second.y));
      const zoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, pinchState.zoom * (distance / pinchState.dist)));
      moveCameraRef.current({
        zoom,
        x: pinchState.worldX * zoom - midX,
        y: pinchState.worldY * zoom - midY,
      });
      return;
    }

    const state = pointer.current;
    if (!state.active || event.pointerId !== state.id) return;
    // 鼠标键已经松开但没收到 pointerup（比如指针在窗口外松开）：按抬起处理。
    if (event.pointerType === 'mouse' && event.buttons === 0) {
      endPointer(event.currentTarget as HTMLElement, event.pointerId);
      return;
    }
    const dx = event.clientX - state.startX;
    const dy = event.clientY - state.startY;
    if (!state.moved && Math.hypot(dx, dy) > CLICK_SLOP) {
      state.moved = true;
      setDragging(true);
    }
    if (!state.moved) return;
    const now = performance.now();
    const dt = Math.max(1, now - state.lastT);
    state.vx = (event.clientX - state.lastX) / dt;
    state.vy = (event.clientY - state.lastY) / dt;
    state.lastX = event.clientX;
    state.lastY = event.clientY;
    state.lastT = now;
    moveCamera({ ...cameraRef.current, x: state.camX - dx, y: state.camY - dy });
  };

  const settleInertia = () => {
    const state = pointer.current;
    let vx = state.vx * 16;
    let vy = state.vy * 16;
    if (Math.hypot(vx, vy) < 1.2) return;
    const step = () => {
      vx *= 0.92;
      vy *= 0.92;
      const cam = cameraRef.current;
      moveCameraRef.current({ ...cam, x: cam.x - vx, y: cam.y - vy });
      if (Math.hypot(vx, vy) > 0.4) inertia.current = requestAnimationFrame(step);
      else inertia.current = 0;
    };
    inertia.current = requestAnimationFrame(step);
  };

  const onPointerUp = (event: React.PointerEvent<HTMLDivElement>) => {
    const host = event.currentTarget as HTMLElement;
    pointers.current.delete(event.pointerId);

    // 缩放中松手：退出缩放；还剩一根手指就交回给平移，而不是让画面"冻住"。
    if (pinch.current) {
      if (pointers.current.size < 2) endPinch(host, event.pointerId);
      return;
    }

    const state = pointer.current;
    if (!state.active || event.pointerId !== state.id) return;
    const moved = state.moved;
    // 命中的卡片在 pointerdown 时就记住了：捕获之后 pointerup 的 target 是画布本身。
    const tile = state.downTile;
    endPointer(host, event.pointerId);
    if (moved) {
      settleInertia();
      return;
    }
    const rawIndex = tile?.dataset.index;
    if (rawIndex === undefined) return;
    const index = Number(rawIndex);
    const item = items[index];
    if (!item) return;
    setActiveId(item.id);
    onOpen(item, index);
  };

  const onPointerCancel = (event: React.PointerEvent<HTMLDivElement>) => {
    const host = event.currentTarget as HTMLElement;
    pointers.current.delete(event.pointerId);
    if (pinch.current) {
      if (pointers.current.size < 2) endPinch(host, event.pointerId);
      return;
    }
    if (event.pointerId !== pointer.current.id) return;
    endPointer(host, event.pointerId);
  };

  // 滚轮必须用非被动监听：React 的 onWheel 挂在 root 上是 passive 的，
  // preventDefault() 会被忽略并报 "Unable to preventDefault inside passive event listener"。
  useEffect(() => {
    const host = viewportRef.current;
    if (!host) return;
    const onWheelNative = (event: WheelEvent) => {
      event.preventDefault();
      stopInertia();
      const cam = cameraRef.current;
      if (event.ctrlKey || event.metaKey) {
        const rect = host.getBoundingClientRect();
        const mx = event.clientX - rect.left;
        const my = event.clientY - rect.top;
        const worldX = (cam.x + mx) / cam.zoom;
        const worldY = (cam.y + my) / cam.zoom;
        const nextZoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, cam.zoom * (1 - event.deltaY * 0.0022)));
        moveCameraRef.current({ zoom: nextZoom, x: worldX * nextZoom - mx, y: worldY * nextZoom - my });
        return;
      }
      // deltaMode：0=像素、1=行、2=页。触控板给的是像素，鼠标滚轮常给行。
      const scale = event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? sizeRef.current.h : 1;
      moveCameraRef.current({ ...cam, y: cam.y + event.deltaY * scale });
    };
    host.addEventListener('wheel', onWheelNative, { passive: false });
    return () => host.removeEventListener('wheel', onWheelNative);
  }, []);

  // 相机停下 320ms 后告诉上层"现在该用什么颜色"（滚动中不换色，避免抽搐）。
  useEffect(() => {
    if (!onFocusItems) return;
    const timer = window.setTimeout(() => {
      const top = camera.y / camera.zoom;
      const bottom = (camera.y + size.h) / camera.zoom;
      const center = (top + bottom) / 2;
      const span = Math.max(1, (bottom - top) / 2);
      const pool = infinite
        ? cellTiles
        : (() => {
            const slice = visibleSlice(layout.tiles, top, bottom, 0);
            return layout.tiles.slice(slice.start, slice.end);
          })();
      const scored = pool.map((tile) => {
        const visibleTop = Math.max(tile.rect.y, top);
        const visibleBottom = Math.min(tile.rect.y + tile.rect.h, bottom);
        const area = Math.max(0, visibleBottom - visibleTop);
        const middle = tile.rect.y + tile.rect.h / 2;
        const centrality = 1 - Math.min(1, Math.abs(middle - center) / span);
        return { item: tile.item, weight: area * (0.35 + centrality) };
      });
      scored.sort((left, right) => right.weight - left.weight);
      onFocusItems(scored.slice(0, 6).map((entry) => entry.item));
    }, 320);
    return () => window.clearTimeout(timer);
  }, [camera, cellTiles, infinite, layout.tiles, onFocusItems, size.h]);

  const worldStyle = useMemo(
    () => ({
      // 无限空间没有世界尺寸：.world 只当坐标系原点用，尺寸交给卡片自己。
      width: infinite ? undefined : worldWidth,
      height: infinite ? undefined : Math.max(layout.height, size.h),
      // worldInset：窄屏把世界右移，让开 64px 的导航缝（桌面为 0，图照样从缝底下穿过）。
      // origin：rebase 后的世界偏移，卡片按 (rect - origin) 渲染，这里把偏移补回屏幕。
      transform:
        'translate3d(' +
        (worldInset - camera.x + origin.x * camera.zoom) +
        'px,' +
        (-camera.y + origin.y * camera.zoom) +
        'px,0) scale(' +
        camera.zoom +
        ')',
      transformOrigin: '0 0',
    }),
    [camera, infinite, layout.height, origin.x, origin.y, size.h, worldInset, worldWidth],
  );

  const slice = useMemo(() => {
    const top = camera.y / camera.zoom;
    const bottom = (camera.y + size.h) / camera.zoom;
    return visibleSlice(layout.tiles, top, bottom);
  }, [camera, layout.tiles, size.h]);

  // P1-2：画布本身的键盘平移（Tab 进来后方向键/PageUp/PageDown，+/- 缩放）。
  const onCanvasKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const cam = cameraRef.current;
    const step = 120;
    const page = Math.max(200, Math.round(sizeRef.current.h * 0.9));
    const zoomBy = (factor: number) => {
      // 缩放锚点取"可见区"中心：窄屏要让开 64px 的缝，否则会往左上偏。
      const inset = sizeRef.current.w <= 760 ? 64 : 0;
      const cx = inset + (sizeRef.current.w - inset) / 2;
      const cy = sizeRef.current.h / 2;
      const worldX = (cam.x + cx) / cam.zoom;
      const worldY = (cam.y + cy) / cam.zoom;
      const nextZoom = Math.min(ZOOM_MAX, Math.max(ZOOM_MIN, cam.zoom * factor));
      stopInertia();
      moveCamera({ zoom: nextZoom, x: worldX * nextZoom - cx, y: worldY * nextZoom - cy });
    };
    switch (event.key) {
      case 'ArrowUp':
        stopInertia();
        moveCamera({ ...cam, y: cam.y - step });
        break;
      case 'ArrowDown':
        stopInertia();
        moveCamera({ ...cam, y: cam.y + step });
        break;
      case 'ArrowLeft':
        stopInertia();
        moveCamera({ ...cam, x: cam.x - step });
        break;
      case 'ArrowRight':
        stopInertia();
        moveCamera({ ...cam, x: cam.x + step });
        break;
      case 'PageUp':
        stopInertia();
        moveCamera({ ...cam, y: cam.y - page });
        break;
      case 'PageDown':
        stopInertia();
        moveCamera({ ...cam, y: cam.y + page });
        break;
      case 'Home':
        stopInertia();
        // 无限空间里 Home = 回到起点（§1.1 必做项）；有限世界仍回顶部。
        if (infinite) centerOnWorld(cellWidth / 2, cellHeight / 2);
        else moveCamera({ ...cam, y: 0 });
        break;
      case 'End':
        stopInertia();
        if (infinite) break;
        moveCamera({ ...cam, y: Math.max(0, layout.height * cam.zoom - sizeRef.current.h) });
        break;
      case '+':
      case '=':
        zoomBy(1.1);
        break;
      case '-':
      case '_':
        zoomBy(1 / 1.1);
        break;
      default:
        return;
    }
    event.preventDefault();
  };

  // 无限空间的卡片按 (rect - origin) 落到 DOM，坐标量级不随相机增长；
  // 有限世界 origin 恒为 0，等同于原来的世界坐标。
  const visible = useMemo(
    () =>
      infinite
        ? cellTiles.map((tile) => ({
            ...tile,
            rect: {
              x: tile.rect.x - origin.x,
              y: tile.rect.y - origin.y,
              w: tile.rect.w,
              h: tile.rect.h,
            },
          }))
        : layout.tiles.slice(slice.start, slice.end),
    [cellTiles, infinite, layout.tiles, origin.x, origin.y, slice.start, slice.end],
  );
  // 对角线波次从视野左上角起算（不是世界原点）：否则滚到中段换页签时会整屏一起延迟。
  const waveLeft = camera.x / camera.zoom - origin.x;
  const waveTop = camera.y / camera.zoom - origin.y;
  const enterDelayOf = (rect: { x: number; y: number }) => {
    if (!enter.on || !enter.wave) return 0;
    const column = Math.max(0, Math.floor((rect.x - waveLeft) / 300));
    const row = Math.max(0, Math.floor((rect.y - waveTop) / 280));
    return Math.min(340, (column + row) * 30);
  };
  const isEmpty = items.length === 0;
  const showEmpty = isEmpty && phase !== 'boot' && phase !== 'loading';
  // 无限空间要等全量到齐再生成格子：分片陆续到达会让每个格子的选图整体换一遍。
  const awaitingWorld = infinite && !ready && phase !== 'error';
  // 走了很远（超过约一屏半）才给「回到起点」——无限空间容易迷路（§1.1 必做）。
  const homeX = cellWidth / 2;
  const homeY = cellHeight / 2;
  const displaced =
    infinite &&
    (Math.abs(viewRect.x + viewRect.w / 2 - homeX) > viewRect.w * 1.6 ||
      Math.abs(viewRect.y + viewRect.h / 2 - homeY) > viewRect.h * 1.6);

  return (
    <div
      className={'canvas' + (dragging ? ' is-grabbing' : '')}
      ref={viewportRef}
      tabIndex={0}
      role="region"
      aria-label="图片画布：Tab 选卡片、Enter 打开、方向键平移、+/- 缩放、Home 回起点"
      onKeyDown={onCanvasKeyDown}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={onPointerCancel}
      onLostPointerCapture={() => {
        // 捕获被浏览器抢走（比如触控手势被系统接管）：别留一个"永远按着"的状态。
        // 缩放中不管——endPinch 会自己给剩下的手指重建基线。
        if (pinch.current || pointers.current.size > 0) return;
        endPointer(null, -1);
      }}
    >
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
      ) : awaitingWorld ? null : (
        <div className="world" style={worldStyle}>
          {visible.map((tile) => (
            <WallCard
              key={tile.item.id + '@' + Math.round(tile.rect.x) + ',' + Math.round(tile.rect.y)}
              item={tile.item}
              index={tile.index}
              paths={paths}
              rect={tile.rect}
              current={tile.item.id === activeId}
              enter={enter.on}
              enterDelay={enterDelayOf(tile.rect)}
              tagCounts={tagCounts}
              onOpen={(openedItem, openedIndex) => {
                setActiveId(openedItem.id);
                onOpen(openedItem, openedIndex);
              }}
              onSummary={(item, clientX, clientY) => {
                setActiveId(item.id);
                onSummary(item, clientX, clientY);
              }}
            />
          ))}
        </div>
      )}

      {/* 硬规则 1：首页不出现脚手架文案。计数只留在边栏底部（.seam__count）；
          加载态只有一条 1px 进度线，错误才给一句提示。 */}
      <div className="canvas__hint">拖拽平移 · 滚轮滚动 · Ctrl+滚轮缩放 · 右键看概要</div>

      <div className="canvas__progress" data-active={phase === 'loading' ? 'true' : undefined} aria-hidden="true" />

      {phase === 'error' ? <div className="canvas__error">数据加载中断</div> : null}

      {displaced ? (
        <button
          type="button"
          className="canvas__home"
          onClick={() => centerOnWorld(homeX, homeY)}
          title="回到起点（Home）"
        >
          回到起点
        </button>
      ) : null}
    </div>
  );
}
