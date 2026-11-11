import type { WallItem } from '../source/types';
import { categoryLabel } from './format';
import { hammingHex } from './hash';

export const SIMILAR_DISTANCE = 12;
export const SIMILAR_LIMIT = 6;
export const TAG_CATEGORY_ORDER = ['general', 'character', 'copyright', 'artist', 'manual', 'style', 'meta', 'rating'];

export interface TagGroup {
  category: string;
  label: string;
  tags: { name: string; score: number }[];
}

function categoryRank(category: string): number {
  const index = TAG_CATEGORY_ORDER.indexOf(category);
  return index < 0 ? TAG_CATEGORY_ORDER.length : index;
}

/** 按类别分组，组内按置信度降序；类别顺序固定，未知类别殿后。 */
export function groupTags(item: WallItem): TagGroup[] {
  const map = new Map<string, { name: string; score: number }[]>();
  for (const [name, category, score] of item.tags) {
    const list = map.get(category);
    if (list) list.push({ name, score });
    else map.set(category, [{ name, score }]);
  }
  return [...map.entries()]
    .sort(([left], [right]) => categoryRank(left) - categoryRank(right))
    .map(([category, tags]) => ({
      category,
      label: categoryLabel(category),
      tags: tags.sort((left, right) => right.score - left.score || left.name.localeCompare(right.name)),
    }));
}

/** 当前集合里与 item 的 pHash 汉明距离最近的若干张。 */
export function similarOf(item: WallItem, items: readonly WallItem[]): WallItem[] {
  if (!item.ph) return [];
  const scored: { entry: WallItem; distance: number }[] = [];
  for (const entry of items) {
    if (entry.id === item.id) continue;
    const distance = hammingHex(item.ph, entry.ph);
    if (distance !== null && distance <= SIMILAR_DISTANCE) scored.push({ entry, distance });
  }
  scored.sort((left, right) => left.distance - right.distance);
  return scored.slice(0, SIMILAR_LIMIT).map((entry) => entry.entry);
}

export type RelatedKind = 'phash' | 'tag' | 'group';

export interface RelatedItems {
  kind: RelatedKind;
  items: WallItem[];
}

/** 分级标签人人都带，拿它算「同标签」会把整个库都算进来。 */
const EXCLUDED_FALLBACK_CATEGORIES = new Set(['rating']);

function tagMap(item: WallItem): Map<string, number> {
  const map = new Map<string, number>();
  for (const [name, category, score] of item.tags) {
    if (EXCLUDED_FALLBACK_CATEGORIES.has(category)) continue;
    const prev = map.get(name);
    if (prev === undefined || score > prev) map.set(name, score);
  }
  return map;
}

/**
 * 详情页的相关图：先 pHash，≤12 没有结果时回退同标签、再回退同分组。
 * 返回 null 表示这个集合里确实没有可推荐的——调用方应整块隐藏，
 * 而不是留一个空的「相似」区块。
 */
export function relatedOf(item: WallItem, items: readonly WallItem[]): RelatedItems | null {
  const similar = similarOf(item, items);
  if (similar.length > 0) return { kind: 'phash', items: similar };

  const own = tagMap(item);
  if (own.size > 0) {
    const scored: { entry: WallItem; shared: number; weight: number }[] = [];
    for (const entry of items) {
      if (entry.id === item.id) continue;
      const seen = new Set<string>();
      let shared = 0;
      let weight = 0;
      for (const [name, category, score] of entry.tags) {
        if (EXCLUDED_FALLBACK_CATEGORIES.has(category) || seen.has(name)) continue;
        const ownScore = own.get(name);
        if (ownScore === undefined) continue;
        seen.add(name);
        shared += 1;
        weight += ownScore + score;
      }
      if (shared > 0) scored.push({ entry, shared, weight });
    }
    if (scored.length > 0) {
      scored.sort(
        (left, right) =>
          right.shared - left.shared ||
          right.weight - left.weight ||
          left.entry.id.localeCompare(right.entry.id),
      );
      return { kind: 'tag', items: scored.slice(0, SIMILAR_LIMIT).map((entry) => entry.entry) };
    }
  }

  if (item.group) {
    const sameGroup: WallItem[] = [];
    for (const entry of items) {
      if (entry.id === item.id || entry.group !== item.group) continue;
      sameGroup.push(entry);
      if (sameGroup.length >= SIMILAR_LIMIT) break;
    }
    if (sameGroup.length > 0) return { kind: 'group', items: sameGroup };
  }

  return null;
}
