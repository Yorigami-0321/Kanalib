import { useEffect, useMemo, useState } from 'react';
import { detailSources, originalUrl, tierUrl } from '../lib/assetUrl';
import { formatCount, formatDateTime, formatSize, formatStars, ratingLabel, truncateMiddle } from '../lib/format';
import { hammingHex } from '../lib/hash';
import type { TierPaths, WallItem } from '../source/types';

const SIMILAR_LIMIT = 12;

export interface DetailProps {
  id: string;
  items: readonly WallItem[];
  paths: TierPaths;
  hasMore: boolean;
  looking: boolean;
  onBack: () => void;
  onOpenDetail: (id: string) => void;
  onSearchMore: () => void;
  onTagClick: (tag: string) => void;
}

interface SimilarEntry {
  item: WallItem;
  distance: number;
}

/** 详情页：大图 + 元数据 + 标签 chips + 相似推荐（ph Hamming，限已加载集合）。 */
export function Detail({
  id,
  items,
  paths,
  hasMore,
  looking,
  onBack,
  onOpenDetail,
  onSearchMore,
  onTagClick,
}: DetailProps) {
  const item = useMemo(() => items.find((candidate) => candidate.id === id), [items, id]);
  const sources = useMemo(() => (item ? detailSources(paths, item) : []), [item, paths]);
  const [sourceIndex, setSourceIndex] = useState(0);

  useEffect(() => {
    setSourceIndex(0);
  }, [id]);

  const similar = useMemo<SimilarEntry[]>(() => {
    if (!item?.ph) return [];
    const entries: SimilarEntry[] = [];
    for (const other of items) {
      if (other.id === item.id || !other.ph) continue;
      const distance = hammingHex(item.ph, other.ph);
      if (distance === null) continue;
      entries.push({ item: other, distance });
    }
    entries.sort((left, right) => left.distance - right.distance);
    return entries.slice(0, SIMILAR_LIMIT);
  }, [item, items]);

  if (!item) {
    return (
      <section className="detail detail--missing">
        <button type="button" className="button" onClick={onBack}>
          ← 返回图片墙
        </button>
        <p className="detail__missing-title">{looking ? '正在已加载的分片里查找…' : '这条资产不在已加载的数据里'}</p>
        {!looking && hasMore ? (
          <>
            <p className="detail__missing-hint">可以继续顺序加载下一片再找（不会一次性拉完整库）。</p>
            <button type="button" className="button button--primary" onClick={onSearchMore}>
              继续加载分片并查找
            </button>
          </>
        ) : null}
      </section>
    );
  }

  const src = sources[sourceIndex];
  const original = originalUrl(paths, item);

  return (
    <section className="detail">
      <div className="detail__head">
        <button type="button" className="button" onClick={onBack}>
          ← 返回图片墙
        </button>
        <h1 className="detail__title" title={item.n}>
          {item.n}
        </h1>
        <span className="detail__id" title={item.id}>
          {truncateMiddle(item.id)}
        </span>
      </div>

      <div className="detail__body">
        <div className="detail__stage">
          {src ? (
            <img
              className="detail__image"
              src={src}
              alt={item.n}
              width={item.w}
              height={item.h}
              onError={() => setSourceIndex((prev) => prev + 1)}
            />
          ) : (
            <p className="detail__image-fallback">图片不可用（原图与缩略图都取不到）</p>
          )}
        </div>

        <aside className="detail__side">
          <dl className="meta">
            <div className="meta__row">
              <dt>尺寸</dt>
              <dd>{formatSize(item.w, item.h)}</dd>
            </div>
            <div className="meta__row">
              <dt>星级</dt>
              <dd title={`${item.st} / 5`}>{formatStars(item.st)}</dd>
            </div>
            <div className="meta__row">
              <dt>收藏</dt>
              <dd>{item.fav ? '♥ 已收藏' : '未收藏'}</dd>
            </div>
            <div className="meta__row">
              <dt>分级</dt>
              <dd>{ratingLabel(item.rt)}</dd>
            </div>
            <div className="meta__row">
              <dt>时间</dt>
              <dd>{formatDateTime(item.mtime)}</dd>
            </div>
            <div className="meta__row">
              <dt>分组</dt>
              <dd>{item.group ?? '未分组'}</dd>
            </div>
            <div className="meta__row">
              <dt>原图</dt>
              <dd>
                {original ? (
                  <a href={original} target="_blank" rel="noreferrer">
                    打开原图（.{item.orig}）
                  </a>
                ) : (
                  '未发布原图'
                )}
              </dd>
            </div>
            <div className="meta__row">
              <dt>标签</dt>
              <dd>{formatCount(item.tags.length)} 个</dd>
            </div>
          </dl>

          <div className="detail__tags">
            <h2 className="detail__subtitle">标签</h2>
            {item.tags.length === 0 ? (
              <p className="detail__empty">这条资产没有标签。</p>
            ) : (
              <ul className="tag-list">
                {item.tags.map(([name, category, score]) => (
                  <li key={`${category}:${name}`}>
                    <button
                      type="button"
                      className="tag-pill"
                      data-category={category}
                      title={`${name}（${category}，score ${score.toFixed(3)}）· 点击回到墙上筛选`}
                      onClick={() => onTagClick(name)}
                    >
                      {name}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </aside>
      </div>

      <div className="detail__similar">
        <h2 className="detail__subtitle">
          相似推荐 <span className="detail__subtitle-hint">按 pHash 距离，仅限已加载的 {formatCount(items.length)} 张</span>
        </h2>
        {item.ph === undefined ? (
          <p className="detail__empty">这条资产没有 pHash，无法计算相似度。</p>
        ) : similar.length === 0 ? (
          <p className="detail__empty">已加载的数据里没有其它带 pHash 的条目。</p>
        ) : (
          <ul className="similar-list">
            {similar.map(({ item: other, distance }) => (
              <li key={other.id}>
                <button
                  type="button"
                  className="similar-card"
                  onClick={() => onOpenDetail(other.id)}
                  title={`${other.n} · 距离 ${distance}`}
                >
                  <img
                    className="similar-card__img"
                    src={tierUrl(paths, 't256', other.id)}
                    alt={other.n}
                    loading="lazy"
                    decoding="async"
                    onError={(event) => {
                      const image = event.currentTarget;
                      if (image.dataset.fallback === '1') {
                        image.style.visibility = 'hidden';
                        return;
                      }
                      image.dataset.fallback = '1';
                      image.src = tierUrl(paths, 't512', other.id);
                    }}
                  />
                  <span className="similar-card__dist">{distance}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </section>
  );
}
