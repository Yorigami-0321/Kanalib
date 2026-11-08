import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ReactZoomPanPinchRef } from 'react-zoom-pan-pinch';
import { Detail } from './components/Detail';
import { closeLightbox, isLightboxOpen, openLightbox, primeLightboxTiers } from './components/Lightbox';
import { Toolbar } from './components/Toolbar';
import { WallView } from './components/Wall';
import { ZoomStage } from './components/ZoomStage';
import { detailHref, navigate, useRoute, wallHref, type Route } from './lib/router';
import { DATA_ROOT, StaticSource } from './source';
import type { WallItem } from './source/types';
import {
  EMPTY_FILTERS,
  filterItems,
  filterSignature,
  isFilterActive,
  type FilterState,
} from './state/filters';
import { useWall } from './state/useWall';

function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || target.isContentEditable;
}

export function App() {
  const [source] = useState(() => new StaticSource(DATA_ROOT));
  const wall = useWall(source);
  const route = useRoute();

  const [filters, setFilters] = useState<FilterState>(EMPTY_FILTERS);
  const [zoomMode, setZoomMode] = useState(false);
  const [scale, setScale] = useState(1);
  const [resolving, setResolving] = useState(false);

  const zoomApi = useRef<ReactZoomPanPinchRef | null>(null);
  const scrollMemo = useRef(0);
  const previousRoute = useRef<Route>(route);
  const probedId = useRef<string | null>(null);

  const shown = useMemo(() => filterItems(wall.items, filters), [wall.items, filters]);
  const signature = filterSignature(filters);
  const filterActive = isFilterActive(filters);
  const indexPath = useMemo(() => {
    try {
      return new URL('index.json', new URL(DATA_ROOT, document.baseURI)).href;
    } catch {
      return `${DATA_ROOT}index.json`;
    }
  }, []);

  // 路由里的 ?tag= 变成筛选 chip（详情页点标签回流到墙）。
  useEffect(() => {
    if (route.view !== 'wall') return;
    const tag = route.tag;
    if (!tag) return;
    setFilters((prev) =>
      prev.tags.includes(tag) ? prev : { ...prev, tags: [...prev.tags, tag] },
    );
  }, [route]);

  // 墙 ⇄ 详情切换时记住/恢复滚动位置。
  useEffect(() => {
    const previous = previousRoute.current;
    if (previous.view === 'wall' && route.view === 'detail') {
      scrollMemo.current = window.scrollY;
      zoomApi.current?.resetTransform(0);
      setZoomMode(false);
    } else if (previous.view === 'detail' && route.view === 'wall') {
      const target = scrollMemo.current;
      const restore = () => window.scrollTo({ top: target });
      requestAnimationFrame(restore);
      const timers = [window.setTimeout(restore, 160), window.setTimeout(restore, 480)];
      previousRoute.current = route;
      return () => timers.forEach((timer) => window.clearTimeout(timer));
    }
    previousRoute.current = route;
    return undefined;
  }, [route]);

  // 进入缩放：先把变换复位到 1:1，避免上一次遗留的缩放被带进来。
  const enterZoomMode = useCallback(() => {
    setZoomMode(true);
    window.requestAnimationFrame(() => zoomApi.current?.resetTransform(0));
  }, []);

  // 退出缩放：复位内部状态 + 关模式（视觉复位由 .app:not(.app--zooming) 的 CSS 兜底）。
  const exitZoomMode = useCallback(() => {
    zoomApi.current?.resetTransform(0);
    setZoomMode(false);
  }, []);

  // W 切换缩放模式；Esc 先退缩放模式（lightbox 打开时交给 PhotoSwipe 自己处理）。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        if (isLightboxOpen()) return;
        if (zoomMode) exitZoomMode();
        return;
      }
      if ((event.key === 'w' || event.key === 'W') && !event.metaKey && !event.ctrlKey && !isTypingTarget(event.target)) {
        if (zoomMode) exitZoomMode();
        else enterZoomMode();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [zoomMode, exitZoomMode]);

  // 缩放模式锁住页面滚动，避免与无限加载抢滚轮。
  useEffect(() => {
    document.body.classList.toggle('is-zoom-locked', zoomMode);
    return () => document.body.classList.remove('is-zoom-locked');
  }, [zoomMode]);

  // 首屏有数据后探一次 t1024 档是否产出（决定 lightbox 的首选档位）。
  useEffect(() => {
    primeLightboxTiers(wall.paths, wall.items[0]?.id);
  }, [wall.paths, wall.items]);

  // 详情页深链：没加载到就顺序补拉分片找一次。
  const detailId = route.view === 'detail' ? route.id : null;
  useEffect(() => {
    if (!detailId) {
      probedId.current = null;
      return;
    }
    if (wall.items.some((candidate) => candidate.id === detailId)) return;
    if (probedId.current === detailId) return;
    probedId.current = detailId;
    setResolving(true);
    void wall.findItem(detailId).finally(() => setResolving(false));
  }, [detailId, wall.items, wall.findItem]);

  const handleOpenLightbox = useCallback(
    (item: WallItem, index: number) => {
      void item;
      openLightbox({
        items: shown,
        startIndex: index,
        paths: wall.paths,
        onDetail: (id) => navigate(detailHref(id)),
      });
    },
    [shown, wall.paths],
  );

  const handleDetail = useCallback((id: string) => {
    navigate(detailHref(id));
  }, []);

  const handleBack = useCallback(() => {
    closeLightbox();
    navigate(wallHref());
  }, []);

  const handleHome = useCallback(() => {
    closeLightbox();
    setFilters(EMPTY_FILTERS);
    navigate(wallHref());
    window.scrollTo({ top: 0 });
  }, []);

  const handleTagClick = useCallback((tag: string) => {
    setFilters((prev) =>
      prev.tags.includes(tag) ? prev : { ...prev, tags: [...prev.tags, tag] },
    );
    navigate(wallHref(tag));
  }, []);

  const handleSearchMore = useCallback(() => {
    if (!detailId) return;
    setResolving(true);
    void wall.loadMoreUntilFound(detailId).finally(() => setResolving(false));
  }, [detailId, wall]);

  const toggleZoom = useCallback(() => {
    if (zoomMode) exitZoomMode();
    else enterZoomMode();
  }, [zoomMode, exitZoomMode, enterZoomMode]);

  const fitScreen = useCallback(() => {
    exitZoomMode();
  }, [exitZoomMode]);

  const facets = wall.index?.facets;

  return (
    <div className={`app${zoomMode ? ' app--zooming' : ''}`}>
      <Toolbar
        filters={filters}
        onChange={setFilters}
        onReset={() => setFilters(EMPTY_FILTERS)}
        tags={facets?.tags ?? []}
        groups={facets?.groups ?? []}
        loaded={wall.items.length}
        total={wall.index?.count ?? 0}
        shown={shown.length}
        filterActive={filterActive}
        zoomMode={zoomMode}
        scale={scale}
        onToggleZoom={toggleZoom}
        onFitScreen={fitScreen}
        onHome={handleHome}
        showBack={route.view === 'detail'}
      />

      {detailId ? (
        <Detail
          key={detailId}
          id={detailId}
          items={wall.items}
          paths={wall.paths}
          hasMore={wall.hasMore}
          looking={resolving}
          onBack={handleBack}
          onOpenDetail={handleDetail}
          onSearchMore={handleSearchMore}
          onTagClick={handleTagClick}
        />
      ) : (
        <main className="wall-main">
          <ZoomStage zoomMode={zoomMode} apiRef={zoomApi} onScaleChange={setScale}>
            <WallView
              items={shown}
              paths={wall.paths}
              phase={wall.phase}
              hasMore={wall.hasMore}
              resetKey={signature}
              loadedCount={wall.items.length}
              totalCount={wall.index?.count ?? 0}
              filtered={filterActive}
              onLoadMore={wall.loadMore}
              onOpen={handleOpenLightbox}
              onDetail={handleDetail}
              onRetry={wall.retry}
            />
          </ZoomStage>
          {wall.error ? (
            <p className="app__error" role="status">
              数据源错误：{wall.error}（数据根 {indexPath}）
            </p>
          ) : null}
        </main>
      )}
    </div>
  );
}
