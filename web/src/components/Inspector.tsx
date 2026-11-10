import { useState } from 'react';
import { originalUrl, tierUrl } from '../lib/assetUrl';
import { categoryLabel, formatCount, formatDateTime, formatSize, formatStars, ratingLabel } from '../lib/format';
import { groupTags, similarOf } from '../lib/itemDetail';
import type { TierPaths, WallItem } from '../source/types';
import './inspector.css';

const COLLAPSED_TAGS = 18;
const COLLAPSED_PER_GROUP = 6;

export interface InspectorProps {
  item: WallItem;
  items: readonly WallItem[];
  paths: TierPaths;
  mode: 'spotlight' | 'detail';
  onClose: () => void;
  onOpenDetail: (id: string) => void;
  onBack?: () => void;
  onTagClick: (tag: string) => void;
  onSimilar: (id: string) => void;
}

/**
 * 左侧信息检查器（v4）：聚焦卡和详情页共用。
 * 详情页的标题/元数据/标签/相似条全部在这里，画面本身不再挂底部信息层。
 */
export function Inspector({
  item,
  items,
  paths,
  mode,
  onClose,
  onOpenDetail,
  onBack,
  onTagClick,
  onSimilar,
}: InspectorProps) {
  const [expanded, setExpanded] = useState(false);
  const groups = groupTags(item);
  const similar = similarOf(item, items);
  const original = originalUrl(paths, item);
  const totalTags = item.tags.length;

  let remaining = COLLAPSED_TAGS;
  const collapsedGroups = groups
    .map((group) => {
      const take = Math.min(COLLAPSED_PER_GROUP, remaining, group.tags.length);
      remaining -= take;
      return {
        ...group,
        tags: group.tags.slice(0, Math.max(0, take)),
        hidden: Math.max(0, group.tags.length - take),
      };
    })
    .filter((group) => group.tags.length > 0);
  const visibleGroups: (typeof groups[number] & { hidden?: number })[] = expanded ? groups : collapsedGroups;

  return (
    <aside className={'inspector inspector--' + mode} aria-label="图片信息">
      <span className="inspector__handle" aria-hidden="true" />
      <div className="inspector__scroll">
        <header className="inspector__head">
          <div>
            <p className="inspector__eyebrow">{mode === 'detail' ? '详情' : '聚焦'}</p>
            <h2 className="inspector__title">{item.n}</h2>
          </div>
          <button type="button" className="inspector__close" onClick={onClose} title="收起信息" aria-label="收起信息">
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" aria-hidden="true">
              <path d="M6 6l12 12" />
              <path d="M18 6 6 18" />
            </svg>
          </button>
        </header>

        <dl className="inspector__meta">
          <dt>尺寸</dt>
          <dd>{formatSize(item.w, item.h)}</dd>
          <dt>日期</dt>
          <dd>{formatDateTime(item.mtime)}</dd>
          <dt>分级</dt>
          <dd>{ratingLabel(item.rt)}</dd>
          <dt>星级</dt>
          <dd>{formatStars(item.st)}</dd>
          <dt>分组</dt>
          <dd>{item.group || '未分组'}</dd>
          <dt>标签</dt>
          <dd>{formatCount(totalTags)} 个</dd>
        </dl>

        <section className="inspector__section" aria-labelledby="inspector-tags">
          <h3 id="inspector-tags">标签</h3>
          {visibleGroups.length > 0 ? (
            <div className="inspector__tags">
              {visibleGroups.map((group) => (
                <div className="inspector__taggroup" key={group.category}>
                  <span className="inspector__taggroup-label">{group.label}</span>
                  <div className="inspector__chips">
                    {group.tags.map((tag) => (
                      <button
                        key={tag.name + '|' + group.category}
                        type="button"
                        className="inspector__chip"
                        title={categoryLabel(group.category)}
                        onClick={() => onTagClick(tag.name)}
                      >
                        {tag.name}
                      </button>
                    ))}
                    {!expanded && (group.hidden ?? 0) > 0 ? (
                      <span className="inspector__chip inspector__chip--more" aria-hidden="true">
                        +{group.hidden ?? 0}
                      </span>
                    ) : null}
                  </div>
                </div>
              ))}
              {totalTags > COLLAPSED_TAGS ? (
                <button type="button" className="inspector__toggle" onClick={() => setExpanded((value) => !value)}>
                  {expanded ? '收起标签' : `全部 ${totalTags} 个标签`}
                </button>
              ) : null}
            </div>
          ) : (
            <p className="inspector__hint">这张图还没有标签。</p>
          )}
        </section>

        <section className="inspector__section" aria-labelledby="inspector-similar">
          <h3 id="inspector-similar">相似</h3>
          {similar.length > 0 ? (
            <div className="inspector__similar">
              {similar.map((entry) => (
                <button
                  key={entry.id}
                  type="button"
                  className="inspector__similar-item"
                  title={entry.n}
                  aria-label={entry.n}
                  onClick={() => onSimilar(entry.id)}
                >
                  <img src={tierUrl(paths, 't256', entry.id)} alt="" decoding="async" />
                </button>
              ))}
            </div>
          ) : (
            <p className="inspector__hint">当前范围里没有 pHash 相近的图。</p>
          )}
        </section>

        <div className="inspector__actions">
          {mode === 'spotlight' ? (
            <button type="button" className="inspector__primary" onClick={() => onOpenDetail(item.id)}>
              打开详情
            </button>
          ) : onBack ? (
            <button type="button" className="inspector__primary" onClick={onBack}>
              返回列表
            </button>
          ) : null}
          {original ? (
            <a className="inspector__link" href={original} target="_blank" rel="noreferrer">
              打开原图
            </a>
          ) : null}
        </div>
      </div>
    </aside>
  );
}
