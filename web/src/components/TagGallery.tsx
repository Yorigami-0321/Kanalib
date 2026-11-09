import { tierUrl } from '../lib/assetUrl';
import { formatCount } from '../lib/format';
import type { TierPaths } from '../source/types';
import './taggallery.css';

export interface TagEntry {
  name: string;
  category: string;
  categoryKey: string;
  count: number;
  coverId?: string;
}

export interface TagGalleryProps {
  entries: TagEntry[];
  paths: TierPaths;
  loading: boolean;
  onOpenTag: (tag: string) => void;
}

interface Section {
  key: string;
  label: string;
  entries: TagEntry[];
}

/** §2.2：类别顺序固定 通用 → 角色 → 作品 → 画师，其余（手动/元信息/分级）殿后。 */
const CATEGORY_ORDER = ['general', 'character', 'copyright', 'artist', 'manual', 'meta', 'rating'];

function categoryRank(key: string): number {
  const index = CATEGORY_ORDER.indexOf(key);
  return index < 0 ? CATEGORY_ORDER.length : index;
}

function groupByCategory(entries: TagEntry[]): Section[] {
  const map = new Map<string, Section>();
  for (const entry of entries) {
    const key = entry.categoryKey || 'other';
    const existing = map.get(key);
    if (existing) existing.entries.push(entry);
    else map.set(key, { key, label: entry.category || '其他', entries: [entry] });
  }
  const sections = [...map.values()];
  for (const section of sections) {
    section.entries.sort(
      (left, right) => right.count - left.count || left.name.localeCompare(right.name, 'zh-Hans-CN'),
    );
  }
  // 先按固定类别顺序排，未知类别按条目数兜底
  return sections.sort(
    (left, right) =>
      categoryRank(left.key) - categoryRank(right.key) || right.entries.length - left.entries.length,
  );
}

/**
 * 第一层：标签总览。排版即界面——按类别分段，每段一条发丝线；
 * 每个标签用超大名字压在固定封面上，点进去才是这个标签的全部图片（第二层）。
 */
export function TagGallery({ entries, paths, loading, onOpenTag }: TagGalleryProps) {
  const sections = groupByCategory(entries);

  return (
    <section className={loading ? 'tags tags--loading' : 'tags'} aria-busy={loading || undefined}>
      <header className="tags__head">
        <h1 className="tags__title">标签</h1>
        <p className="tags__sub">
          {loading ? '正在整理整个库…' : formatCount(entries.length) + ' 个标签'}
        </p>
      </header>

      {sections.map((section) => {
        const headingId = 'tags-group-' + section.key;
        return (
          <section className="tags__group" key={section.key} aria-labelledby={headingId}>
            <div className="tags__group-head">
              <h2 className="tags__group-title" id={headingId}>
                {section.label}
              </h2>
              <span className="tags__group-rule" aria-hidden="true" />
              <span className="tags__group-count">{formatCount(section.entries.length)} 个</span>
            </div>
            <div className="tags__grid">
              {section.entries.map((entry) => (
                <button
                  key={entry.name}
                  type="button"
                  className="tagcard"
                  onClick={() => onOpenTag(entry.name)}
                  title={entry.name + ' · ' + formatCount(entry.count) + ' 张'}
                >
                  {entry.coverId ? (
                    <img
                      className="tagcard__img"
                      src={tierUrl(paths, 't256', entry.coverId)}
                      alt={entry.name}
                      loading="lazy"
                      decoding="async"
                      draggable={false}
                    />
                  ) : (
                    <span className="tagcard__blank" aria-hidden="true" />
                  )}
                  <span className="tagcard__shade" aria-hidden="true" />
                  <span className="tagcard__body">
                    <span className="tagcard__name">{entry.name}</span>
                    <span className="tagcard__meta">{formatCount(entry.count)} 张</span>
                  </span>
                </button>
              ))}
            </div>
          </section>
        );
      })}

      {!loading && entries.length === 0 ? <p className="tags__empty">这个库里还没有标签。</p> : null}
    </section>
  );
}
