import { useEffect, useRef, useState, type ReactNode } from 'react';
import { formatCount } from '../lib/format';
import './sidebar.css';
import { Inspector, type InspectorProps } from './Inspector';
import { EMPTY_FILTERS, type FilterState } from '../state/filters';
import type { WallItem } from '../source/types';

export type SortMode = 'random' | 'time' | 'name' | 'stars';

type PanelId = 'search' | 'sort' | 'about';

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

/** 单色预览：左半实心的圆。 */
const ICON_MONO = (
  <Icon>
    <circle cx="12" cy="12" r="8.5" />
    <path d="M12 3.5a8.5 8.5 0 0 1 0 17z" fill="currentColor" stroke="none" />
  </Icon>
);

const ICON_MINUS = (
  <Icon>
    <path d="M5 12h14" />
  </Icon>
);

const ICON_PLUS = (
  <Icon>
    <path d="M12 5v14" />
    <path d="M5 12h14" />
  </Icon>
);

/** 缩放复位：四角括号=“铺回可视区”。 */
const ICON_FIT = (
  <Icon>
    <path d="M4 9V5.5A1.5 1.5 0 0 1 5.5 4H9" />
    <path d="M20 9V5.5A1.5 1.5 0 0 0 18.5 4H15" />
    <path d="M4 15v3.5A1.5 1.5 0 0 0 5.5 20H9" />
    <path d="M20 15v3.5A1.5 1.5 0 0 1 18.5 20H15" />
  </Icon>
);

/** 回到起点：准星。无限空间回格子原点，有限世界回顶部。 */
const ICON_CROSSHAIR = (
  <Icon>
    <circle cx="12" cy="12" r="7.5" />
    <path d="M12 2.5v4" />
    <path d="M12 17.5v4" />
    <path d="M2.5 12h4" />
    <path d="M17.5 12h4" />
    <circle cx="12" cy="12" r="0.8" fill="currentColor" />
  </Icon>
);

const ICON_BACK = (
  <Icon>
    <path d="M19 12H5" />
    <path d="m11 18-6-6 6-6" />
  </Icon>
);

const ICON_PREV = (
  <Icon>
    <path d="m15 5-7 7 7 7" />
  </Icon>
);

const ICON_NEXT = (
  <Icon>
    <path d="m9 5 7 7-7 7" />
  </Icon>
);

const ICON_PLAY = (
  <Icon>
    <path d="M8 5.5v13l11-6.5z" fill="currentColor" stroke="none" />
  </Icon>
);

const ICON_PAUSE = (
  <Icon>
    <path d="M9 5v14" />
    <path d="M15 5v14" />
  </Icon>
);

/** 留白视图：一张画框。 */
const ICON_MATTE = (
  <Icon>
    <rect x="3.5" y="3.5" width="17" height="17" rx="1.5" />
    <rect x="7" y="7" width="10" height="10" rx="0.8" />
  </Icon>
);

export type RailView =
  | 'home'
  | 'tags'
  | 'tag'
  | 'favs'
  | 'fav'
  | 'ratings'
  | 'rating'
  | 'groups'
  | 'group';

/** 视图操作（墙页）：缩放与回到起点。离开墙页传 null，整组不渲染。 */
export interface CanvasOps {
  onZoomIn: () => void;
  onZoomOut: () => void;
  onZoomReset: () => void;
  onHome: () => void;
  /** 按钮语义随世界形态变：无限是“回到起点”，有限是“回到顶部”。 */
  homeTitle: string;
}

/** 详情操作（详情页）：返回 / 翻页 / 序号。离开详情传 null。 */
export interface DetailOps {
  index: number;
  total: number;
  hasPrev: boolean;
  hasNext: boolean;
  playing: boolean;
  matte: boolean;
  onBack: () => void;
  onPrev: () => void;
  onNext: () => void;
  onTogglePlay: () => void;
  onToggleMatte: () => void;
}

