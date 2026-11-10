import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { BucketGallery, type BucketEntry } from './components/BucketGallery';
import { Detail } from './components/Detail';
import { Sidebar, type RailView, type SortMode } from './components/Sidebar';
import { TagGallery, type TagEntry } from './components/TagGallery';
import { WallView, type WallControls } from './components/Wall';
import { accentOf, blendAccents } from './lib/accent';
import { tierUrl } from './lib/assetUrl';
import { beginExpand, containRect, flyImage, type Rect } from './lib/expand';
import { categoryLabel, formatCount, ratingLabel } from './lib/format';
import {
  detailHref,
  favHref,
  favsHref,
  groupHref,
  groupsHref,
  navigate,
  ratingHref,
  ratingsHref,
  tagHref,
  tagsHref,
  useRoute,
  wallHref,
} from './lib/router';
import { appendShuffled, hashString } from './lib/shuffle';
import { DATA_ROOT } from './source/config';
import { StaticSource } from './source/StaticSource';
import { EMPTY_FILTERS, filterItems, filterSignature, isFilterActive, type FilterState } from './state/filters';
import { useWall } from './state/useWall';
import type { TagFacet, WallItem } from './source/types';

const EMPTY_TAGS: TagFacet[] = [];
const RATING_ORDER = ['G', 'S', 'Q', 'E'];
const FALLBACK_ACCENT = '#8f8f92';

/** 焦点归位的最大重试帧数：虚拟化的卡可能晚一两帧才挂载，8 帧（约 130ms）足够且不会久拖。 */
const FOCUS_RETRY_FRAMES = 8;

