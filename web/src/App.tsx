import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { BucketGallery, type BucketEntry } from './components/BucketGallery';
import { GestureDebug } from './components/GestureDebug';
import { Detail } from './components/Detail';
import { Sidebar, type RailView, type SortMode } from './components/Sidebar';
import { TagGallery, type TagEntry } from './components/TagGallery';
import { WallView, type WallControls } from './components/Wall';
import { accentOf, blendAccents } from './lib/accent';
import { tierUrl } from './lib/assetUrl';
import { beginExpand, containRect, flyImage, type Rect } from './lib/expand';
import { categoryLabel, displayGroup, displayTag, formatCount, ratingLabel } from './lib/format';
import {
  detailHref,
  favHref,
  favsHref,
  groupHref,
  groupsHref,
  navigate,
  parseUiParams,
  ratingHref,
  ratingsHref,
  replaceNavigate,
  tagHref,
  tagsHref,
  useRoute,
  useUiParamsSync,
  wallHref,
  writeUiParams,
  type UiParams,
} from './lib/router';
import { INSPECTOR_WIDTH, readLayoutMetrics } from './lib/layout';
import { appendShuffled, hashString } from './lib/shuffle';
import { DATA_ROOT } from './source/config';
import { StaticSource, type SearchRecord } from './source/StaticSource';
import {
  EMPTY_FILTERS,
  filterItems,
  filterSignature,
  isFilterActive,
  type FilterState,
  type SearchIndexPhase,
} from './state/filters';
import { useWall } from './state/useWall';
import type { TagFacet, WallItem } from './source/types';

const EMPTY_TAGS: TagFacet[] = [];
const RATING_ORDER = ['G', 'S', 'Q', 'E'];
const FALLBACK_ACCENT = '#8f8f92';

/**
 * 标签图少于这个数就回退有限世界：无界空间会循环铺同一组图，
 * 图片太少时视觉上就是「一张图铺满」，比留白更难读。8 张约能撑起一屏三列。
 */
const INFINITE_MIN_TAG_ITEMS = 8;

