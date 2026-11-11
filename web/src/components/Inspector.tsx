import { useEffect, useMemo, useRef, useState } from 'react';
import { originalUrl, tierUrl } from '../lib/assetUrl';
import {
  categoryLabel,
  displayGroup,
  displayTag,
  formatCount,
  formatDateTime,
  formatSize,
  formatStars,
  ratingLabel,
} from '../lib/format';
import { detailHref } from '../lib/router';
import { groupTags, relatedOf } from '../lib/itemDetail';
import type { TierPaths, WallItem } from '../source/types';
import './inspector.css';

const COLLAPSED_TAGS = 18;
const COLLAPSED_PER_GROUP = 6;

export interface InspectorProps {
  item: WallItem;
  items: readonly WallItem[];
  paths: TierPaths;
  onClose: () => void;
  onBack?: () => void;
  onTagClick: (tag: string) => void;
  onSimilar: (id: string) => void;
  /** 退出窗口：收起时先播 200ms 离场动画再卸载（Sidebar 驱动）。 */
  leaving?: boolean;
}

/**
 * 左侧信息检查器（v4）：详情页的标题/元数据/标签/相似条全部在这里，
 * 画面本身不再挂底部信息层。单击聚焦不带检查器（反馈问题 2）。
 */
export function Inspector({
  item,
  items,
  paths,
  onClose,
  onBack,
  onTagClick,
  onSimilar,
  leaving = false,
}: InspectorProps) {
  const [expanded, setExpanded] = useState(false);
  const [copied, setCopied] = useState(false);
  const copyTimer = useRef(0);
  useEffect(() => () => window.clearTimeout(copyTimer.current), []);

  const copyLink = async () => {
    try {
      const link = new URL(detailHref(item.id), window.location.href).href;
      await navigator.clipboard.writeText(link);
      setCopied(true);
      window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => setCopied(false), 1800);
    } catch {
      setCopied(false);
    }
  };

  const toggleFullscreen = async () => {
    try {
      if (document.fullscreenElement) await document.exitFullscreen();
      else await document.documentElement.requestFullscreen();
    } catch {
      /* 浏览器拒绝（非用户手势/权限）时保持原状 */
    }
  };

  // 分级已经在元信息行展示，`rating:g` chip 不再重复；其余分组按类别展示。
  const groups = groupTags(item).filter((group) => group.category !== 'rating');
  // 先 pHash，没有 ≤12 的邻居时回退同标签/同分组；null 时整块不渲染。
  const related = useMemo(() => relatedOf(item, items), [item, items]);
  const relatedLabel = !related
    ? ''
    : related.kind === 'tag'
      ? '同标签'
      : related.kind === 'group'
        ? '同分组'
        : '相似';
  const original = originalUrl(paths, item);
  const totalTags = groups.reduce((sum, group) => sum + group.tags.length, 0);

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
    <aside className={'inspector' + (leaving ? ' inspector--leave' : '')} aria-label="图片信息">
      <span className="inspector__handle" aria-hidden="true" />
      <div className="inspector__scroll">
        <header className="inspector__head">
          <div>
            <p className="inspector__eyebrow">详情</p>
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
          <dd>{displayGroup(item.group ?? '')}</dd>
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
                        {displayTag(tag.name)}
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

        {related ? (
          <section className="inspector__section" aria-labelledby="inspector-similar">
            <h3 id="inspector-similar">{relatedLabel}</h3>
            {related.kind !== 'phash' ? (
              <p className="inspector__hint">没有足够相近的图片，以下为{relatedLabel}。</p>
            ) : null}
            <div className="inspector__similar">
              {related.items.map((entry) => (
                <button
                  key={entry.id}
                  type="button"
                  className="inspector__similar-item"
                  title={entry.n}
                  aria-label={entry.n}
                  onClick={() => onSimilar(entry.id)}
                >
                  <img
                    src={tierUrl(paths, 't256', entry.id)}
                    alt=""
                    decoding="async"
                    loading="lazy"
                    fetchPriority="low"
                  />
                </button>
              ))}
            </div>
          </section>
        ) : null}
      </div>

      <div className="inspector__actions">
        {onBack ? (
          <button type="button" className="inspector__primary" onClick={onBack}>
            返回列表
          </button>
        ) : null}
        <div className="inspector__secondary">
          <button
            type="button"
            className={'inspector__link inspector__link--button' + (copied ? ' is-done' : '')}
            onClick={copyLink}
            aria-live="polite"
          >
            {copied ? '✓ 已复制链接' : '复制链接'}
          </button>
          <button type="button" className="inspector__link inspector__link--button" onClick={toggleFullscreen}>
            全屏（F）
          </button>
          {original ? (
            <a className="inspector__link" href={original} target="_blank" rel="noreferrer">
              原图
            </a>
          ) : null}
        </div>
      </div>
    </aside>
  );
}
