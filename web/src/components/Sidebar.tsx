import { useEffect, useRef, useState, type ReactNode } from 'react';
import { formatCount } from '../lib/format';
import './sidebar.css';
import { EMPTY_FILTERS, type FilterState } from '../state/filters';
import type { WallItem } from '../source/types';

export type SortMode = 'random' | 'time' | 'name' | 'stars';

type PanelId = 'search' | 'favorites' | 'ratings' | 'groups' | 'sort' | 'about';

const RATING_ORDER = ['G', 'S', 'Q', 'E'];

function Icon({ children, size = 15 }: { children: ReactNode; size?: number }) {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.5}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {children}
    </svg>
  );
}

const ICON_HOME = (
  <Icon>
    <path d="M4 10.5 12 3.5l8 7" />
    <path d="M6 9.8V20h5v-5.5h2V20h5V9.8" />
  </Icon>
);

const ICON_TAG = (
  <Icon>
    <path d="M12.6 2.6A2 2 0 0 0 11.2 2H4a2 2 0 0 0-2 2v7.2a2 2 0 0 0 .6 1.4l8.7 8.7a2.4 2.4 0 0 0 3.4 0l6.6-6.6a2.4 2.4 0 0 0 0-3.4z" />
    <circle cx="7.5" cy="7.5" r=".6" fill="currentColor" />
  </Icon>
);

