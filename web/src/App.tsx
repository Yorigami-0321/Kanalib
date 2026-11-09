import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ReactZoomPanPinchRef } from 'react-zoom-pan-pinch';
import { Detail } from './components/Detail';
import {
  closeLightbox,
  isLightboxOpen,
  openLightbox,
  primeLightboxTiers,
} from './components/Lightbox';
import { Sidebar, type SortMode } from './components/Sidebar';
import { WallView } from './components/Wall';
import { ZoomStage } from './components/ZoomStage';
import { detailHref, navigate, useRoute, wallHref } from './lib/router';
import { DATA_ROOT } from './source/config';
import { StaticSource } from './source/StaticSource';
import { EMPTY_FILTERS, filterItems, filterSignature, isFilterActive, type FilterState } from './state/filters';
import { useWall } from './state/useWall';
import type { TagFacet, WallItem } from './source/types';

const EMPTY_TAGS: TagFacet[] = [];

function isTypingTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName.toLowerCase();
  return tag === 'input' || tag === 'textarea' || tag === 'select' || target.isContentEditable;
}

function sortItems(items: readonly WallItem[], mode: SortMode): WallItem[] {
  const copy = [...items];
  if (mode === 'name') {
    copy.sort((left, right) => left.n.localeCompare(right.n, 'zh-Hans-CN'));
  } else if (mode === 'stars') {
    copy.sort((left, right) => right.st - left.st || right.mtime - left.mtime);
  } else {
    copy.sort((left, right) => right.mtime - left.mtime);
  }
  return copy;
}

/**
 * 应用外壳：左侧边栏 + 整屏墙面（或详情页）。
 * 数据只从 StaticSource 来；路由只有墙与详情两种形状。
 */
