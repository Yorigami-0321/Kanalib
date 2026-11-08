import type { WallItem } from '../source/types';

export interface FilterState {
  /** 文件名子串（大小写不敏感）。 */
  query: string;
  /** 标签多选（任一命中即通过；与库里 tag: 的 AND 语义不同，这里的墙是浏览式的）。 */
  tags: string[];
  onlyFav: boolean;
  /** 分级多选，空数组 = 全部分级。 */
  ratings: string[];
  /** 最低星级（0 = 不限）。 */
  minStars: number;
  /** 分组成员（契约 v1 的 item 未定义该字段时无匹配）。 */
  group: string | null;
}

export const EMPTY_FILTERS: FilterState = {
  query: '',
  tags: [],
  onlyFav: false,
  ratings: [],
  minStars: 0,
  group: null,
};

export function isFilterActive(filters: FilterState): boolean {
  return (
    filters.query.trim().length > 0 ||
    filters.tags.length > 0 ||
    filters.onlyFav ||
    filters.ratings.length > 0 ||
    filters.minStars > 0 ||
    filters.group !== null
  );
}

/** 筛选签名：变化时让无限网格整体重排（Masonry 不支持原地重排尺寸）。 */
export function filterSignature(filters: FilterState): string {
  return JSON.stringify({
    q: filters.query.trim().toLowerCase(),
    t: [...filters.tags].sort(),
    f: filters.onlyFav,
    r: [...filters.ratings].sort(),
    s: filters.minStars,
    g: filters.group,
  });
}

export function filterItems(items: readonly WallItem[], filters: FilterState): WallItem[] {
  const needle = filters.query.trim().toLowerCase();
  const tags = filters.tags;
  const ratings = filters.ratings;
  return items.filter((item) => {
    if (needle.length > 0 && !item.n.toLowerCase().includes(needle)) return false;
    if (filters.onlyFav && !item.fav) return false;
    if (ratings.length > 0 && !ratings.includes(item.rt)) return false;
    if (filters.minStars > 0 && item.st < filters.minStars) return false;
    if (filters.group !== null && item.group !== filters.group) return false;
    if (tags.length > 0) {
      const names = new Set(item.tags.map((tag) => tag[0]));
      if (!tags.some((tag) => names.has(tag))) return false;
    }
    return true;
  });
}
