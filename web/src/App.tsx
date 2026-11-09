import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { accentOf, blendAccents } from './lib/accent';
import { tierUrl } from './lib/assetUrl';
import { Detail } from './components/Detail';
import { closeLightbox, openLightbox, primeLightboxTiers } from './components/Lightbox';
import { Sidebar, type RailView, type SortMode } from './components/Sidebar';
import { TagGallery, type TagEntry } from './components/TagGallery';
import { WallView } from './components/Wall';
import { categoryLabel, formatCount, formatDateTime, formatSize } from './lib/format';
import { detailHref, navigate, tagHref, tagsHref, useRoute, wallHref } from './lib/router';
import { appendShuffled, hashString } from './lib/shuffle';
import { DATA_ROOT } from './source/config';
import { StaticSource } from './source/StaticSource';
import { EMPTY_FILTERS, filterItems, filterSignature, isFilterActive, type FilterState } from './state/filters';
import { useWall } from './state/useWall';
import type { TagFacet, WallItem } from './source/types';

const EMPTY_TAGS: TagFacet[] = [];

function sortItems(items: readonly WallItem[], mode: SortMode): WallItem[] {
  if (mode === 'random') return [...items];
  const copy = [...items];
  if (mode === 'name') copy.sort((left, right) => left.n.localeCompare(right.n, 'zh-Hans-CN'));
  else if (mode === 'stars') copy.sort((left, right) => right.st - left.st || right.mtime - left.mtime);
  else copy.sort((left, right) => right.mtime - left.mtime);
  return copy;
}

/**
 * 应用外壳：左侧黑白"缝" + 一张可以拖拽平移的图片空间。
 * 四种页面：首页随机墙、标签总览、单标签墙、详情。
 */