export function App() {
  const [source] = useState(() => new StaticSource(DATA_ROOT));
  const wall = useWall(source);
  const route = useRoute();

  const [filters, setFilters] = useState<FilterState>(EMPTY_FILTERS);
  const [sort, setSort] = useState<SortMode>('time');
  const [railOpen, setRailOpen] = useState(false);
  const [zoomMode, setZoomMode] = useState(false);
  const [scale, setScale] = useState(1);
  const [detailItem, setDetailItem] = useState<WallItem | null>(null);
  const [detailMissing, setDetailMissing] = useState(false);

  const zoomApi = useRef<ReactZoomPanPinchRef | null>(null);
  const scrollMemo = useRef(0);
  const previousRoute = useRef(route);
  const probedId = useRef<string | null>(null);
  const wallRef = useRef(wall);
  wallRef.current = wall;

  const detailId = route.view === 'detail' ? route.id : null;

  const shown = useMemo(
    () => sortItems(filterItems(wall.items, filters), sort),
    [wall.items, filters, sort],
  );
  const signature = filterSignature(filters) + '|' + sort;
  const filtered = isFilterActive(filters);
  const total = wall.index?.count ?? wall.items.length;
  const facetTags = wall.index?.facets?.tags ?? EMPTY_TAGS;

  const groups = useMemo(() => {
    const fromIndex = wall.index?.facets?.groups;
    if (fromIndex && fromIndex.length > 0) return fromIndex;
    const seen = new Set<string>();
    for (const item of wall.items) {
      if (item.group) seen.add(item.group);
    }
    return [...seen].sort();
  }, [wall.index, wall.items]);

  const indexPath = useMemo(
    () => new URL('index.json', new URL(DATA_ROOT, document.baseURI)).href,
    [],
  );

  // 深链详情：先查已加载条目，再让数据源往后探分片找。
  useEffect(() => {
    setDetailMissing(false);
    if (!detailId) {
      setDetailItem(null);
      return;
    }
    const local = wallRef.current.items.find((item) => item.id === detailId);
    if (local) {
      setDetailItem(local);
      return;
    }
    setDetailItem(null);
    let alive = true;
    void wallRef.current.findItem(detailId).then((found) => {
      if (!alive) return;
      if (found) setDetailItem(found);
      else setDetailMissing(true);
    });
    return () => {
      alive = false;
    };
  }, [detailId, wall.items]);

  // 路由带标签参数时并进当前筛选。
  useEffect(() => {
    if (route.view !== 'wall' || !route.tag) return;
    const tag = route.tag;
    setFilters((current) =>
      current.tags.includes(tag) ? current : { ...current, tags: [...current.tags, tag] },
    );
  }, [route]);

  // 墙与详情之间切换：记住墙的滚动位置，切回来时恢复。
  useEffect(() => {
    const wasWall = previousRoute.current.view === 'wall';
    const isWall = route.view === 'wall';
    if (wasWall && !isWall) {
      scrollMemo.current = window.scrollY;
      window.scrollTo(0, 0);
    } else if (!wasWall && isWall) {
      const top = scrollMemo.current;
      window.requestAnimationFrame(() => window.scrollTo(0, top));
    }
    previousRoute.current = route;
    zoomApi.current?.resetTransform(0);
    setZoomMode(false);
  }, [route]);

  useEffect(() => {
    document.body.classList.toggle('is-zoom-locked', zoomMode);
    return () => document.body.classList.remove('is-zoom-locked');
  }, [zoomMode]);

  useEffect(() => {
    const first = wall.items[0];
    if (!first || probedId.current === first.id) return;
    probedId.current = first.id;
    primeLightboxTiers(wall.paths, first.id);
  }, [wall.items, wall.paths]);

  const setZoom = useCallback((next: boolean) => {
    if (!next) zoomApi.current?.resetTransform(0);
    setZoomMode(next);
  }, []);

  const toggleZoom = useCallback(() => {
    if (zoomMode) {
      setZoom(false);
      return;
    }
    window.requestAnimationFrame(() => zoomApi.current?.resetTransform(0));
    setZoom(true);
  }, [zoomMode, setZoom]);

  const fitScreen = useCallback(() => {
    zoomApi.current?.resetTransform(0);
    setZoomMode(false);
  }, []);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (isTypingTarget(event.target)) return;
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      if (event.key === 'Escape') {
        if (isLightboxOpen()) return;
        if (zoomMode) {
          event.preventDefault();
          setZoom(false);
        }
        return;
      }
      if (event.key.toLowerCase() === 'w') {
        if (detailId) return;
        event.preventDefault();
        toggleZoom();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [detailId, setZoom, toggleZoom, zoomMode]);

  const handleOpen = useCallback(
    (_item: WallItem, index: number) => {
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
    closeLightbox();
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
    window.scrollTo(0, 0);
  }, []);

  const handleTagClick = useCallback((tag: string) => {
    setFilters((current) => ({ ...current, tags: [tag] }));
    navigate(wallHref());
  }, []);

  const handleLoadMore = useCallback(() => {
    void wall.loadMore();
  }, [wall]);

  return (
    <div
      className={'app' + (railOpen ? ' app--rail-open' : '') + (zoomMode ? ' app--zooming' : '')}
    >
      <Sidebar
        filters={filters}
        onFilters={setFilters}
        facetTags={facetTags}
        groups={groups}
        loaded={wall.items.length}
        total={total}
        shown={shown.length}
        filtered={filtered}
        sort={sort}
        onSort={setSort}
        expanded={railOpen}
        onExpandedChange={setRailOpen}
        zoomMode={zoomMode}
        scale={scale}
        onToggleZoom={toggleZoom}
        onFitScreen={fitScreen}
        onHome={handleHome}
      />

      <main className="stage">
        {detailId ? (
          detailItem ? (
            <Detail
              key={detailItem.id}
              item={detailItem}
              items={wall.items}
              paths={wall.paths}
              onBack={handleBack}
              onOpenDetail={handleDetail}
              onTagClick={handleTagClick}
            />
          ) : (
            <p className="stage__hint">
              {detailMissing ? '找不到这张图片（可能不在已导出的范围里）' : '正在定位…'}
            </p>
          )
        ) : (
          <ZoomStage zoomMode={zoomMode} apiRef={zoomApi} onScaleChange={setScale}>
            <WallView
              items={shown}
              paths={wall.paths}
              phase={wall.phase}
              hasMore={wall.hasMore}
              resetKey={signature}
              loadedCount={wall.items.length}
              totalCount={total}
              filtered={filtered}
              onLoadMore={handleLoadMore}
              onOpen={handleOpen}
              onDetail={handleDetail}
              onRetry={wall.retry}
            />
          </ZoomStage>
        )}

        {wall.error ? (
          <p className="app__error">
            数据源错误：{wall.error}（数据根 {indexPath}）
          </p>
        ) : null}
      </main>

      {zoomMode ? (
        <div className="zoom-pill">
          <span>{Math.round(scale * 100)}%</span>
          <button type="button" className="ghost" onClick={fitScreen}>
            适应屏幕
          </button>
          <button type="button" className="ghost is-on" onClick={toggleZoom}>
            退出
          </button>
        </div>
      ) : null}
    </div>
  );
}