export interface SidebarProps {
  view: RailView;
  onHome: () => void;
  onTags: () => void;
  onFavs: () => void;
  onRatings: () => void;
  onGroups: () => void;
  filters: FilterState;
  onFilters: (next: FilterState) => void;
  loaded: number;
  total: number;
  shown: number;
  filtered: boolean;
  sort: SortMode;
  onSort: (mode: SortMode) => void;
  focusItem?: WallItem | null;
  canvas?: CanvasOps | null;
  detail?: DetailOps | null;
  /** 当前聚焦/详情图的信息检查器；null 时不渲染。 */
  inspector?: InspectorProps | null;
  /** 详情页信息栏是否收起（undefined = 不显示信息开关）。 */
  detailInfoHidden?: boolean;
  onToggleInfo?: () => void;
  /** 单色预览：只给图片去色，强调色与 UI 保持原样。 */
  mono: boolean;
  onToggleMono: () => void;
}

/**
 * 左侧的"缝"：120px 黑白竖排页签。纸面常驻 76% 半透明 + blur（Q6=C′），
 * 悬停只加深模糊、不改变底色。标签/收藏/分级/分组是四个入口页，
 * 搜索/排序/关于是浮层面板（Q8）。
 */
export function Sidebar({
  view,
  onHome,
  onTags,
  onFavs,
  onRatings,
  onGroups,
  filters,
  onFilters,
  loaded,
  total,
  shown,
  filtered,
  sort,
  onSort,
  focusItem,
  canvas,
  detail,
  inspector,
  detailInfoHidden,
  onToggleInfo,
  mono,
  onToggleMono,
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
          <button
            type="button"
            className={'seam__tab' + (view === 'favs' || view === 'fav' ? ' is-on' : '')}
            onClick={onFavs}
            title="收藏"
            aria-current={view === 'favs' || view === 'fav' ? 'page' : undefined}
          >
            {ICON_HEART}
            <span className="seam__label">收藏</span>
          </button>
          <button
            type="button"
            className={'seam__tab' + (view === 'ratings' || view === 'rating' ? ' is-on' : '')}
            onClick={onRatings}
            title="分级"
            aria-current={view === 'ratings' || view === 'rating' ? 'page' : undefined}
          >
            {ICON_SHIELD}
            <span className="seam__label">分级</span>
          </button>
          <button
            type="button"
            className={'seam__tab' + (view === 'groups' || view === 'group' ? ' is-on' : '')}
            onClick={onGroups}
            title="分组"
            aria-current={view === 'groups' || view === 'group' ? 'page' : undefined}
          >
            {ICON_FOLDER}
            <span className="seam__label">分组</span>
          </button>

          <button
            type="button"
            className={'seam__tab' + (panel === 'search' || filters.query.trim() ? ' is-on' : '')}
            id="seam-panel-trigger-search"
            aria-expanded={panel === 'search'}
            aria-controls={panel === 'search' ? 'seam-panel' : undefined}
            onClick={(event) => openPanel('search', event.currentTarget)}
            title="搜索"
          >
            {ICON_SEARCH}
            <span className="seam__label">搜索</span>
          </button>
          <button
            type="button"
            className={'seam__tab' + (panel === 'sort' || sort !== 'random' ? ' is-on' : '')}
            id="seam-panel-trigger-sort"
            aria-expanded={panel === 'sort'}
            aria-controls={panel === 'sort' ? 'seam-panel' : undefined}
            onClick={(event) => openPanel('sort', event.currentTarget)}
            title="排序"
          >
            {ICON_SORT}
            <span className="seam__label">排序</span>
          </button>
        </div>

        {/* 功能键统一收在缝里（2026-11-09 用户指令）：画布与详情页不再挂任何操作钮。
            墙页给缩放/回到起点，详情页给返回/翻页——两组互斥，高度不叠加。 */}
        {canvas ? (
          <div className="seam__ops" role="group" aria-label="视图操作">
            <button type="button" className="seam__tab seam__tab--icon" onClick={canvas.onZoomOut} title="缩小">
              {ICON_MINUS}
            </button>
            <button type="button" className="seam__tab seam__tab--icon" onClick={canvas.onZoomIn} title="放大">
              {ICON_PLUS}
            </button>
            <button type="button" className="seam__tab seam__tab--icon" onClick={canvas.onZoomReset} title="缩放复位（100%）">
              {ICON_FIT}
            </button>
            <button type="button" className="seam__tab seam__tab--icon" onClick={canvas.onHome} title={canvas.homeTitle}>
              {ICON_CROSSHAIR}
            </button>
          </div>
        ) : null}

        {detail ? (
          <div className="seam__ops seam__ops--detail" role="group" aria-label="详情操作">
            <button type="button" className="seam__tab seam__tab--icon" onClick={detail.onBack} title="返回（Esc）">
              {ICON_BACK}
            </button>
            <button
              type="button"
              className="seam__tab seam__tab--icon"
              onClick={detail.onPrev}
              disabled={!detail.hasPrev}
              title="上一张（←）"
            >
              {ICON_PREV}
            </button>
            <button
              type="button"
              className="seam__tab seam__tab--icon"
              onClick={detail.onNext}
              disabled={!detail.hasNext}
              title="下一张（→）"
            >
              {ICON_NEXT}
            </button>
            {detailInfoHidden !== undefined && onToggleInfo ? (
              <button
                type="button"
                className={'seam__tab seam__tab--icon' + (detailInfoHidden ? '' : ' is-on')}
                onClick={onToggleInfo}
                title={detailInfoHidden ? '显示信息（I）' : '收起信息（I）'}
              >
                {ICON_INFO}
              </button>
            ) : null}
            <button
              type="button"
              className={'seam__tab seam__tab--icon' + (detail.playing ? ' is-on' : '')}
              onClick={detail.onTogglePlay}
              title={detail.playing ? '暂停放映（P）' : '自动放映（P）'}
              aria-pressed={detail.playing}
            >
              {detail.playing ? ICON_PAUSE : ICON_PLAY}
            </button>
            <button
              type="button"
              className={'seam__tab seam__tab--icon' + (detail.matte ? ' is-on' : '')}
              onClick={detail.onToggleMatte}
              title={detail.matte ? '沉浸视图（M）' : '留白视图（M）'}
              aria-pressed={detail.matte}
            >
              {ICON_MATTE}
            </button>
            <span className="seam__count">
              {String(detail.index + 1).padStart(2, '0')} / {formatCount(detail.total)}
            </span>
          </div>
        ) : null}

        <div className="seam__foot">
          <span className="seam__count">
            {formatCount(loaded)} / {formatCount(total)}
          </span>
          <div className="seam__foot-actions">
            <button
              type="button"
              className={'seam__tab seam__tab--icon' + (mono ? ' is-on' : '')}
              onClick={onToggleMono}
              aria-pressed={mono}
              title={mono ? '恢复彩色（N）' : '单色预览（N）'}
            >
              {ICON_MONO}
            </button>
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
              <dl className="panel__keys">
                <dt>单击 / 双击</dt>
                <dd>聚焦卡片 / 打开详情</dd>
                <dt>拖拽 · 滚轮</dt>
                <dd>平移画布</dd>
                <dt>Ctrl + 滚轮</dt>
                <dd>以光标为中心缩放</dd>
                <dt>← → · Esc</dt>
                <dd>详情中换图 / 返回</dd>
                <dt>I · F · P</dt>
                <dd>收起信息 · 全屏 · 放映</dd>
                <dt>N · M</dt>
                <dd>单色预览 · 留白视图</dd>
                <dt>Home · + −</dt>
                <dd>回到起点 · 缩放</dd>
              </dl>
              <div className="panel__actions">
                <button type="button" className="ghost" onClick={() => onFilters({ ...EMPTY_FILTERS })}>
                  清除全部筛选
                </button>
              </div>
            </>
          ) : null}
        </div>
      ) : null}

      {inspector ? <Inspector key={inspector.item.id} {...inspector} /> : null}
    </>
  );
}
