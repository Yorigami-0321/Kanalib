import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { accentOf, blendAccents } from './lib/accent';
import { tierUrl } from './lib/assetUrl';
import { Detail } from './components/Detail';
import { Sidebar, type RailView, type SortMode } from './components/Sidebar';
import { TagGallery, type TagEntry } from './components/TagGallery';
import { WallView, type WallControls } from './components/Wall';
import { beginExpand, containRect, flyImage, type Rect } from './lib/expand';
import { categoryLabel, formatCount } from './lib/format';
import { detailHref, navigate, tagHref, tagsHref, useRoute, wallHref } from './lib/router';
import { appendShuffled, hashString } from './lib/shuffle';
import { DATA_ROOT } from './source/config';
import { StaticSource } from './source/StaticSource';
import { EMPTY_FILTERS, filterItems, filterSignature, isFilterActive, type FilterState } from './state/filters';
import { useWall } from './state/useWall';
import type { TagFacet, WallItem } from './source/types';

const EMPTY_TAGS: TagFacet[] = [];

/** 焦点归位的最大重试帧数：虚拟化的卡可能晚一两帧才挂载，8 帧（约 130ms）足够且不会久拖。 */
const FOCUS_RETRY_FRAMES = 8;

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
  const [summaryId, setSummaryId] = useState<string | null>(null);
  const [orderSeed] = useState(() => Math.floor(Math.random() * 0x7fffffff));

  const scrollMemo = useRef(0);
  const previousRoute = useRef(route);
  const listHref = useRef(wallHref());
  // 焦点要还给谁：记录最近一次从墙上 / 详情里进入详情的条目 id。
  // "详情 → Esc 回墙"中间换过路由、墙还重挂过，浏览器不会自己把焦点还回去，
  // 所以这里单独留一份。
  const returnFocusId = useRef<string | null>(null);
  const routeViewRef = useRef(route.view);
  const orderCache = useRef<{ source: readonly WallItem[]; list: WallItem[] }>({ source: [], list: [] });
  const wallRef = useRef(wall);
  // 侧栏功能键的操纵句柄：缩放/复位/回到起点经 ref 直调 Wall，不经过 React 状态。
  const wallControls = useRef<WallControls | null>(null);
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

  /**
   * 标签封面：确定性 + 段内不相邻重复。
   *
   * 旧做法是 `list[hash(name) % list.length]`，只看单个标签。样本库里 135 个标签却只有
   * 5 张带标签的图，整段会连着出现同一张照片，看起来像渲染坏了。现在按 categoryKey 分段
   * 贪心：先避开左邻那张卡的封面，再在本段内挑「已被用作封面次数最少」的候选，
   * 由 hash(name) 的轮转顺序决定起点与打平。全程不掷随机数，同一份数据刷新结果一致。
   * 只有单图标签（没有别的候选）允许重复，这是数据决定的，不是策略问题。
   *
   * 段内顺序必须和 TagGallery 一致（张数降序、同名按名称）——它会拿 entries 再用同一个
   * 比较器排一次；这里保持同序，"上一张封面"才对得上真实的相邻关系。
   */
  const tagEntries = useMemo<TagEntry[]>(() => {
    const byTag = new Map<string, WallItem[]>();
    for (const item of wall.items) {
      for (const tag of item.tags) {
        const list = byTag.get(tag[0]);
        if (list) list.push(item);
        else byTag.set(tag[0], [item]);
      }
    }
    // 候选 id 先排序：wall.items 带会话级 shuffle，直接沿用会让封面每次刷新都变。
    const ordered = facetTags
      .map((facet) => ({
        name: facet.name,
        category: categoryLabel(facet.category),
        categoryKey: facet.category,
        count: facet.count,
        candidates: (byTag.get(facet.name) ?? []).map((item) => item.id).sort(),
      }))
      .sort((left, right) => right.count - left.count || left.name.localeCompare(right.name, 'zh-Hans-CN'));

    // 分段独立计数：used 是「封面 → 本段已用次数」，prev 是本段上一张卡的封面。
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
      // 从 hash 决定的起点开始轮转：同样的"最少使用"里也保留各标签自己的偏好，不至于全挤在字典序第一个。
      const offset = hashString(entry.name) % entry.candidates.length;
      const rotated = entry.candidates.slice(offset).concat(entry.candidates.slice(0, offset));
      // 先剔除左邻已经用过的那张（相邻不重复优先于均匀），再在剩下的里挑用量最少的；
      // 只有该标签全部候选都等于左邻时才会重复。打平仍按轮转顺序，结果稳定。
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

  // 焦点归位（status §6 P2-11）：从详情回到列表时，把焦点还给触发那张卡。
  // 墙是虚拟化的，返回那一帧卡片可能还没挂载，所以在 rAF 里最多重试 8 帧；
  // 仍找不到（例如卡被筛掉/走远了）就退到画布容器——.canvas 有 tabIndex=0，
  // 方向键仍能继续浏览，不会把焦点丢给 body。
  // 只在「详情 → 列表」这一跳触发：进详情不该抢焦点，回标签总览也没有对应卡片。
  useEffect(() => {
    const cameFromDetail = routeViewRef.current === 'detail';
    routeViewRef.current = route.view;
    if (route.view === 'detail') {
      // 详情里换图（相似/前后）或深链进来时，以当前这张为准，避免留下上一次的旧 id。
      returnFocusId.current = route.id;
      return;
    }
    if (!cameFromDetail || (route.view !== 'wall' && route.view !== 'tag')) return;
    const id = returnFocusId.current;
    if (!id) return;
    let frame = 0;
    let raf = 0;
    const step = () => {
      // 不拼属性选择器：id 来自数据，逐张比对 dataset 更稳妥，也省掉转义问题。
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

  // 概要层关法：Esc、点卡片以外任何地方。叠层内部（按钮）的 pointerdown 不会冒到
  // window，所以点「打开详情」不会先把层关掉。
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

  // 双击 / Enter / 触屏单击：图片从卡片位置"长"到全屏（lib/expand.ts 的克隆飞行），
  // 详情页自己就是全屏，不再有浮在页面上面的灯箱。
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
            homeTitle: route.view === 'wall' && !filtered ? '回到起点（Home）' : '回到顶部（Home）',
          },
    [filtered, route.view],
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
            loading={!allLoaded && wall.hasMore}
            onOpenTag={handleOpenTag}
          />
        ) : (
          <>
            {route.view === 'tag' ? (
              <header className="wallhead">
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