const ICON_SEARCH = (
  <Icon>
    <circle cx="11" cy="11" r="7.5" />
    <path d="m21 21-4.6-4.6" />
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

const ICON_INFO = (
  <Icon>
    <circle cx="12" cy="12" r="9.5" />
    <path d="M12 16.5v-4.5" />
    <path d="M12 7.8h.01" />
  </Icon>
);

interface PanelEntry {
  id: PanelId;
  label: string;
  icon: ReactNode;
}

const PANELS: PanelEntry[] = [
  { id: 'search', label: '搜索', icon: ICON_SEARCH },
  { id: 'favorites', label: '收藏', icon: ICON_HEART },
  { id: 'ratings', label: '分级', icon: ICON_SHIELD },
  { id: 'groups', label: '分组', icon: ICON_FOLDER },
  { id: 'sort', label: '排序', icon: ICON_SORT },
];

export type RailView = 'home' | 'tags' | 'tag';

export interface SidebarProps {
  view: RailView;
  onHome: () => void;
  onTags: () => void;
  filters: FilterState;
  onFilters: (next: FilterState) => void;
  groups: string[];
  loaded: number;
  total: number;
  shown: number;
  filtered: boolean;
  sort: SortMode;
  onSort: (mode: SortMode) => void;
  focusItem?: WallItem | null;
}

/**
 * 左侧的"缝"：120px 黑白竖排页签。默认是实色纸面 + 大投影，
 * 鼠标进入时切成薄玻璃，让底下的墙透出来一点。
 */
export function Sidebar({
  view,
  onHome,
  onTags,
  filters,
  onFilters,
  groups,
  loaded,
  total,
  shown,
  filtered,
  sort,
  onSort,
  focusItem,
}: SidebarProps) {
  const [panel, setPanel] = useState<PanelId | null>(null);
  const panelRef = useRef<HTMLDivElement | null>(null);
  /** 打开面板的按钮：关闭后把焦点还回去（P2-2）。 */
  const panelTriggerRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    if (!panel) return;
    const onPointerDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (panelRef.current && target && panelRef.current.contains(target)) return;
      if (target instanceof Element && target.closest('.seam')) return;
      setPanel(null);
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

  useEffect(() => {
    setPanel(null);
  }, [view]);

  // P2-2：role="dialog" 要配焦点管理——打开时把焦点送进面板（搜索框自己 autoFocus），
  // 关闭/切换后还给触发按钮，而不是只挂一个 aria-expanded 就完事。
  useEffect(() => {
    if (!panel) return;
    const node = panelRef.current;
    if (node && panel !== 'search') {
      const first = node.querySelector<HTMLElement>(
        'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])',
      );
      first?.focus();
    }
    return () => {
      const trigger = panelTriggerRef.current;
      panelTriggerRef.current = null;
      trigger?.focus();
    };
  }, [panel]);

  const setRating = (rating: string | null) => {
    onFilters({ ...filters, ratings: rating ? [rating] : [] });
  };

  const openPanel = (id: PanelId, trigger: HTMLElement | null = null) => {
    if (panel === id) {
      setPanel(null);
      return;
    }
    panelTriggerRef.current = trigger;
    setPanel(id);
  };

  return (
    <>
      <nav className="seam" aria-label="图片墙导航">
        <span className="seam__dot" />
        <span className="seam__word" aria-hidden="true">
          Kanalib
        </span>

        <div className="seam__nav">
          <button
            type="button"
            className={'seam__tab' + (view === 'home' ? ' is-on' : '')}
            onClick={onHome}
            title="首页"
            aria-current={view === 'home' ? 'page' : undefined}
          >
            {ICON_HOME}
            <span className="seam__label">首页</span>
          </button>
          <button
            type="button"
            className={'seam__tab' + (view === 'tags' || view === 'tag' ? ' is-on' : '')}
            onClick={onTags}
            title="标签"
            aria-current={view === 'tags' || view === 'tag' ? 'page' : undefined}
          >
            {ICON_TAG}
            <span className="seam__label">标签</span>
          </button>

          {PANELS.map((entry) => (
            <button
              key={entry.id}
              type="button"
              className={
                'seam__tab' +
                (panel === entry.id ? ' is-on' : '') +
                (entry.id === 'favorites' && filters.onlyFav ? ' is-on' : '')
              }
              id={'seam-panel-trigger-' + entry.id}
              aria-expanded={panel === entry.id}
              aria-controls={panel === entry.id ? 'seam-panel' : undefined}
              onClick={(event) => openPanel(entry.id, event.currentTarget)}
              title={entry.label}
            >
              {entry.icon}
              <span className="seam__label">{entry.label}</span>
            </button>
          ))}
        </div>

        <div className="seam__foot">
          <span className="seam__count">
            {formatCount(loaded)} / {formatCount(total)}
          </span>
          <button
            type="button"
            className={'seam__tab seam__tab--icon' + (panel === 'about' ? ' is-on' : '')}
            id="seam-panel-trigger-about"
            aria-expanded={panel === 'about'}
            aria-controls={panel === 'about' ? 'seam-panel' : undefined}
            onClick={(event) => openPanel('about', event.currentTarget)}
            title="关于"
          >
            {ICON_INFO}
          </button>
        </div>
      </nav>

      {panel ? (
        <div
          className="panel"
          id="seam-panel"
          ref={panelRef}
          role="dialog"
          aria-modal="false"
          aria-labelledby={'seam-panel-trigger-' + panel}
        >
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
                <button type="button" className="ghost" onClick={() => onFilters({ ...filters, query: '' })}>
                  清空
                </button>
              </div>
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
                    className={'list__row' + (filters.ratings.includes(rating) ? ' is-on' : '')}
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
                    ['random', '随机（每次打开重排）'],
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

          {panel === 'about' ? (
            <>
              <p className="panel__title">关于</p>
              <p className="panel__hint" style={{ marginTop: 0 }}>
                只读展示墙：图片在工作台的本地库里整理，导出成静态站点后在这里浏览。
              </p>
              <p className="panel__hint">
                已加载 {formatCount(loaded)} / {formatCount(total)} 张
                {filtered ? '，筛选后 ' + formatCount(shown) + ' 张' : ''}。
              </p>
              {focusItem ? (
                <p className="panel__hint">
                  最近看的一张：{focusItem.n}（{Math.round(focusItem.w)} × {Math.round(focusItem.h)}）
                </p>
              ) : null}
              <div className="panel__actions">
                <button type="button" className="ghost" onClick={() => onFilters({ ...EMPTY_FILTERS })}>
                  清除全部筛选
                </button>
              </div>
            </>
          ) : null}
        </div>
      ) : null}
    </>
  );
}
