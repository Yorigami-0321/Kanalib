import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { formatCount } from '../lib/format';
import type { FilterState } from '../state/filters';
import type { TagFacet } from '../source/types';

/** 墙面排序：默认按发布时间倒序（发布器已按 Modified 降序分片）。 */
export type SortMode = 'time' | 'name' | 'stars';

type PanelId = 'search' | 'tags' | 'favorites' | 'ratings' | 'groups' | 'sort' | 'zoom' | 'about';

const RATING_ORDER = ['G', 'S', 'Q', 'E'];

function Icon({ children, size = 16 }: { children: ReactNode; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  );
}

const ICON_SEARCH = (
  <Icon>
    <circle cx="11" cy="11" r="7.5" />
    <path d="m21 21-4.6-4.6" />
  </Icon>
);

const ICON_TAG = (
  <Icon>
    <path d="M12.6 2.6A2 2 0 0 0 11.2 2H4a2 2 0 0 0-2 2v7.2a2 2 0 0 0 .6 1.4l8.7 8.7a2.4 2.4 0 0 0 3.4 0l6.6-6.6a2.4 2.4 0 0 0 0-3.4z" />
    <circle cx="7.5" cy="7.5" r=".6" fill="currentColor" />
  </Icon>
);

const ICON_HEART = (
  <Icon>
    <path d="M19 14c1.5-1.5 3-3.2 3-5.5A5.5 5.5 0 0 0 16.5 3c-1.8 0-3 .5-4.5 2-1.5-1.5-2.7-2-4.5-2A5.5 5.5 0 0 0 2 8.5c0 2.3 1.5 4 3 5.5l7 7z" />
  </Icon>
);

const ICON_SHIELD = (
  <Icon>
    <path d="M20 13c0 5-3.5 7.5-7.7 9a1 1 0 0 1-.6 0C7.5 20.5 4 18 4 13V6a1 1 0 0 1 1-1c2 0 4.5-1.2 6.2-2.7a1.2 1.2 0 0 1 1.6 0C14.5 3.8 17 5 19 5a1 1 0 0 1 1 1z" />
  </Icon>
);

const ICON_FOLDER = (
  <Icon>
    <path d="M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.7-.9L9.6 3.9A2 2 0 0 0 7.9 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2z" />
  </Icon>
);

const ICON_SORT = (
  <Icon>
    <path d="m21 16-4 4-4-4" />
    <path d="M17 20V4" />
    <path d="m3 8 4-4 4 4" />
    <path d="M7 4v16" />
  </Icon>
);

const ICON_ZOOM = (
  <Icon>
    <path d="M8 3H5a2 2 0 0 0-2 2v3" />
    <path d="M21 8V5a2 2 0 0 0-2-2h-3" />
    <path d="M3 16v3a2 2 0 0 0 2 2h3" />
    <path d="M16 21h3a2 2 0 0 0 2-2v-3" />
  </Icon>
);

const ICON_INFO = (
  <Icon>
    <circle cx="12" cy="12" r="9.5" />
    <path d="M12 16.5v-4.5" />
    <path d="M12 7.8h.01" />
  </Icon>
);

const ICON_CHEVRON = (
  <Icon>
    <path d="m9 18 6-6-6-6" />
  </Icon>
);

interface NavEntry {
  id: PanelId;
  label: string;
  icon: ReactNode;
}

const NAV: NavEntry[] = [
  { id: 'search', label: '搜索', icon: ICON_SEARCH },
  { id: 'tags', label: '标签', icon: ICON_TAG },
  { id: 'favorites', label: '收藏', icon: ICON_HEART },
  { id: 'ratings', label: '分级', icon: ICON_SHIELD },
  { id: 'groups', label: '分组', icon: ICON_FOLDER },
  { id: 'sort', label: '排序', icon: ICON_SORT },
  { id: 'zoom', label: '缩放', icon: ICON_ZOOM },
];

export interface SidebarProps {
  filters: FilterState;
  onFilters: (next: FilterState) => void;
  facetTags: TagFacet[];
  groups: string[];
  loaded: number;
  total: number;
  shown: number;
  filtered: boolean;
  sort: SortMode;
  onSort: (mode: SortMode) => void;
  expanded: boolean;
  onExpandedChange: (next: boolean) => void;
  zoomMode: boolean;
  scale: number;
  onToggleZoom: () => void;
  onFitScreen: () => void;
  onHome: () => void;
}

function matchesNeedle(needle: string, text: string): boolean {
  return needle.length === 0 || text.toLowerCase().includes(needle);
}

/**
 * 左侧竖向边栏：唯一的导航与筛选入口（取代 v1 的顶部工具条）。
 * 收起时只留图标，展开后浮层面板给出具体选项；筛选状态立即作用于墙。
 */