/** 二级墙：这些路由共用「筛选后的一组图 + 标题」形态。 */
const WALL_VIEWS = new Set(['tag', 'fav', 'rating', 'group']);

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
 * 页面：首页随机墙；标签/收藏/分级/分组四套「封面总览 → 二级墙」；详情。
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
  const [summaryId, setSummaryId] = useState<string | null>(null);
  const [orderSeed] = useState(() => Math.floor(Math.random() * 0x7fffffff));
  // 详情页仍保留进入前那条列表的页签高亮（详情本身没有页签）。
  const [railView, setRailView] = useState<RailView>('home');

  const scrollMemo = useRef(0);
  const previousRoute = useRef(route);
  const listHref = useRef(wallHref());
  // 焦点要还给谁：记录最近一次从墙上 / 详情里进入详情的条目 id。
  const returnFocusId = useRef<string | null>(null);
  const routeViewRef = useRef(route.view);
  const orderCache = useRef<{ source: readonly WallItem[]; list: WallItem[] }>({ source: [], list: [] });
  const wallRef = useRef(wall);
  const wallControls = useRef<WallControls | null>(null);
  wallRef.current = wall;

  const detailId = route.view === 'detail' ? route.id : null;
  const queryActive = isFilterActive(filters);
  const routeFiltered = WALL_VIEWS.has(route.view);

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

  /** 当前路由的第一层筛选：标签/收藏/分级/分组各自独立一条路径（Q8）。 */
  const baseItems = useMemo(() => {
    switch (route.view) {
      case 'tag':
        return orderedItems.filter((item) => item.tags.some((tag) => tag[0] === route.tag));
      case 'fav':
        return orderedItems.filter((item) => item.fav);
      case 'rating':
        return orderedItems.filter((item) => item.rt === route.rating);
      case 'group':
        return orderedItems.filter((item) => item.group === route.group);
      default:
        return orderedItems;
    }
  }, [orderedItems, route]);

  const shown = useMemo(() => sortItems(filterItems(baseItems, filters), sort), [baseItems, filters, sort]);

  // 详情页的浏览序列跟用户当前看到的墙（筛后+排序后）一致。
  const detailBrowseItems = useMemo(() => {
    if (!detailItem) return orderedItems;
    return shown.some((entry) => entry.id === detailItem.id) ? shown : orderedItems;
  }, [detailItem, shown, orderedItems]);

  const signature =
    route.view +
    ':' +
    ('tag' in route ? route.tag : '') +
    ':' +
    ('rating' in route ? route.rating : '') +
    ':' +
    ('group' in route ? route.group : '') +
    '|' +
    filterSignature(filters) +
    '|' +
    sort;
  const filtered = queryActive || routeFiltered;
  const total = wall.index?.count ?? wall.items.length;
  const facetTags = wall.index?.facets?.tags ?? EMPTY_TAGS;

  const groups = useMemo(() => {
    const fromIndex = wall.index?.facets?.groups;
    if (fromIndex && fromIndex.length > 0) return fromIndex;
    const seen = new Set<string>();
    for (const item of wall.items) if (item.group) seen.add(item.group);
    return [...seen].sort();
  }, [wall.index, wall.items]);

  /** 标签封面：确定性 + 段内不相邻重复。规则见 photo-wall-status.md §7。 */
  const tagEntries = useMemo<TagEntry[]>(() => {
    const byTag = new Map<string, WallItem[]>();
    for (const item of wall.items) {
      for (const tag of item.tags) {
        const list = byTag.get(tag[0]);
        if (list) list.push(item);
        else byTag.set(tag[0], [item]);
      }
    }
    const ordered = facetTags
      .map((facet) => ({
        name: facet.name,
        category: categoryLabel(facet.category),
        categoryKey: facet.category,
        count: facet.count,
        candidates: (byTag.get(facet.name) ?? []).map((item) => item.id).sort(),
      }))
      .sort((left, right) => right.count - left.count || left.name.localeCompare(right.name, 'zh-Hans-CN'));

    const used = new Map<string, Map<string, number>>();
    const prev = new Map<string, string>();
    const covers = new Map<string, string>();
    for (const entry of ordered) {
      if (entry.candidates.length === 0) continue;
      const key = entry.categoryKey || 'other';
      let counts = used.get(key);
      if (!counts) {
        counts = new Map();
        used.set(key, counts);
      }
      const offset = hashString(entry.name) % entry.candidates.length;
      const rotated = entry.candidates.slice(offset).concat(entry.candidates.slice(0, offset));
      const pool = rotated.filter((id) => id !== prev.get(key));
      const usable = pool.length > 0 ? pool : rotated;
      let pick = usable[0];
      let pickUses = counts.get(pick) ?? 0;
      for (const id of usable) {
        const uses = counts.get(id) ?? 0;
        if (uses < pickUses) {
          pick = id;
          pickUses = uses;
        }
      }
      counts.set(pick, pickUses + 1);
      prev.set(key, pick);
      covers.set(entry.name, pick);
    }

    return ordered.map((entry) => ({
      name: entry.name,
      category: entry.category,
      categoryKey: entry.categoryKey,
      count: entry.count,
      coverId: covers.get(entry.name),
    }));
  }, [wall.items, facetTags]);

  /** 收藏/分级/分组的第一层：计数与封面取原始加载顺序（mtime 降序），刷新稳定。 */
  const favEntries = useMemo<BucketEntry[]>(() => {
    const items = wall.items.filter((item) => item.fav);
    return [
      {
        key: 'fav',
        label: '收藏',
        count: items.length,
        coverId: items[0]?.id,
        href: favHref(),
      },
    ];
  }, [wall.items]);

  const ratingEntries = useMemo<BucketEntry[]>(
    () =>
      RATING_ORDER.map((rating) => {
        const items = wall.items.filter((item) => item.rt === rating);
        return {
          key: rating,
          label: ratingLabel(rating),
          count: items.length,
          coverId: items[0]?.id,
          href: ratingHref(rating),
        };
      }),
    [wall.items],
  );

  const groupEntries = useMemo<BucketEntry[]>(
    () =>
      groups.map((group) => {
        const items = wall.items.filter((item) => item.group === group);
        return {
          key: group,
          label: group,
          count: items.length,
          coverId: items[0]?.id,
          href: groupHref(group),
        };
      }),
    [groups, wall.items],
  );

  /** 角标用的「标签 → 张数」：来自 index.facets；facets 没到齐时角标只显示标签名。 */
  const tagCounts = useMemo(() => {
    const map = new Map<string, number>();
    for (const facet of facetTags) map.set(facet.name, facet.count);
    return map;
  }, [facetTags]);

  const indexPath = useMemo(
    () => new URL('index.json', new URL(DATA_ROOT, document.baseURI)).href,
    [],
  );

  // 所有列表/总览页都要全量分片：搜索与封面计数必须在整库上算，
  // 不能只看已滚到的部分（之前的"筛选只过滤已加载"就是这里的缺口）。
  const needAll = route.view !== 'detail';
  useEffect(() => {
    if (!needAll || allLoaded) return;
    if (wall.phase === 'boot' || wall.phase === 'loading' || wall.phase === 'error') return;
    if (!wall.hasMore) {
      setAllLoaded(true);
      return;
    }
    wall.loadMore();
  }, [needAll, allLoaded, wall.hasMore, wall.phase, wall.loadedShards, wall.loadMore]);

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
    if (route.view !== 'detail') setRailView(route.view === 'wall' ? 'home' : route.view);
  }, [route]);

  useEffect(() => {
    if (route.view === 'wall') listHref.current = wallHref();
    else if (route.view === 'tag') listHref.current = tagHref(route.tag);
    else if (route.view === 'fav') listHref.current = favHref();
    else if (route.view === 'rating') listHref.current = ratingHref(route.rating);
    else if (route.view === 'group') listHref.current = groupHref(route.group);
  }, [route]);

  // 焦点归位：从详情回到列表时，把焦点还给触发那张卡（虚拟化可能晚一两帧挂载）。
  useEffect(() => {
    const cameFromDetail = routeViewRef.current === 'detail';
    routeViewRef.current = route.view;
    if (route.view === 'detail') {
      returnFocusId.current = route.id;
      return;
    }
    const isWallRoute = route.view === 'wall' || WALL_VIEWS.has(route.view);
    if (!cameFromDetail || !isWallRoute) return;
    const id = returnFocusId.current;
    if (!id) return;
    let frame = 0;
    let raf = 0;
    const step = () => {
      const card = Array.from(document.querySelectorAll<HTMLElement>('.tile')).find(
        (node) => node.dataset.id === id,
      );
      if (card) {
        card.focus({ preventScroll: true });
        return;
      }
      frame += 1;
      if (frame <= FOCUS_RETRY_FRAMES) {
        raf = window.requestAnimationFrame(step);
        return;
      }
      document.querySelector<HTMLElement>('.canvas')?.focus({ preventScroll: true });
    };
    raf = window.requestAnimationFrame(step);
    return () => window.cancelAnimationFrame(raf);
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
    setSummaryId(null);
  }, [route]);

  const handleFocusItems = useCallback((items: WallItem[]) => setFocusItems(items), []);

  // 首页：视野里权重最高的几张按 OKLCH 平均。其余入口用各自封面（Q7a C）。
  useEffect(() => {
    if (route.view !== 'wall' || focusItems.length === 0) return;
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
  }, [focusItems, wall.paths, route.view]);

  /** 标签/收藏/分级/分组页（含总览）的强调色来源：该入口的封面。 */
  const accentCoverId = useMemo(() => {
    switch (route.view) {
      case 'tag':
        return tagEntries.find((entry) => entry.name === route.tag)?.coverId;
      case 'tags':
        // 总览首屏第一张可见卡是「通用」段第一项，用它而不是全库计数第一项。
        return (
          tagEntries.find((entry) => entry.categoryKey === 'general' && entry.coverId)?.coverId ??
          tagEntries.find((entry) => entry.coverId)?.coverId
        );
      case 'fav':
      case 'favs':
        return favEntries[0]?.coverId;
      case 'rating':
        return ratingEntries.find((entry) => entry.key === route.rating)?.coverId;
      case 'ratings':
        return ratingEntries.find((entry) => entry.coverId)?.coverId;
      case 'group':
        return groupEntries.find((entry) => entry.key === route.group)?.coverId;
      case 'groups':
        return groupEntries[0]?.coverId;
      default:
        return null;
    }
  }, [route, tagEntries, favEntries, ratingEntries, groupEntries]);

  useEffect(() => {
    const setAccent = (value: string) => document.documentElement.style.setProperty('--accent', value);
    if (detailItem) {
      let alive = true;
      void accentOf(detailItem.id, tierUrl(wall.paths, 't256', detailItem.id)).then((hex) => {
        if (alive && hex) setAccent(hex);
      });
      return () => {
        alive = false;
      };
    }
    if (accentCoverId) {
      let alive = true;
      void accentOf(accentCoverId, tierUrl(wall.paths, 't256', accentCoverId)).then((hex) => {
        if (alive) setAccent(hex ?? FALLBACK_ACCENT);
      });
      return () => {
        alive = false;
      };
    }
    // 首页交给上面的聚合 effect；其余没有封面的页面回中性灰，不能残留上一页的颜色。
    if (route.view !== 'wall') setAccent(FALLBACK_ACCENT);
  }, [detailItem, accentCoverId, route.view, wall.paths]);

  // 概要层关法：Esc、点卡片以外任何地方。
  useEffect(() => {
    if (!summaryId) return;
    const close = (event: PointerEvent) => {
      if (event.target instanceof Element && event.target.closest('.tile__info')) return;
      setSummaryId(null);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setSummaryId(null);
    };
    window.addEventListener('pointerdown', close);
    window.addEventListener('keydown', onKeyDown);
    return () => {
      window.removeEventListener('pointerdown', close);
      window.removeEventListener('keydown', onKeyDown);
    };
  }, [summaryId]);

  const handleSummary = useCallback((item: WallItem) => {
    setSummaryId(item.id);
  }, []);

  // 双击 / Enter / 触屏单击：图片从卡片位置"长"到全屏。
  const handleOpen = useCallback(
    (item: WallItem, _index: number, origin?: Rect) => {
      setSummaryId(null);
      returnFocusId.current = item.id;
      const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
      if (origin && !reduced) {
        const target = containRect(item.w, item.h, window.innerWidth, window.innerHeight);
        beginExpand();
        flyImage(tierUrl(wall.paths, 't512', item.id), origin, target);
      }
      navigate(detailHref(item.id));
    },
    [wall.paths],
  );

  const handleDetail = useCallback((id: string) => {
    returnFocusId.current = id;
    setSummaryId(null);
    navigate(detailHref(id));
  }, []);

  const handleBack = useCallback(() => {
    navigate(listHref.current);
  }, []);

  const handleHome = useCallback(() => {
    setFilters(EMPTY_FILTERS);
    navigate(wallHref());
    window.scrollTo(0, 0);
  }, []);

  const handleTags = useCallback(() => {
    setFilters(EMPTY_FILTERS);
    navigate(tagsHref());
    window.scrollTo(0, 0);
  }, []);

  const handleFavs = useCallback(() => {
    setFilters(EMPTY_FILTERS);
    navigate(favsHref());
    window.scrollTo(0, 0);
  }, []);

  const handleRatings = useCallback(() => {
    setFilters(EMPTY_FILTERS);
    navigate(ratingsHref());
    window.scrollTo(0, 0);
  }, []);

  const handleGroups = useCallback(() => {
    setFilters(EMPTY_FILTERS);
    navigate(groupsHref());
    window.scrollTo(0, 0);
  }, []);

  const handleOpenTag = useCallback((tag: string) => {
    setFilters(EMPTY_FILTERS);
    navigate(tagHref(tag));
    window.scrollTo(0, 0);
  }, []);

  const handleOpenBucket = useCallback((href: string) => {
    setFilters(EMPTY_FILTERS);
    navigate(href);
    window.scrollTo(0, 0);
  }, []);

  // 依赖只认 loadMore（useWall 里是稳定的 useCallback）。
  const handleLoadMore = useCallback(() => {
    wall.loadMore();
  }, [wall.loadMore]);

  const infiniteHome = route.view === 'wall' && !queryActive && sort === 'random';

  // 侧栏功能键：墙页给缩放/回到起点，详情页给返回/翻页，两组互斥。
  const canvasOps = useMemo(
    () =>
      route.view === 'detail'
        ? null
        : {
            onZoomIn: () => wallControls.current?.zoomBy(1.25),
            onZoomOut: () => wallControls.current?.zoomBy(1 / 1.25),
            onZoomReset: () => wallControls.current?.zoomReset(),
            onHome: () => wallControls.current?.goHome(),
            homeTitle: infiniteHome ? '回到起点（Home）' : '回到顶部（Home）',
          },
    [infiniteHome, route.view],
  );

  const detailIndex = detailItem ? detailBrowseItems.findIndex((e) => e.id === detailItem.id) : -1;
  const detailOps = useMemo(
    () =>
      detailItem && detailIndex >= 0
        ? {
            index: detailIndex,
            total: detailBrowseItems.length,
            hasPrev: detailIndex > 0,
            hasNext: detailIndex + 1 < detailBrowseItems.length,
            onBack: handleBack,
            onPrev: () => {
              const prev = detailBrowseItems[detailIndex - 1];
              if (prev) handleDetail(prev.id);
            },
            onNext: () => {
              const next = detailBrowseItems[detailIndex + 1];
              if (next) handleDetail(next.id);
            },
          }
        : null,
    [detailBrowseItems, detailIndex, detailItem, handleBack, handleDetail],
  );

  const wallHead = useMemo(() => {
    switch (route.view) {
      case 'tag':
        return { title: route.tag, count: shown.length };
      case 'fav':
        return { title: '收藏', count: shown.length };
      case 'rating':
        return { title: ratingLabel(route.rating), count: shown.length };
      case 'group':
        return { title: route.group, count: shown.length };
      default:
        return null;
    }
  }, [route, shown.length]);

  const emptyText = queryActive
    ? '没有符合条件的图片'
    : route.view === 'fav'
      ? '还没有收藏的图片'
      : route.view === 'rating'
        ? '这个分级还没有图片'
        : route.view === 'group'
          ? '这个分组还没有图片'
          : route.view === 'tag'
            ? '这个标签还没有图片'
            : undefined;
  const emptyHint = queryActive
    ? '清空搜索或换个关键词。'
    : route.view === 'fav'
      ? '在工作台里收藏图片后再发布。'
      : route.view === 'rating'
        ? '换个分级或回首页看看。'
        : route.view === 'group'
          ? '换个分组或回首页看看。'
          : undefined;

  const loadingBuckets = !allLoaded && wall.hasMore;

  return (
    <div className="app">
      <Sidebar
        view={railView}
        onHome={handleHome}
        onTags={handleTags}
        onFavs={handleFavs}
        onRatings={handleRatings}
        onGroups={handleGroups}
        filters={filters}
        onFilters={setFilters}
        loaded={wall.items.length}
        total={total}
        shown={shown.length}
        filtered={filtered}
        sort={sort}
        onSort={setSort}
        focusItem={focusItems[0] ?? null}
        canvas={canvasOps}
        detail={detailOps}
      />

      <main className="stage">
        {detailId ? (
          detailItem ? (
            <Detail
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
            loading={loadingBuckets}
            onOpenTag={handleOpenTag}
          />
        ) : route.view === 'favs' ? (
          <BucketGallery
            title="收藏"
            subtitle={formatCount(favEntries[0]?.count ?? 0) + ' 张'}
            entries={favEntries}
            paths={wall.paths}
            loading={loadingBuckets}
            emptyText="还没有收藏（收藏状态来自工作台）"
            onOpen={handleOpenBucket}
          />
        ) : route.view === 'ratings' ? (
          <BucketGallery
            title="分级"
            subtitle={formatCount(ratingEntries.length) + ' 个分级'}
            entries={ratingEntries}
            paths={wall.paths}
            loading={loadingBuckets}
            emptyText="这份数据没有分级"
            onOpen={handleOpenBucket}
          />
        ) : route.view === 'groups' ? (
          <BucketGallery
            title="分组"
            subtitle={formatCount(groupEntries.length) + ' 个分组'}
            entries={groupEntries}
            paths={wall.paths}
            loading={loadingBuckets}
            emptyText="这份数据没有分组"
            onOpen={handleOpenBucket}
          />
        ) : (
          <>
            {wallHead ? (
              <header className="wallhead">
                <h1 className="wallhead__title">{wallHead.title}</h1>
                <span className="wallhead__count">{formatCount(wallHead.count)} 张</span>
              </header>
            ) : null}

            <WallView
              items={shown}
              paths={wall.paths}
              phase={wall.phase}
              hasMore={wall.hasMore}
              resetKey={signature}
              filtered={filtered}
              infinite={infiniteHome}
              ready={allLoaded}
              worldSeed={orderSeed}
              stableAppend={route.view === 'wall' && sort === 'random' && !queryActive}
              tagCounts={tagCounts}
              emptyText={emptyText}
              emptyHint={emptyHint}
              onLoadMore={handleLoadMore}
              onOpen={handleOpen}
              onSummary={handleSummary}
              summaryId={summaryId}
              onCloseSummary={() => setSummaryId(null)}
              controlsRef={wallControls}
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
    </div>
  );
}