/** 焦点归位的最大重试帧数：从详情返回聚焦时相机要 360ms 居中，卡才进可见集合；30 帧约 500ms。 */
const FOCUS_RETRY_FRAMES = 30;

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

  const [filters, setFilters] = useState<FilterState>(() => ({
    query: parseUiParams(window.location.hash).q,
  }));
  const [sort, setSort] = useState<SortMode>(() => parseUiParams(window.location.hash).sort);
  const [detailItem, setDetailItem] = useState<WallItem | null>(null);
  const [detailMissing, setDetailMissing] = useState(false);
  const [allLoaded, setAllLoaded] = useState(false);
  const [focusItems, setFocusItems] = useState<WallItem[]>([]);
  const [summaryId, setSummaryId] = useState<string | null>(null);
  const [spotlightItem, setSpotlightItem] = useState<WallItem | null>(null);
  const [spotlightAnchor, setSpotlightAnchor] = useState<{ x: number; y: number } | null>(null);
  /** 被点卡片在点击瞬间的世界矩形：聚焦重排以它为锚，图片不会被搬到固定英雄位。 */
  const [spotlightRect, setSpotlightRect] = useState<Rect | null>(null);
  const [detailInfoHidden, setDetailInfoHidden] = useState(false);
  const [playing, setPlaying] = useState(false);
  const [mono, setMono] = useState(() => parseUiParams(window.location.hash).mono);
  const [matte, setMatte] = useState(() => parseUiParams(window.location.hash).matte);
  /** 搜索索引：首屏不拉；第一次需要搜索时才取，404/未就绪静默退回文件名搜索。 */
  const [searchPhase, setSearchPhase] = useState<SearchIndexPhase>('idle');
  const [searchRecords, setSearchRecords] = useState<ReadonlyMap<string, SearchRecord> | null>(null);
  const searchRequested = useRef(false);
  const [layout, setLayout] = useState(readLayoutMetrics);
  const [orderSeed] = useState(() => Math.floor(Math.random() * 0x7fffffff));
  const gestureDebug = useMemo(
    () => typeof window !== 'undefined' && new URLSearchParams(window.location.search).has('gestures'),
    [],
  );
  // 详情页仍保留进入前那条列表的页签高亮（详情本身没有页签）。
  const [railView, setRailView] = useState<RailView>('home');

  const scrollMemo = useRef(0);
  const previousRoute = useRef(route);
  const listHref = useRef(wallHref());
  // 焦点要还给谁：记录最近一次从墙上 / 详情里进入详情的条目 id。
  const returnFocusId = useRef<string | null>(null);
  const routeViewRef = useRef(route.view);
  /** 深链定位：记录「这个 id 探测到第几个分片」，纯 sync 不再触发新一轮 findItem。 */
  const detailProbe = useRef<{ id: string; loaded: number } | null>(null);
  const orderCache = useRef<{ source: readonly WallItem[]; list: WallItem[] }>({ source: [], list: [] });
  const wallRef = useRef(wall);
  const wallControls = useRef<WallControls | null>(null);
  wallRef.current = wall;

  useEffect(() => {
    if (!spotlightItem) {
      setSpotlightAnchor(null);
      setSpotlightRect(null);
    }
  }, [spotlightItem]);

  // 总览页（标签/收藏/分级/分组）不是画布：从详情/墙直接改 hash 过来时，
  // 残留的聚焦检查器会盖住封面墙，这里统一收掉。二级墙（tag/fav/rating/group）保留聚焦。
  useEffect(() => {
    if (route.view === 'tags' || route.view === 'favs' || route.view === 'ratings' || route.view === 'groups') {
      setSpotlightItem(null);
      setSummaryId(null);
    }
  }, [route.view]);

  useEffect(() => {
    const onResize = () => setLayout(readLayoutMetrics());
    window.addEventListener('resize', onResize);
    return () => window.removeEventListener('resize', onResize);
  }, []);

  // N：单色预览（参考 Harry Atkins 的 Monochrome 模式）。输入框内让行。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      if (event.key !== 'n' && event.key !== 'N') return;
      const target = event.target as Element | null;
      if (target?.closest('input, textarea, select, [contenteditable="true"]')) return;
      event.preventDefault();
      setMono((value) => !value);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

  // 搜索索引按需加载：打开搜索框或开始输入时才取，首屏不阻塞；
  // 失败（404/离线/结构不合约）只降级，不写 console。
  const requestSearchIndex = useCallback(() => {
    if (searchRequested.current) return;
    searchRequested.current = true;
    setSearchPhase('loading');
    void source.loadSearchIndex().then((data) => {
      if (!data) {
        setSearchPhase('unavailable');
        return;
      }
      setSearchRecords(data.records);
      setSearchPhase('ready');
    });
  }, [source]);

  useEffect(() => {
    if (filters.query.trim().length > 0) requestSearchIndex();
  }, [filters.query, requestSearchIndex]);

  // 浏览状态写进 hash query：replace 不产生历史条目，刷新/分享即可恢复。
  useEffect(() => {
    writeUiParams({ q: filters.query, sort, mono, matte });
  }, [filters.query, sort, mono, matte]);

  // 前进/后退或手改地址：把 URL 上的 q/sort/mono/matte 喂回状态。
  const applyUiParams = useCallback((ui: UiParams) => {
    setFilters({ query: ui.q });
    setSort(ui.sort);
    setMono(ui.mono);
    setMatte(ui.matte);
  }, []);
  useUiParamsSync(applyUiParams);

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

  const shown = useMemo(
    () => sortItems(filterItems(baseItems, filters, searchRecords), sort),
    [baseItems, filters, searchRecords, sort],
  );

  // 详情页的浏览序列跟用户当前看到的墙（筛后+排序后）一致。
  const detailBrowseItems = useMemo(() => {
    if (!detailItem) return orderedItems;
    return shown.some((entry) => entry.id === detailItem.id) ? shown : orderedItems;
  }, [detailItem, shown, orderedItems]);

  // 检查器只在详情页出现（双击放大后）：单击聚焦不再带信息面板，
  // 墙面上也没有右侧/底部让位（反馈问题 2）。
  const inspectorItem = detailItem && !detailInfoHidden ? detailItem : null;
  const inspectorOpen = Boolean(inspectorItem);

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
          label: displayGroup(group),
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

  // 分片全部到齐（或已无下一片）：此时已加载集合即全量，可当作计数兜底。
  // 不用 allLoaded 单指标：它比快照晚一帧，会让标签模式多闪一次。
  const dataComplete = allLoaded || (!wall.hasMore && wall.phase === 'end');

  // 标签规模决定用不用无界空间：facets 是发布时算好的全量计数，index 到达即可用；
  // facets 缺这个标签时才退回已加载集合，且等全量到位后再采用，
  // 避免把加载中途偏小的计数当成结论、让世界模式来回翻转。
  const tagItemCount =
    route.view === 'tag'
      ? tagCounts.get(route.tag) ?? (dataComplete ? baseItems.length : null)
      : null;

  // 无界空间：首页随机墙 + 图片足够多的单个标签随机墙。标签页四周补图，
  // 否则点顶部一行时上方留出大片空缺；图片太少的标签回退有限世界，
  // 避免同一张图循环铺满。显式排序、搜索、收藏/分级/分组墙保持有限世界。
  const infiniteWall =
    (route.view === 'wall' ||
      (route.view === 'tag' &&
        (tagItemCount === null || tagItemCount >= INFINITE_MIN_TAG_ITEMS))) &&
    !queryActive &&
    sort === 'random';

  // 世界模式也算进 resetKey：异常情况下（index 晚到、facets 缺失）模式切换时
  // 让相机按新世界重新定位，不会带着另一种坐标系的偏移进入画面。
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
    sort +
    '|' +
    (infiniteWall ? 'infinite' : 'finite');

  const indexPath = useMemo(
    () => new URL('index.json', new URL(DATA_ROOT, document.baseURI)).href,
    [],
  );

  // 所有列表/总览页都要全量分片：搜索与封面计数必须在整库上算，
  // 不能只看已滚到的部分（之前的"筛选只过滤已加载"就是这里的缺口）。
  // 未知路由只渲染缺失页，不需要拉数据。
  const needAll = route.view !== 'detail' && route.view !== 'notfound';
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
    if (!detailId) {
      detailProbe.current = null;
      setDetailItem(null);
      setDetailMissing(false);
      return;
    }
    const local = wall.items.find((item) => item.id === detailId);
    if (local) {
      setDetailItem(local);
      setDetailMissing(false);
      return;
    }
    setDetailItem(null);
    // 同一深链、分片数没变：停在当前缺失页，不再发起 findItem。
    // 否则「找不到 → sync 换 items 引用 → effect 重跑 → 再找不到」会无限循环。
    const probe = detailProbe.current;
    if (probe && probe.id === detailId && probe.loaded === wall.loadedShards) {
      if (!wall.hasMore && wall.phase !== 'error') setDetailMissing(true);
      return;
    }
    detailProbe.current = { id: detailId, loaded: wall.loadedShards };
    setDetailMissing(false);
    let alive = true;
    void wallRef.current.findItem(detailId).then((found) => {
      if (!alive) return;
      if (found) {
        setDetailItem(found);
        setDetailMissing(false);
      } else {
        setDetailMissing(true);
      }
    });
    return () => {
      alive = false;
    };
  }, [detailId, wall.items, wall.loadedShards, wall.hasMore, wall.phase]);

  useEffect(() => {
    if (route.view !== 'detail') {
      setRailView(route.view === 'wall' || route.view === 'notfound' ? 'home' : route.view);
    }
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
    if (route.view !== 'wall' || spotlightItem || focusItems.length === 0) return;
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
  }, [focusItems, wall.paths, route.view, spotlightItem]);

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
    if (spotlightItem) {
      let alive = true;
      void accentOf(spotlightItem.id, tierUrl(wall.paths, 't256', spotlightItem.id)).then((hex) => {
        if (alive) setAccent(hex ?? FALLBACK_ACCENT);
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
  }, [detailItem, spotlightItem, accentCoverId, route.view, wall.paths]);

  // 概要层关法：Esc、点卡片以外任何地方。卡片上的右键让行给 WallCard
  // 的开/关判定（再次右键退出，反馈问题 4b），不然这里会抢先关掉再被打开。
  useEffect(() => {
    if (!summaryId) return;
    const close = (event: PointerEvent) => {
      if (!(event.target instanceof Element)) return;
      if (event.target.closest('.tile__info')) return;
      if (event.button === 2 && event.target.closest('.tile')) return;
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

  const handleCloseSummary = useCallback(() => setSummaryId(null), []);

  // 双击 / Enter / 触屏单击：图片从卡片位置"长"到全屏。
  const handleOpen = useCallback(
    (item: WallItem, _index: number, origin?: Rect) => {
      setSummaryId(null);
      setDetailInfoHidden(false);
      returnFocusId.current = item.id;
      const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
      if (origin && !reduced) {
        // 详情默认带信息检查器：落点跳过 seam + 300px；窄屏是底部 sheet，图仍全宽。
        const areaX = layout.wide ? layout.seam + INSPECTOR_WIDTH : 0;
        const target = containRect(
          item.w,
          item.h,
          window.innerWidth - areaX,
          window.innerHeight,
          areaX,
          0,
        );
        beginExpand();
        flyImage(tierUrl(wall.paths, 't512', item.id), origin, target);
      }
      navigate(detailHref(item.id));
    },
    [layout, wall.paths],
  );

  const handleDetail = useCallback((id: string) => {
    returnFocusId.current = id;
    setSummaryId(null);
    setDetailInfoHidden(false);
    // 详情内翻页/放映：replace 当前历史条目，一次后退直接回列表。
    replaceNavigate(detailHref(id));
  }, []);

  /** 深链缺失页的「用此 ID 搜索」：把 id 写进 q，回到墙上继续找。 */
  const handleSearchId = useCallback((id: string) => {
    setFilters({ query: id });
    setSpotlightItem(null);
    navigate(wallHref());
    window.scrollTo(0, 0);
  }, []);

  /** 未知地址缺失页的「去搜索」：回首页并打开侧栏搜索面板（输入框自带 autoFocus）。 */
  const handleGoSearch = useCallback(() => {
    setSpotlightItem(null);
    navigate(wallHref());
    window.scrollTo(0, 0);
    // 侧栏常驻；等路由切到首页后再开面板，避免面板开在缺失页上。
    window.requestAnimationFrame(() => {
      document.getElementById('seam-panel-trigger-search')?.click();
    });
  }, []);

  // 自动放映：每张停留 4.2s，到最后一张回到第一张；离开详情自动停。
  const PLAY_INTERVAL = 4200;
  useEffect(() => {
    if (!playing || route.view !== 'detail' || !detailItem || detailBrowseItems.length < 2) return;
    const index = detailBrowseItems.findIndex((entry) => entry.id === detailItem.id);
    if (index < 0) return;
    const timer = window.setTimeout(() => {
      const next = detailBrowseItems[(index + 1) % detailBrowseItems.length];
      if (next) handleDetail(next.id);
    }, PLAY_INTERVAL);
    return () => window.clearTimeout(timer);
  }, [detailBrowseItems, detailItem, handleDetail, playing, route.view]);

  useEffect(() => {
    if (route.view !== 'detail' && playing) setPlaying(false);
  }, [playing, route.view]);

  const handleTogglePlay = useCallback(() => {
    if (!detailItem) return;
    setPlaying((value) => !value);
  }, [detailItem]);

  const handleToggleMatte = useCallback(() => setMatte((value) => !value), []);

  const handleSpotlight = useCallback(
    (item: WallItem | null, anchor?: { x: number; y: number }, rect?: Rect) => {
      setSpotlightItem(item);
      setSpotlightAnchor(anchor ?? null);
      setSpotlightRect(rect ?? null);
    },
    [],
  );

  const handleCloseInspector = useCallback(() => {
    setDetailInfoHidden(true);
  }, []);

  const handleToggleInfo = useCallback(() => {
    setDetailInfoHidden((value) => !value);
  }, []);

  const handleBack = useCallback(() => {
    navigate(listHref.current);
  }, []);

  const handleHome = useCallback(() => {
    setFilters(EMPTY_FILTERS);
    setSpotlightItem(null);
    navigate(wallHref());
    window.scrollTo(0, 0);
  }, []);

  const handleTags = useCallback(() => {
    setFilters(EMPTY_FILTERS);
    setSpotlightItem(null);
    navigate(tagsHref());
    window.scrollTo(0, 0);
  }, []);

  const handleFavs = useCallback(() => {
    setFilters(EMPTY_FILTERS);
    setSpotlightItem(null);
    navigate(favsHref());
    window.scrollTo(0, 0);
  }, []);

  const handleRatings = useCallback(() => {
    setFilters(EMPTY_FILTERS);
    setSpotlightItem(null);
    navigate(ratingsHref());
    window.scrollTo(0, 0);
  }, []);

  const handleGroups = useCallback(() => {
    setFilters(EMPTY_FILTERS);
    setSpotlightItem(null);
    navigate(groupsHref());
    window.scrollTo(0, 0);
  }, []);

  const handleOpenTag = useCallback((tag: string) => {
    setFilters(EMPTY_FILTERS);
    setSpotlightItem(null);
    navigate(tagHref(tag));
    window.scrollTo(0, 0);
  }, []);

  const handleOpenBucket = useCallback((href: string) => {
    setFilters(EMPTY_FILTERS);
    setSpotlightItem(null);
    navigate(href);
    window.scrollTo(0, 0);
  }, []);

  // 依赖只认 loadMore（useWall 里是稳定的 useCallback）。
  const handleLoadMore = useCallback(() => {
    wall.loadMore();
  }, [wall.loadMore]);

  // 侧栏功能键：墙页给缩放/回到起点，详情页给返回/翻页，两组互斥。
  const canvasOps = useMemo(
    () =>
      route.view === 'detail'
        ? null
        : {
            onZoomIn: () => wallControls.current?.zoomBy(1.25),
            onZoomOut: () => wallControls.current?.zoomBy(1 / 1.25),
            onZoomReset: () => wallControls.current?.zoomReset(),
            onHome: () => {
              setSpotlightItem(null);
              wallControls.current?.goHome();
            },
            homeTitle: infiniteWall ? '回到起点（Home）' : '回到顶部（Home）',
          },
    [infiniteWall, route.view],
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
            playing,
            matte,
            onTogglePlay: handleTogglePlay,
            onToggleMatte: handleToggleMatte,
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
    [
      detailBrowseItems,
      detailIndex,
      detailItem,
      handleBack,
      handleDetail,
      handleToggleMatte,
      handleTogglePlay,
      matte,
      playing,
    ],
  );

  const wallHead = useMemo(() => {
    switch (route.view) {
      case 'tag':
        return { title: displayTag(route.tag), count: shown.length };
      case 'fav':
        return { title: '收藏', count: shown.length };
      case 'rating':
        return { title: ratingLabel(route.rating), count: shown.length };
      case 'group':
        return { title: displayGroup(route.group), count: shown.length };
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

  /** 空态至少给一个可执行动作：清掉搜索，或回到该类内容的总览。 */
  const emptyAction = !filtered ? null : queryActive ? (
    <button type="button" className="ghost" onClick={() => setFilters((prev) => ({ ...prev, query: '' }))}>
      清空搜索
    </button>
  ) : route.view === 'fav' ? (
    <button type="button" className="ghost" onClick={handleHome}>
      回首页
    </button>
  ) : route.view === 'tag' ? (
    <button type="button" className="ghost" onClick={handleTags}>
      回标签总览
    </button>
  ) : route.view === 'rating' ? (
    <button type="button" className="ghost" onClick={handleRatings}>
      回分级总览
    </button>
  ) : route.view === 'group' ? (
    <button type="button" className="ghost" onClick={handleGroups}>
      回分组总览
    </button>
  ) : null;

  const loadingBuckets = !allLoaded && wall.hasMore;

  const inspector = inspectorItem
    ? {
        item: inspectorItem,
        items: detailBrowseItems,
        paths: wall.paths,
        onClose: handleCloseInspector,
        onBack: handleBack,
        onTagClick: handleOpenTag,
        onSimilar: handleDetail,
      }
    : null;

  return (
    <div className={'app' + (inspectorOpen ? ' has-inspector' : '') + (mono ? ' is-mono' : '')}>
      <Sidebar
        view={railView}
        onHome={handleHome}
        onTags={handleTags}
        onFavs={handleFavs}
        onRatings={handleRatings}
        onGroups={handleGroups}
        filters={filters}
        onFilters={setFilters}
        searchPhase={searchPhase}
        onSearchIntent={requestSearchIndex}
        loaded={wall.items.length}
        total={total}
        shown={shown.length}
        filtered={filtered}
        sort={sort}
        onSort={setSort}
        focusItem={focusItems[0] ?? null}
        canvas={canvasOps}
        detail={detailOps}
        inspector={inspector}
        detailInfoHidden={detailItem ? detailInfoHidden : undefined}
        onToggleInfo={detailItem ? handleToggleInfo : undefined}
        mono={mono}
        onToggleMono={() => setMono((value) => !value)}
      />

      <main className="stage">
        {detailId ? (
          detailItem ? (
            <Detail
              item={detailItem}
              items={detailBrowseItems}
              paths={wall.paths}
              infoOpen={!detailInfoHidden}
              playing={playing}
              matte={matte}
              onBack={handleBack}
              onOpenDetail={handleDetail}
              onToggleInfo={handleToggleInfo}
              onTogglePlay={handleTogglePlay}
              onToggleMatte={handleToggleMatte}
            />
          ) : detailMissing ? (
            <div className="stage__missing">
              <p className="stage__hint">找不到这张图片（可能不在已导出的范围里）</p>
              <div className="stage__missing-actions" role="group" aria-label="缺失图片的处理">
                <button type="button" className="ghost" onClick={handleHome}>
                  回首页
                </button>
                <button type="button" className="ghost" onClick={() => handleSearchId(detailId)}>
                  用此 ID 搜索
                </button>
              </div>
            </div>
          ) : (
            <p className="stage__hint">正在定位…</p>
          )
        ) : route.view === 'notfound' ? (
          <section className="stage__missing" aria-labelledby="notfound-title">
            <h1 className="stage__missing-title" id="notfound-title">
              这个地址没有对应的页面
            </h1>
            <p className="stage__missing-sub">链接可能拼错了，或者这个页面已经改名。</p>
            <div className="stage__missing-actions" role="group" aria-label="缺失页面的处理">
              <button type="button" className="ghost" onClick={handleHome}>
                回首页
              </button>
              <button type="button" className="ghost" onClick={handleGoSearch}>
                去搜索
              </button>
            </div>
          </section>
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
            entries={favEntries[0]?.count ? favEntries : []}
            paths={wall.paths}
            loading={loadingBuckets}
            emptyText="还没有收藏（收藏状态来自工作台）"
            emptyAction={
              <button type="button" className="ghost" onClick={handleHome}>
                回首页
              </button>
            }
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
            emptyAction={
              <button type="button" className="ghost" onClick={handleHome}>
                回首页
              </button>
            }
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
            emptyAction={
              <button type="button" className="ghost" onClick={handleHome}>
                回首页
              </button>
            }
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
              infinite={infiniteWall}
              ready={allLoaded}
              worldSeed={orderSeed}
              stableAppend={route.view === 'wall' && sort === 'random' && !queryActive}
              tagCounts={tagCounts}
              emptyText={emptyText}
              emptyHint={emptyHint}
              emptyAction={emptyAction}
              spotlightId={spotlightItem?.id ?? null}
              spotlightAnchor={spotlightAnchor}
              spotlightAnchorRect={spotlightRect}
              onSpotlightChange={handleSpotlight}
              onLoadMore={handleLoadMore}
              onOpen={handleOpen}
              onSummary={handleSummary}
              summaryId={summaryId}
              onCloseSummary={handleCloseSummary}
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
      {gestureDebug ? <GestureDebug /> : null}
    </div>
  );
}
