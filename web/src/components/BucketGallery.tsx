import type { ReactNode } from 'react';
import { tierUrl } from '../lib/assetUrl';
import { formatCount } from '../lib/format';
import type { TierPaths } from '../source/types';
import './taggallery.css';

export interface BucketEntry {
  key: string;
  label: string;
  count: number;
  coverId?: string;
  href: string;
}

export interface BucketGalleryProps {
  title: string;
  subtitle?: string;
  entries: BucketEntry[];
  paths: TierPaths;
  loading: boolean;
  emptyText: string;
  /** 空态动作（回首页等）；由 App 给出，保持按钮可键盘触发。 */
  emptyAction?: ReactNode;
  onOpen: (href: string) => void;
}

/**
 * 收藏 / 分级 / 分组的第一层：与标签总览同一套排版（大名字压在封面上），
 * 点进第二层才是这一类别的全部图片（Q8）。
 */
export function BucketGallery({
  title,
  subtitle,
  entries,
  paths,
  loading,
  emptyText,
  emptyAction,
  onOpen,
}: BucketGalleryProps) {
  return (
    <section className={loading ? 'tags buckets tags--loading' : 'tags buckets'} aria-busy={loading || undefined}>
      <header className="tags__head">
        <h1 className="tags__title">{title}</h1>
        <p className="tags__sub">
          {loading ? '正在整理整个库…' : subtitle ?? formatCount(entries.length) + ' 项'}
        </p>
      </header>

      {entries.length > 0 ? (
        <div className="tags__grid">
          {entries.map((entry, index) => (
            <button
              key={entry.key}
              type="button"
              className="tagcard tagcard--enter"
              style={{ animationDelay: Math.min(320, index * 24) + 'ms' }}
              onClick={() => onOpen(entry.href)}
              title={entry.label + ' · ' + formatCount(entry.count) + ' 张'}
            >
              {entry.coverId ? (
                <img
                  className="tagcard__img"
                  src={tierUrl(paths, 't256', entry.coverId)}
                  alt={entry.label}
                  loading="lazy"
                  decoding="async"
                  draggable={false}
                />
              ) : (
                <span className="tagcard__blank" aria-hidden="true" />
              )}
              <span className="tagcard__shade" aria-hidden="true" />
              <span className="tagcard__body">
                <span className="tagcard__name">{entry.label}</span>
                <span className="tagcard__meta">{formatCount(entry.count)} 张</span>
              </span>
            </button>
          ))}
        </div>
      ) : !loading ? (
        <>
          <p className="tags__empty">{emptyText}</p>
          {emptyAction ? <div className="tags__empty-actions">{emptyAction}</div> : null}
        </>
      ) : null}
    </section>
  );
}