export function App() {
  const [source] = useState(() => new StaticSource(DATA_ROOT));
  const wall = useWall(source);
  const route = useRoute();

  const [filters, setFilters] = useState<FilterState>(EMPTY_FILTERS);
  const [sort, setSort] = useState<SortMode>('random');
  const [detailItem, setDetailItem] = useState<WallItem | null>(null);
  const [detailMissing, setDetailMissing] = useState(false);
  const [allLoaded, setAllLoaded] = useState(false);
  const [focusItems, setFocusItems] = useState<WallItem[]>([]);
  const [summary, setSummary] = useState<{ item: WallItem; x: number; y: number } | null>(null);
  const [orderSeed] = useState(() => Math.floor(Math.random() * 0x7fffffff));

  const scrollMemo = useRef(0);
  const previousRoute = useRef(route);
  const probedId = useRef<string | null>(null);
  const listHref = useRef(wallHref());
  const orderCache = useRef<{ source: readonly WallItem[]; list: WallItem[] }>({ source: [], list: [] });
  const wallRef = useRef(wall);
  wallRef.current = wall;

  const detailId = route.view === 'detail' ? route.id : null;

  const orderedItems = useMemo(() => {
    const cache = orderCache.current;
    if (cache.source === wall.items) return cache.list;
    const previous = cache.source;
    const restarted =
      wall.items.length < previous.length || (previous.length === 0 && cache.list.length > 0);
    if (restarted) {
      const fresh = appendShuffled([], wall.items, orderSeed);
      orderCache.current = { source: wall.items, list: fresh };
      return fresh;
    }
    const incoming = wall.items.slice(previous.length);
    const next = appendShuffled(cache.list, incoming, orderSeed);
    orderCache.current = { source: wall.items, list: next };
    return next;
  }, [wall.items, orderSeed]);

  const shown = useMemo(() => {
    const base =
      route.view === 'tag'
        ? orderedItems.filter((item) => item.tags.some((tag) => tag[0] === route.tag))
        : orderedItems;
    return sortItems(filterItems(base, filters), sort);
  }, [orderedItems, filters, sort, route]);

  // 详情页的浏览序列跟用户当前看到的墙（筛后+排序后）一致——lightbox 用的也是 shown。
  // 只有当这张被筛掉（深链 #/d/<id> 进来）时才退回全库顺序，避免详情里算不出 prev/next。
  const detailBrowseItems = useMemo(() => {
    if (!detailItem) return orderedItems;
    return shown.some((entry) => entry.id === detailItem.id) ? shown : orderedItems;
  }, [detailItem, shown, orderedItems]);

  const signature = filterSignature(filters) + '|' + sort + '|' + (route.view === 'tag' ? route.tag : '');
  const filtered = isFilterActive(filters);
  const total = wall.index?.count ?? wall.items.length;
  const facetTags = wall.index?.facets?.tags ?? EMPTY_TAGS;
  const view: RailView = route.view === 'tags' ? 'tags' : route.view === 'tag' ? 'tag' : 'home';

  const groups = useMemo(() => {
    const fromIndex = wall.index?.facets?.groups;
    if (fromIndex && fromIndex.length > 0) return fromIndex;
    const seen = new Set<string>();
    for (const item of wall.items) if (item.group) seen.add(item.group);
    return [...seen].sort();
  }, [wall.index, wall.items]);

  const tagEntries = useMemo<TagEntry[]>(() => {
    const byTag = new Map<string, WallItem[]>();
    for (const item of wall.items) {
      for (const tag of item.tags) {
        const list = byTag.get(tag[0]);
        if (list) list.push(item);
        else byTag.set(tag[0], [item]);
      }
    }
    return facetTags
      .map((facet) => {
        const list = byTag.get(facet.name);
        const cover = list && list.length > 0 ? list[hashString(facet.name) % list.length] : undefined;
        return {
          name: facet.name,
          category: categoryLabel(facet.category),
          categoryKey: facet.category,
          count: facet.count,
          coverId: cover?.id,
        };
      })
      .sort((left, right) => right.count - left.count || left.name.localeCompare(right.name, 'zh-Hans-CN'));
  }, [wall.items, facetTags]);

  const tagTotal = useMemo(() => {
    if (route.view !== 'tag') return 0;
    const facet = facetTags.find((entry) => entry.name === route.tag);
    return facet?.count ?? shown.length;
  }, [facetTags, route, shown.length]);

  /** 角标用的「标签 → 张数」：来自 index.facets（B1）。facets 没到齐时角标只显示标签名。 */
  const tagCounts = useMemo(() => {
    const map = new Map<string, number>();
    for (const facet of facetTags) map.set(facet.name, facet.count);
    return map;
  }, [facetTags]);

  const indexPath = useMemo(
    () => new URL('index.json', new URL(DATA_ROOT, document.baseURI)).href,
    [],
  );

  // 标签页要全量条目（封面 + 单标签墙都从 items 里按 tag 捞）。
  // 无限空间（首页无筛选）同理：格子按全量池选图，片段陆续到达会让整屏换图。
  // useWall.loadMore() 是 void 的（内部 request() 是 fire-and-forget），不能 await；
  // 改成跟着快照推进：每次分片落地再驱动下一片，直到 hasMore=false。
  // 注意：首屏 boot/loading 时 hasMore 还是初始值 false（首片没到），此时绝不能据此判定
  // 「已经全量」——否则 allLoaded 被永久置真，直开 #/tags 只会停在 5,000 条。
  const infiniteHome = route.view === 'wall' && !filtered;
  const needAll = route.view === 'tags' || route.view === 'tag' || infiniteHome;
  useEffect(() => {
    if (!needAll || allLoaded) return;
    if (wall.phase === 'boot' || wall.phase === 'loading' || wall.phase === 'error') return;
    if (!wall.hasMore) {
      setAllLoaded(true);
      return;
    }
    // loadMore 在 busy 时会被静默丢弃，所以 loadedShards 也是驱动源：每落一片再要一片。
    wall.loadMore();
  }, [needAll, allLoaded, wall.hasMore, wall.phase, wall.loadedShards, wall.loadMore]);

  // allLoaded 只能表示「这一份数据已经拉全」：retry()/重新 boot 会把分片数清零，
  // 这时候必须把开关放回去，否则标签页的驱动器永久短路、只剩首片。
  useEffect(() => {
    if (wall.loadedShards === 0) setAllLoaded(false);
  }, [wall.loadedShards]);

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

  useEffect(() => {
    if (route.view === 'wall') listHref.current = wallHref();
    else if (route.view === 'tag') listHref.current = tagHref(route.tag);
  }, [route]);

  useEffect(() => {
    const wasList = previousRoute.current.view !== 'detail';
    const isList = route.view !== 'detail';
    if (wasList && !isList) {
      scrollMemo.current = window.scrollY;
      window.scrollTo(0, 0);
    } else if (!wasList && isList) {
      const top = scrollMemo.current;
      window.requestAnimationFrame(() => window.scrollTo(0, top));
    }
    previousRoute.current = route;
    setSummary(null);
  }, [route]);

  useEffect(() => {
    const first = wall.items[0];
    if (!first || probedId.current === first.id) return;
    probedId.current = first.id;
    primeLightboxTiers(wall.paths, first.id);
  }, [wall.items, wall.paths]);

  // 动态强调色：视野里权重最高的几张按 OKLCH 平均。
  const handleFocusItems = useCallback((items: WallItem[]) => setFocusItems(items), []);

  useEffect(() => {
    if (focusItems.length === 0) return;
    let alive = true;
    void (async () => {
      const sampled: { hex: string; weight: number }[] = [];
      for (let index = 0; index < focusItems.length; index += 1) {
        const item = focusItems[index];
        const hex = await accentOf(item.id, tierUrl(wall.paths, 't256', item.id));
        if (hex) sampled.push({ hex, weight: focusItems.length - index });
      }
      if (!alive) return;
      const blended = blendAccents(sampled);
      if (blended) document.documentElement.style.setProperty('--accent', blended);
    })();
    return () => {
      alive = false;
    };
  }, [focusItems, wall.paths]);

  // 详情页用这一张图自己的主色。
  useEffect(() => {
    if (!detailItem) return;
    let alive = true;
    void accentOf(detailItem.id, tierUrl(wall.paths, 't256', detailItem.id)).then((hex) => {
      if (alive && hex) document.documentElement.style.setProperty('--accent', hex);
    });
    return () => {
      alive = false;
    };
  }, [detailItem, wall.paths]);

  useEffect(() => {
    if (!summary) return;
    const close = () => setSummary(null);
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setSummary(null);
    };
    window.addEventListener('pointerdown', close);
    window.addEventListener('keydown', onKeyDown);
    return () => {
      window.removeEventListener('pointerdown', close);
      window.removeEventListener('keydown', onKeyDown);
    };
  }, [summary]);

  const handleSummary = useCallback((item: WallItem, clientX: number, clientY: number) => {
    setSummary({ item, x: clientX, y: clientY });
  }, []);

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
    navigate(listHref.current);
  }, []);

  const handleHome = useCallback(() => {
    closeLightbox();
    setFilters(EMPTY_FILTERS);
    navigate(wallHref());
    window.scrollTo(0, 0);
  }, []);

  const handleTags = useCallback(() => {
    closeLightbox();
    navigate(tagsHref());
    window.scrollTo(0, 0);
  }, []);

  const handleOpenTag = useCallback((tag: string) => {
    navigate(tagHref(tag));
    window.scrollTo(0, 0);
  }, []);

  // 依赖只认 loadMore（useWall 里是稳定的 useCallback）：挂 [wall] 会让它每次渲染换身份，
  // 进而让 Wall.tsx 的预取 effect 每次渲染都重跑。
  const handleLoadMore = useCallback(() => {
    wall.loadMore();
  }, [wall.loadMore]);

  return (
    <div className="app">
      <Sidebar
        view={view}
        onHome={handleHome}
        onTags={handleTags}
        filters={filters}
        onFilters={setFilters}
        groups={groups}
        loaded={wall.items.length}
        total={total}
        shown={shown.length}
        filtered={filtered}
        sort={sort}
        onSort={setSort}
        focusItem={focusItems[0] ?? null}
      />

      <main className="stage">
        {detailId ? (
          detailItem ? (
            <Detail
              key={detailItem.id}
              item={detailItem}
              items={detailBrowseItems}
              paths={wall.paths}
              onBack={handleBack}
              onOpenDetail={handleDetail}
              onTagClick={handleOpenTag}
            />
          ) : (
            <p className="stage__hint">
              {detailMissing ? '找不到这张图片（可能不在已导出的范围里）' : '正在定位…'}
            </p>
          )
        ) : route.view === 'tags' ? (
          <TagGallery
            entries={tagEntries}
            paths={wall.paths}
            loading={!allLoaded && wall.hasMore}
            onOpenTag={handleOpenTag}
          />
        ) : (
          <>
            {route.view === 'tag' ? (
              <header className="wallhead">
                <button type="button" className="wallhead__back" onClick={handleTags} title="回到标签">
                  ←
                </button>
                <h1 className="wallhead__title">{route.tag}</h1>
                <span className="wallhead__count">{formatCount(tagTotal)} 张</span>
              </header>
            ) : null}

            <WallView
              items={shown}
              paths={wall.paths}
              phase={wall.phase}
              hasMore={wall.hasMore}
              resetKey={signature}
              filtered={filtered || route.view === 'tag'}
              infinite={infiniteHome}
              ready={allLoaded}
              worldSeed={orderSeed}
              stableAppend={route.view === 'wall' && sort === 'random' && !filtered}
              tagCounts={tagCounts}
              onLoadMore={handleLoadMore}
              onOpen={handleOpen}
              onSummary={handleSummary}
              onFocusItems={handleFocusItems}
              onRetry={wall.retry}
            />
          </>
        )}

        {wall.error ? (
          <p className="app__error">
            数据源错误：{wall.error}（数据根 {indexPath}）
          </p>
        ) : null}
      </main>

      {summary ? (
        <SummaryCard
          item={summary.item}
          x={summary.x}
          y={summary.y}
          paths={wall.paths}
          onOpen={() => {
            setSummary(null);
            handleDetail(summary.item.id);
          }}
          onClose={() => setSummary(null)}
        />
      ) : null}
    </div>
  );
}

