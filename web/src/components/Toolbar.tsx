import { useEffect, useMemo, useRef, useState } from 'react';
import { categoryLabel, formatCount, RATING_LABELS } from '../lib/format';
import type { TagFacet } from '../source/types';
import type { FilterState } from '../state/filters';

const MAX_TAG_ROWS = 240;
const RATINGS = ['G', 'S', 'Q', 'E'] as const;

export interface ToolbarProps {
  filters: FilterState;
  onChange: (next: FilterState) => void;
  onReset: () => void;
  tags: TagFacet[];
  groups: string[];
  loaded: number;
  total: number;
  shown: number;
  filterActive: boolean;
  zoomMode: boolean;
  scale: number;
  onToggleZoom: () => void;
  onFitScreen: () => void;
  onHome: () => void;
  showBack: boolean;
}

interface TagPickerProps {
  facets: TagFacet[];
  selected: string[];
  onToggle: (name: string) => void;
}

function TagPicker({ facets, selected, onToggle }: TagPickerProps) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const rootRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false);
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKeyDown);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKeyDown);
    };
  }, [open]);

  const needle = query.trim().toLowerCase();
  const matches = useMemo(() => {
    const filtered = needle
      ? facets.filter(
          (facet) =>
            facet.name.toLowerCase().includes(needle) ||
            facet.category.toLowerCase().includes(needle),
        )
      : facets;
    return filtered.slice(0, MAX_TAG_ROWS);
  }, [facets, needle]);

  return (
    <div className="tag-picker" ref={rootRef}>
      <button
        type="button"
        className={`chip-button${selected.length > 0 ? ' chip-button--on' : ''}`}
        aria-expanded={open}
        aria-haspopup="dialog"
        onClick={() => setOpen((value) => !value)}
      >
        标签{selected.length > 0 ? ` · ${selected.length}` : ''}
      </button>
      {open ? (
        <div className="tag-panel" role="dialog" aria-label="按标签筛选">
          <input
            type="search"
            className="tag-panel__search"
            placeholder="搜索标签…"
            value={query}
            autoFocus
            onChange={(event) => setQuery(event.target.value)}
          />
          <ul className="tag-panel__list">
            {matches.length === 0 ? (
              <li className="tag-panel__empty">
                {facets.length === 0 ? '这份数据没有标签 facet' : '没有匹配的标签'}
              </li>
            ) : (
              matches.map((facet) => {
                const on = selected.includes(facet.name);
                return (
                  <li key={facet.name}>
                    <button
                      type="button"
                      className={`tag-row${on ? ' tag-row--on' : ''}`}
                      onClick={() => onToggle(facet.name)}
                      aria-pressed={on}
                    >
                      <span className="tag-row__name">{facet.name}</span>
                      <span className={`tag-row__cat tag-row__cat--${facet.category}`}>
                        {categoryLabel(facet.category)}
                      </span>
                      <span className="tag-row__count">{formatCount(facet.count)}</span>
                    </button>
                  </li>
                );
              })
            )}
          </ul>
        </div>
      ) : null}
    </div>
  );
}

/** 顶部工具条：品牌/统计 + 筛选 + 缩放控制。 */
export function Toolbar({
  filters,
  onChange,
  onReset,
  tags,
  groups,
  loaded,
  total,
  shown,
  filterActive,
  zoomMode,
  scale,
  onToggleZoom,
  onFitScreen,
  onHome,
  showBack,
}: ToolbarProps) {
  const toggleTag = (name: string) => {
    const next = filters.tags.includes(name)
      ? filters.tags.filter((tag) => tag !== name)
      : [...filters.tags, name];
    onChange({ ...filters, tags: next });
  };

  return (
    <header className="toolbar">
      <div className="toolbar__row">
        <button type="button" className="brand" onClick={onHome} title="回到墙首屏">
          {showBack ? '← ' : ''}Kanalib 图片墙
        </button>
        <span className="toolbar__stats">
          已加载 {formatCount(loaded)} / 共 {formatCount(total)}
          {filterActive ? ` · 命中 ${formatCount(shown)}` : ''}
        </span>
        <div className="toolbar__spacer" />

        <label className="field field--search">
          <span className="field__label">文件名</span>
          <input
            type="search"
            className="field__input"
            placeholder="文件名子串…"
            value={filters.query}
            onChange={(event) => onChange({ ...filters, query: event.target.value })}
          />
        </label>

        <TagPicker facets={tags} selected={filters.tags} onToggle={toggleTag} />

        <button
          type="button"
          className={`chip-button${filters.onlyFav ? ' chip-button--on' : ''}`}
          aria-pressed={filters.onlyFav}
          onClick={() => onChange({ ...filters, onlyFav: !filters.onlyFav })}
        >
          ♥ 收藏
        </button>

        <label className="field">
          <span className="field__label">分级</span>
          <select
            className="field__input"
            value={filters.ratings[0] ?? ''}
            onChange={(event) =>
              onChange({
                ...filters,
                ratings: event.target.value ? [event.target.value] : [],
              })
            }
          >
            <option value="">全部</option>
            {RATINGS.map((rating) => (
              <option key={rating} value={rating}>
                {RATING_LABELS[rating] ?? rating}
              </option>
            ))}
          </select>
        </label>

        <label className="field">
          <span className="field__label">星级</span>
          <select
            className="field__input"
            value={String(filters.minStars)}
            onChange={(event) => onChange({ ...filters, minStars: Number(event.target.value) })}
          >
            <option value="0">不限</option>
            {[1, 2, 3, 4, 5].map((stars) => (
              <option key={stars} value={String(stars)}>
                {'★'.repeat(stars)} 以上
              </option>
            ))}
          </select>
        </label>

        <label className="field">
          <span className="field__label">分组</span>
          <select
            className="field__input"
            value={filters.group ?? ''}
            onChange={(event) =>
              onChange({ ...filters, group: event.target.value || null })
            }
          >
            <option value="">全部</option>
            {groups.map((group) => (
              <option key={group} value={group}>
                {group}
              </option>
            ))}
          </select>
        </label>

        {filterActive ? (
          <button type="button" className="chip-button" onClick={onReset}>
            重置筛选
          </button>
        ) : null}

        <div className="toolbar__divider" />

        <button
          type="button"
          className={`chip-button${zoomMode ? ' chip-button--on' : ''}`}
          aria-pressed={zoomMode}
          onClick={onToggleZoom}
          title="整墙缩放（快捷键 W）：滚轮缩放、拖拽平移"
        >
          缩放模式{zoomMode ? ` · ${Math.round(scale * 100)}%` : ''}
        </button>
        <button type="button" className="chip-button" onClick={onFitScreen} title="复位缩放并回到普通滚动">
          适应屏幕
        </button>
      </div>

      {filters.tags.length > 0 ? (
        <div className="toolbar__chips">
          <span className="toolbar__chips-label">标签：</span>
          {filters.tags.map((tag) => (
            <button
              key={tag}
              type="button"
              className="tag-chip"
              onClick={() => toggleTag(tag)}
              aria-label={`移除标签 ${tag}`}
            >
              {tag} ×
            </button>
          ))}
        </div>
      ) : null}
    </header>
  );
}
