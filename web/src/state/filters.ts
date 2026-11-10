import type { WallItem } from '../source/types';

/**
 * 浏览器侧只剩文件名搜索；标签/收藏/分级/分组已经是独立的两级路径（Q8），
 * 不再作为叠加在墙上的滤镜。
 */
export interface FilterState {
  /** 文件名子串（大小写不敏感）。 */
  query: string;
}

export const EMPTY_FILTERS: FilterState = { query: '' };

export function isFilterActive(filters: FilterState): boolean {
  return filters.query.trim().length > 0;
}

/** 筛选签名：变化时让无限网格整体重排。 */
export function filterSignature(filters: FilterState): string {
  return filters.query.trim().toLowerCase();
}

export function filterItems(items: readonly WallItem[], filters: FilterState): WallItem[] {
  const needle = filters.query.trim().toLowerCase();
  if (needle.length === 0) return [...items];
  return items.filter((item) => item.n.toLowerCase().includes(needle));
}
