import { useCallback, useEffect, useMemo, useState } from 'react';
import { detailSources, originalUrl, tierUrl } from '../lib/assetUrl';
import { categoryLabel, formatCount, formatDateTime, formatSize, ratingLabel } from '../lib/format';
import { hammingHex } from '../lib/hash';
import type { TierPaths, WallItem } from '../source/types';
import './detail.css';

const SIMILAR_DISTANCE = 12;
const SIMILAR_LIMIT = 6;

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
 * 全屏作品页：图固定占满视口（contain），信息压成底部一块，
 * 信息层自己滚动——B2 的修复就在这里，不依赖 shared styles.css 的 body 滚动。
 */
export function Detail({ item, items, paths, onBack, onOpenDetail, onTagClick }: DetailProps) {
  const sources = detailSources(paths, item);
  // P2-3：失败档位跟 item.id 绑在一起存，不在 effect 里重置 —— 换 item 的那一帧
  // effect 还没跑，旧 srcIndex 会去索引新的 sources（可能越界成 undefined）。
  const [loadStage, setLoadStage] = useState<{ id: string; index: number }>({ id: item.id, index: 0 });
  const srcIndex = loadStage.id === item.id ? Math.min(loadStage.index, sources.length - 1) : 0;

  const index = useMemo(() => items.findIndex((entry) => entry.id === item.id), [items, item.id]);
  const prevId = index > 0 ? items[index - 1]?.id : undefined;
  const nextId = index >= 0 && index + 1 < items.length ? items[index + 1]?.id : undefined;

  const similar = useMemo<WallItem[]>(() => {
    if (!item.ph) return [];
    const scored: { entry: WallItem; distance: number }[] = [];
    for (const entry of items) {
      if (entry.id === item.id) continue;
      const distance = hammingHex(item.ph, entry.ph);
      if (distance !== null && distance <= SIMILAR_DISTANCE) scored.push({ entry, distance });
    }
    scored.sort((left, right) => left.distance - right.distance);
    return scored.slice(0, SIMILAR_LIMIT).map((entry) => entry.entry);
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
      } else if (event.key === 'Escape') {
        event.preventDefault();
        onBack();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [onBack, openByOffset]);

  const original = originalUrl(paths, item);

  return (
    <article className="view view--detail">
      <div className="view__media">
        <img
          className="view__img"
          src={sources[srcIndex]}
          alt={item.n}
          decoding="async"
          onError={() =>
            setLoadStage((current) => {
              const base = current.id === item.id ? current.index : 0;
              return { id: item.id, index: Math.min(base + 1, sources.length - 1) };
            })
          }
        />
      </div>
      <div className="view__scrim" aria-hidden="true" />

      <header className="view__bar">
        <button type="button" className="icon-btn" onClick={onBack} title="返回（Esc）" aria-label="返回">
          <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M19 12H5" />
            <path d="m11 18-6-6 6-6" />
          </svg>
        </button>
        <div className="view__bar-end">
          <button
            type="button"
            className="view__step"
            onClick={() => openByOffset(-1)}
            disabled={!prevId}
            title="上一张（←）"
            aria-label="上一张"
          >
            ←
          </button>
          <button
            type="button"
            className="view__step"
            onClick={() => openByOffset(1)}
            disabled={!nextId}
            title="下一张（→）"
            aria-label="下一张"
          >
            →
          </button>
          <span className="view__seq">
            {index >= 0 ? pad(index + 1) : '--'} / {formatCount(items.length)}
          </span>
        </div>
      </header>

      <div className="view__scroll">
        <div className="view__inner">
          <div className="view__info">
            <div className="view__text">
              <h1 className="view__title">{item.n}</h1>
              <p className="view__meta">
                <span>{formatSize(item.w, item.h)}</span>
                {item.mtime ? <span>{formatDateTime(item.mtime)}</span> : null}
                {item.rt ? <span>{ratingLabel(item.rt)}</span> : null}
                {item.st > 0 ? <span>{'★'.repeat(Math.min(5, Math.round(item.st)))}</span> : null}
              </p>

              <div className="view__tags">
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
                {original ? (
                  <a className="chip chip--accent" href={original} target="_blank" rel="noreferrer">
                    原图
                  </a>
                ) : null}
              </div>
            </div>

            {similar.length > 0 ? (
              <div className="view__similar">
                <span className="view__similar-label">相似</span>
                <div className="view__similar-strip">
                  {similar.map((entry) => (
                    <button
                      key={entry.id}
                      type="button"
                      className="similar__item"
                      onClick={() => onOpenDetail(entry.id)}
                      title={entry.n}
                      aria-label={entry.n}
                    >
                      <img src={tierUrl(paths, 't256', entry.id)} alt="" loading="lazy" decoding="async" />
                    </button>
                  ))}
                </div>
              </div>
            ) : null}
          </div>
        </div>
      </div>
    </article>
  );
}
