import { useCallback, useEffect, useMemo, useState } from 'react';
import { detailSources, originalUrl, tierUrl } from '../lib/assetUrl';
import { categoryLabel, formatCount, formatDateTime, formatSize, ratingLabel } from '../lib/format';
import { hammingHex } from '../lib/hash';
import type { TierPaths, WallItem } from '../source/types';

/** 相似判定的 Hamming 距离上限（64 位 pHash）。 */
const SIMILAR_DISTANCE = 12;
const SIMILAR_LIMIT = 10;

export interface DetailProps {
  item: WallItem;
  items: readonly WallItem[];
  paths: TierPaths;
  onBack: () => void;
  onOpenDetail: (id: string) => void;
  onTagClick: (tag: string) => void;
}

function pad(value: number): string {
  return value < 10 ? '0' + String(value) : String(value);
}

/**
 * 展示型详情页：左侧大图、右侧信息列、底部相似横条。
 * 没有工具栏、没有操作按钮栏，只保留返回、序号和内容本身。
 */
export function Detail({ item, items, paths, onBack, onOpenDetail, onTagClick }: DetailProps) {
  const sources = detailSources(paths, item);
  const [srcIndex, setSrcIndex] = useState(0);

  useEffect(() => {
    setSrcIndex(0);
  }, [item.id]);

  const index = useMemo(() => items.findIndex((entry) => entry.id === item.id), [items, item.id]);
  const prevId = index > 0 ? items[index - 1]?.id : undefined;
  const nextId = index >= 0 && index + 1 < items.length ? items[index + 1]?.id : undefined;

  const similar = useMemo(() => {
    if (!item.ph) return [];
    const scored: { id: string; distance: number }[] = [];
    for (const entry of items) {
      if (entry.id === item.id) continue;
      const distance = hammingHex(item.ph, entry.ph);
      if (distance !== null && distance <= SIMILAR_DISTANCE) scored.push({ id: entry.id, distance });
    }
    scored.sort((a, b) => a.distance - b.distance);
    return scored.slice(0, SIMILAR_LIMIT).map((entry) => entry.id);
  }, [items, item.id, item.ph]);

  const openByOffset = useCallback(
    (offset: number) => {
      const target = offset < 0 ? prevId : nextId;
      if (target) onOpenDetail(target);
    },
    [nextId, onOpenDetail, prevId],
  );

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target;
      if (target instanceof Element && target.closest('input, textarea, select')) return;
      if (event.key === 'ArrowLeft') {
        event.preventDefault();
        openByOffset(-1);
      } else if (event.key === 'ArrowRight') {
        event.preventDefault();
        openByOffset(1);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [openByOffset]);

  const original = originalUrl(paths, item);

  return (
    <article className="view">
      <header className="view__bar">
        <button type="button" className="icon-btn" onClick={onBack} title="返回墙（Esc）" aria-label="返回">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M19 12H5" />
            <path d="m11 18-6-6 6-6" />
          </svg>
        </button>
        <span className="view__seq">
          {index >= 0 ? pad(index + 1) : '--'} / {formatCount(items.length)}
        </span>
      </header>

      <div className="detail">
        <figure className="detail__figure">
          <img
            src={sources[srcIndex]}
            alt={item.n}
            decoding="async"
            onError={() => setSrcIndex((current) => (current + 1 < sources.length ? current + 1 : current))}
          />
        </figure>

        <aside className="detail__info">
          <h1 className="detail__title">{item.n}</h1>
          <p className="detail__meta">
            {formatSize(item.w, item.h)}
            {item.mtime ? ' · ' + formatDateTime(item.mtime).slice(0, 10) : ''}
            {item.rt ? ' · ' + ratingLabel(item.rt) : ''}
          </p>

          <div className="detail__rule" />

          {item.tags.length > 0 ? (
            <div className="detail__chips">
              {item.tags.map(([name, category]) => (
                <button
                  key={name + '|' + category}
                  type="button"
                  className="chip"
                  title={categoryLabel(category)}
                  onClick={() => onTagClick(name)}
                >
                  {name}
                </button>
              ))}
            </div>
          ) : (
            <p className="detail__meta" style={{ margin: 0 }}>
              这张图还没有标签。
            </p>
          )}

          <div className="detail__actions">
            {original ? (
              <a className="chip chip--accent" href={original} target="_blank" rel="noreferrer">
                打开原图
              </a>
            ) : null}
            {prevId ? (
              <button type="button" className="chip" onClick={() => openByOffset(-1)}>
                ← 上一张
              </button>
            ) : null}
            {nextId ? (
              <button type="button" className="chip" onClick={() => openByOffset(1)}>
                下一张 →
              </button>
            ) : null}
          </div>

          <p className="detail__foot">
            {item.group ? '分组 ' + item.group + ' · ' : ''}
            {item.st > 0 ? '★' + Math.round(item.st) + ' · ' : ''}
            {item.fav ? '已收藏 · ' : ''}
            ID {item.id}
          </p>
        </aside>
      </div>

      {similar.length > 0 ? (
        <section className="similar">
          <div className="similar__head">
            <h2>相似</h2>
          </div>
          <div className="similar__track">
            {similar.map((id) => {
              const entry = items.find((candidate) => candidate.id === id);
              if (!entry) return null;
              return (
                <button
                  key={id}
                  type="button"
                  className="similar__item"
                  onClick={() => onOpenDetail(id)}
                  title={entry.n}
                >
                  <img src={tierUrl(paths, 't256', entry.id)} alt={entry.n} loading="lazy" decoding="async" />
                </button>
              );
            })}
          </div>
        </section>
      ) : null}
    </article>
  );
}
