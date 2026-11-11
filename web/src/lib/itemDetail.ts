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