export function Sidebar({
  filters,
  onFilters,
  facetTags,
  groups,
  loaded,
  total,
  shown,
  filtered,
  sort,
  onSort,
  expanded,
  onExpandedChange,
  zoomMode,
  scale,
  onToggleZoom,
  onFitScreen,
  onHome,
}: SidebarProps) {
  const [panel, setPanel] = useState<PanelId | null>(null);
  const [tagNeedle, setTagNeedle] = useState('');
  const panelRef = useRef<HTMLDivElement | null>(null);

  const closePanel = () => setPanel(null);

  useEffect(() => {
    if (!panel) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (panelRef.current && target && panelRef.current.contains(target)) return;
      if (target instanceof Element && target.closest('.rail')) return;
      closePanel();
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setPanel(null);
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [panel]);

  const tagList = useMemo(() => {
    const needle = tagNeedle.trim().toLowerCase();
    return facetTags.filter((tag) => matchesNeedle(needle, tag.name)).slice(0, 200);
  }, [facetTags, tagNeedle]);

  const toggleTag = (name: string) => {
    const next = filters.tags.includes(name)
      ? filters.tags.filter((tag) => tag !== name)
      : [...filters.tags, name];
    onFilters({ ...filters, tags: next });
  };

  const setRating = (rating: string | null) => {
    onFilters({ ...filters, ratings: rating ? [rating] : [] });
  };

  const activeCount =
    (filters.query.trim() ? 1 : 0) +
    (filters.tags.length > 0 ? 1 : 0) +
    (filters.onlyFav ? 1 : 0) +
    (filters.ratings.length > 0 ? 1 : 0) +
    (filters.group !== null ? 1 : 0);

  const openPanel = (id: PanelId) => {
    setPanel((current) => (current === id ? null : id));
    // 窄屏上不展开边栏：面板本身就要占掉大半屏宽，展开反而没有位置。
    const narrow = typeof window !== 'undefined' && window.matchMedia('(max-width: 760px)').matches;
    if (!expanded && !narrow) onExpandedChange(true);
  };

  return (
    <>
      <nav className="rail" aria-label="图片墙导航">
        <button type="button" className="rail__brand" onClick={onHome} title="回到全部图片">
          <span className="rail__dot" />
          <span className="rail__word">
            <span className="rail__name">Kanalib</span>
            <span className="rail__sub">图片墙</span>
          </span>
        </button>

        <div className="rail__nav">
          {NAV.map((entry) => (
            <button
              key={entry.id}
              type="button"
              className={
                'rail__item' +
                (panel === entry.id ? ' is-on' : '') +
                (entry.id === 'favorites' && filters.onlyFav ? ' is-on' : '')
              }
              aria-expanded={panel === entry.id}
              onClick={() => openPanel(entry.id)}
              title={entry.label}
            >
              {entry.icon}
              <span className="rail__label">{entry.label}</span>
            </button>
          ))}
        </div>

        <p className="rail__count">
          {formatCount(loaded)} / {formatCount(total)}
          {filtered ? ' · 筛后 ' + formatCount(shown) : ''}
        </p>

        <div className="rail__foot">
          <button
            type="button"
            className="rail__item"
            onClick={() => openPanel('about')}
            title="关于"
          >
            {ICON_INFO}
            <span className="rail__label">关于</span>
          </button>
          <button
            type="button"
            className="rail__item"
            onClick={() => onExpandedChange(!expanded)}
            aria-expanded={expanded}
            title={expanded ? '收起边栏' : '展开边栏'}
          >
            <span style={{ display: 'inline-flex', transform: expanded ? 'rotate(180deg)' : 'none' }}>
              {ICON_CHEVRON}
            </span>
            <span className="rail__label">收起</span>
          </button>
        </div>
      </nav>

      {panel ? (
        <div className="panel" ref={panelRef} role="dialog" aria-label="筛选">
          {panel === 'search' ? (
            <>
              <p className="panel__title">搜索</p>
              <label className="field">
                {ICON_SEARCH}
                <input
                  value={filters.query}
                  placeholder="按文件名搜索"
                  autoFocus
                  onChange={(event) => onFilters({ ...filters, query: event.target.value })}
                />
              </label>
              <p className="panel__hint">
                {filters.query.trim() ? '已按文件名过滤' : '输入文件名的一部分即可过滤'}
              </p>
              <div className="panel__actions">
                <button
                  type="button"
                  className="ghost"
                  onClick={() => onFilters({ ...filters, query: '' })}
                >
                  清空
                </button>
              </div>
            </>
          ) : null}

          {panel === 'tags' ? (
            <>
              <p className="panel__title">标签 · {filters.tags.length} 项已选</p>
              <label className="field">
                {ICON_SEARCH}
                <input
                  value={tagNeedle}
                  placeholder="过滤标签"
                  onChange={(event) => setTagNeedle(event.target.value)}
                />
              </label>
              <div className="list">
                {tagList.map((tag) => {
                  const on = filters.tags.includes(tag.name);
                  return (
                    <button
                      key={tag.name + '|' + tag.category}
                      type="button"
                      className={'list__row' + (on ? ' is-on' : '')}
                      onClick={() => toggleTag(tag.name)}
                    >
                      <span className="mark" />
                      <span className="list__text">{tag.name}</span>
                      <span className="list__num">{formatCount(tag.count)}</span>
                    </button>
                  );
                })}
                {tagList.length === 0 ? <p className="panel__hint">没有匹配的标签</p> : null}
              </div>
              {filters.tags.length > 0 ? (
                <div className="panel__actions">
                  <button
                    type="button"
                    className="ghost"
                    onClick={() => onFilters({ ...filters, tags: [] })}
                  >
                    清空标签
                  </button>
                </div>
              ) : null}
            </>
          ) : null}

          {panel === 'favorites' ? (
            <>
              <p className="panel__title">收藏</p>
              <button
                type="button"
                className={'list__row' + (filters.onlyFav ? ' is-on' : '')}
                onClick={() => onFilters({ ...filters, onlyFav: !filters.onlyFav })}
              >
                <span className="mark" />
                <span className="list__text">只看收藏</span>
              </button>
              <p className="panel__hint">收藏状态来自工作台，墙上只读。</p>
            </>
          ) : null}

          {panel === 'ratings' ? (
            <>
              <p className="panel__title">分级</p>
              <div className="list">
                <button
                  type="button"
                  className={'list__row' + (filters.ratings.length === 0 ? ' is-on' : '')}
                  onClick={() => setRating(null)}
                >
                  <span className="mark" />
                  <span className="list__text">全部分级</span>
                </button>
                {RATING_ORDER.map((rating) => (
                  <button
                    key={rating}
                    type="button"
                    className={
                      'list__row' + (filters.ratings.includes(rating) ? ' is-on' : '')
                    }
                    onClick={() => setRating(rating)}
                  >
                    <span className="mark" />
                    <span className="list__text">{rating}</span>
                  </button>
                ))}
              </div>
            </>
          ) : null}

          {panel === 'groups' ? (
            <>
              <p className="panel__title">分组</p>
              <div className="list">
                <button
                  type="button"
                  className={'list__row' + (filters.group === null ? ' is-on' : '')}
                  onClick={() => onFilters({ ...filters, group: null })}
                >
                  <span className="mark" />
                  <span className="list__text">全部分组</span>
                </button>
                {groups.map((group) => (
                  <button
                    key={group}
                    type="button"
                    className={'list__row' + (filters.group === group ? ' is-on' : '')}
                    onClick={() => onFilters({ ...filters, group })}
                  >
                    <span className="mark" />
                    <span className="list__text">{group}</span>
                  </button>
                ))}
              </div>
            </>
          ) : null}

          {panel === 'sort' ? (
            <>
              <p className="panel__title">排序</p>
              <div className="list">
                {(
                  [
                    ['time', '发布时间（新→旧）'],
                    ['name', '文件名'],
                    ['stars', '星级（高→低）'],
                  ] as [SortMode, string][]
                ).map(([mode, label]) => (
                  <button
                    key={mode}
                    type="button"
                    className={'list__row' + (sort === mode ? ' is-on' : '')}
                    onClick={() => onSort(mode)}
                  >
                    <span className="mark" />
                    <span className="list__text">{label}</span>
                  </button>
                ))}
              </div>
            </>
          ) : null}

          {panel === 'zoom' ? (
            <>
              <p className="panel__title">缩放</p>
              <div className="list">
                <button
                  type="button"
                  className={'list__row' + (zoomMode ? ' is-on' : '')}
                  onClick={onToggleZoom}
                >
                  <span className="mark" />
                  <span className="list__text">{zoomMode ? '退出缩放' : '进入缩放'}</span>
                  <span className="list__num">{Math.round(scale * 100)}%</span>
                </button>
                <button type="button" className="list__row" onClick={onFitScreen}>
                  <span className="mark" />
                  <span className="list__text">适应屏幕</span>
                </button>
              </div>
              <p className="panel__hint">缩放模式下滚轮缩放、拖拽平移；Esc 退出。</p>
            </>
          ) : null}

          {panel === 'about' ? (
            <>
              <p className="panel__title">关于</p>
              <p className="panel__hint" style={{ marginTop: 0 }}>
                这是 Kanalib 的只读展示墙：图片在工作台的本地库里整理，导出成静态站点后在这里浏览。
              </p>
              <p className="panel__hint">
                已加载 {formatCount(loaded)} / {formatCount(total)} 张{filtered ? '，筛选后 ' + formatCount(shown) + ' 张' : ''}。
              </p>
              {activeCount > 0 ? (
                <div className="panel__actions">
                  <button type="button" className="ghost" onClick={() => onFilters({ ...EMPTY })}>
                    清除全部筛选
                  </button>
                </div>
              ) : null}
            </>
          ) : null}
        </div>
      ) : null}
    </>
  );
}

/** 全空的筛选项（组件内部清理用）。 */
const EMPTY: FilterState = {
  query: '',
  tags: [],
  onlyFav: false,
  ratings: [],
  minStars: 0,
  group: null,
};