interface SummaryCardProps {
  item: WallItem;
  x: number;
  y: number;
  paths: ReturnType<typeof useWall>['paths'];
  onOpen: () => void;
  onClose: () => void;
}

/** 右键概要：只放最必要的信息，避免变成第二个详情页。 */
function SummaryCard({ item, x, y, paths, onOpen, onClose }: SummaryCardProps) {
  // 320 是 .summary 的视觉宽度；测量失败时的兜底值必须和 CSS 一致，否则贴边翻转会算错。
  const width = 320;
  const panelRef = useRef<HTMLDivElement | null>(null);
  const [placed, setPlaced] = useState<{ left: number; top: number } | null>(null);

  // 落点：默认贴在光标右下；右边放不下翻到光标左侧，下边放不下翻到光标上方，
  // 最后再整块夹进视口（原来的实现写死高 220px，内容一多就顶出屏幕底部）。
  useLayoutEffect(() => {
    const node = panelRef.current;
    if (!node) return;
    const place = () => {
      const box = node.getBoundingClientRect();
      const panelWidth = box.width || width;
      const panelHeight = box.height || 220;
      const margin = 12;
      const gap = 8;
      let left = x + gap;
      if (left + panelWidth > window.innerWidth - margin) left = x - panelWidth - gap;
      left = Math.min(
        Math.max(margin, left),
        Math.max(margin, window.innerWidth - panelWidth - margin),
      );
      let top = y + gap;
      if (top + panelHeight > window.innerHeight - margin) top = y - panelHeight - gap;
      top = Math.min(
        Math.max(margin, top),
        Math.max(margin, window.innerHeight - panelHeight - margin),
      );
      setPlaced((current) =>
        current && Math.abs(current.left - left) < 0.5 && Math.abs(current.top - top) < 0.5
          ? current
          : { left, top },
      );
    };
    place();
    window.addEventListener('resize', place);
    return () => window.removeEventListener('resize', place);
  }, [x, y, item.id, width]);

  // Esc / 点外部由 App 的 effect 统一处理；这里只保证焦点在浮层内，Tab 不会跑回画布。
  useEffect(() => {
    panelRef.current?.focus();
  }, []);

  return (
    <div
      ref={panelRef}
      className="summary"
      role="dialog"
      aria-label={'概要：' + item.n}
      tabIndex={-1}
      style={{
        left: placed ? placed.left : x + 8,
        top: placed ? placed.top : y + 8,
        width,
        visibility: placed ? 'visible' : 'hidden',
      }}
      onPointerDown={(event) => event.stopPropagation()}
    >
      <div className="summary__cover">
        <img src={tierUrl(paths, 't256', item.id)} alt={item.n} draggable={false} />
      </div>
      <div className="summary__body">
        <strong className="summary__name">{item.n}</strong>
        <span className="summary__meta">
          {formatSize(item.w, item.h)} · {item.mtime ? formatDateTime(item.mtime).slice(0, 10) : '—'}
        </span>
        <span className="summary__meta">
          {item.rt}
          {item.st > 0 ? ' · ' + '★'.repeat(Math.min(5, Math.round(item.st))) : ''}
          {item.fav ? ' · 已收藏' : ''}
        </span>
        {item.tags.length > 0 ? (
          <span className="summary__tags">
            {item.tags.slice(0, 4).map((tag) => tag[0]).join(' · ')}
            {item.tags.length > 4 ? ' …' : ''}
          </span>
        ) : null}
        <span className="summary__actions">
          <button type="button" className="ghost" onClick={onOpen}>
            打开详情
          </button>
          <button type="button" className="ghost" onClick={onClose}>
            关闭
          </button>
        </span>
      </div>
    </div>
  );
}