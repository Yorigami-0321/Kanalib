import { RATING_LABELS } from '../lib/format';
import type { SearchRecord } from '../source/StaticSource';
import type { WallItem } from '../source/types';

/**
 * 浏览器侧只剩搜索；标签/收藏/分级/分组已经是独立的两级路径（Q8），
 * 不再作为叠加在墙上的滤镜。
 *
 * 搜索有两种运行形态：
 * - 搜索索引已就绪：名称 / 标签 / 分组 / 分级全库参与，支持空格 AND、`-词` 排除、`tag:` 限定；
 * - 索引未就绪或 404：退回旧的文件名子串搜索（行为与 v1 一致）。
 */
export interface FilterState {
  /** 原始查询串；语法由 `parseQuery` 解释。 */
  query: string;
}

/** 搜索索引的加载阶段，侧栏提示与筛选分支都依赖它。 */
export type SearchIndexPhase = 'idle' | 'loading' | 'ready' | 'unavailable';

export const EMPTY_FILTERS: FilterState = { query: '' };

export function isFilterActive(filters: FilterState): boolean {
  return filters.query.trim().length > 0;
}

/** 筛选签名：变化时让无限网格整体重排。 */
export function filterSignature(filters: FilterState): string {
  return filters.query.trim().toLowerCase();
}

export type SearchField = 'any' | 'name' | 'tag' | 'group' | 'rating';

export interface SearchTerm {
  /** 已折叠（小写、下划线转空格）的检索词。 */
  value: string;
  field: SearchField;
  exclude: boolean;
}

/** 字段前缀别名：中英文都收，写错前缀时整体当普通词处理。 */
const FIELD_ALIASES: Record<string, SearchField> = {
  tag: 'tag',
  tags: 'tag',
  '标签': 'tag',
  name: 'name',
  n: 'name',
  '文件名': 'name',
  group: 'group',
  g: 'group',
  '分组': 'group',
  rating: 'rating',
  r: 'rating',
  '分级': 'rating',
};

/**
 * 折叠成匹配用的形状：小写、`_` 转空格、压缩空白。
 * `long_hair` 与 `long hair` 因此等价，标签与文件名共用同一套规则。
 */
function fold(value: string): string {
  return value.toLowerCase().replace(/_+/g, ' ').replace(/\s+/g, ' ').trim();
}

function unquote(value: string): string {
  if (value.length >= 2 && value.startsWith('"') && value.endsWith('"')) {
    return value.slice(1, -1);
  }
  return value;
}

/** 按空白切词，但保留引号内的空格（`tag:"two words"`）。 */
function splitTerms(input: string): string[] {
  const out: string[] = [];
  let current = '';
  let quoted = false;
  for (const char of input) {
    if (char === '"') {
      quoted = !quoted;
      current += char;
      continue;
    }
    if (!quoted && /\s/.test(char)) {
      if (current.length > 0) out.push(current);
      current = '';
      continue;
    }
    current += char;
  }
  if (current.length > 0) out.push(current);
  return out;
}

function parseTerm(token: string): SearchTerm | null {
  let rest = token;
  let exclude = false;
  if (rest.startsWith('-') && rest.length > 1) {
    exclude = true;
    rest = rest.slice(1);
  }
  let field: SearchField = 'any';
  const colon = rest.search(/[:：]/);
  if (colon > 0) {
    const alias = FIELD_ALIASES[rest.slice(0, colon).toLowerCase()];
    if (alias) {
      field = alias;
      rest = rest.slice(colon + 1);
    }
  }
  const value = fold(unquote(rest));
  if (value.length === 0) return null;
  return { value, field, exclude };
}

/** 解析查询串：空格 AND、`-词` 排除、`tag:` / `name:` / `group:` / `rating:` 限定字段。 */
export function parseQuery(query: string): SearchTerm[] {
  const terms: SearchTerm[] = [];
  for (const token of splitTerms(query.trim())) {
    const term = parseTerm(token);
    if (term) terms.push(term);
  }
  return terms;
}

function ratingText(raw: string): string {
  return fold(RATING_LABELS[raw.toUpperCase()] ?? '');
}

function haystackOf(
  name: string,
  tags: readonly string[],
  group: string,
  rating: string,
): string {
  return [name, tags.join(' '), group, rating, ratingText(rating)].filter(Boolean).join(' ');
}

function matchesTerm(term: SearchTerm, value: string): boolean {
  return value.includes(term.value);
}

function matchesItem(
  item: WallItem,
  record: SearchRecord | undefined,
  terms: readonly SearchTerm[],
): boolean {
  const name = record ? record.name : fold(item.n);
  const tags = record ? record.tags : item.tags.map((tag) => fold(tag[0]));
  const group = record ? record.group : fold(item.group ?? '');
  const rating = record ? record.rating : fold(item.rt);
  const hay = record ? record.hay : haystackOf(name, tags, group, rating);
  for (const term of terms) {
    let hit: boolean;
    switch (term.field) {
      case 'tag':
        hit = tags.some((tag) => matchesTerm(term, tag));
        break;
      case 'name':
        hit = matchesTerm(term, name);
        break;
      case 'group':
        hit = matchesTerm(term, group);
        break;
      case 'rating':
        hit = matchesTerm(term, rating) || matchesTerm(term, ratingText(rating));
        break;
      default:
        hit = matchesTerm(term, hay);
    }
    if (hit === term.exclude) return false;
  }
  return true;
}

/**
 * 先按当前路由收窄，再按搜索词过滤。`records` 为 null 时说明索引不可用，
 * 保持旧的文件名子串搜索；索引就绪后名称/标签/分组/分级一起参与。
 */
export function filterItems(
  items: readonly WallItem[],
  filters: FilterState,
  records: ReadonlyMap<string, SearchRecord> | null = null,
): WallItem[] {
  const raw = filters.query.trim();
  if (raw.length === 0) return [...items];
  const terms = parseQuery(raw);
  if (terms.length === 0) return [...items];
  if (!records) {
    const needle = raw.toLowerCase();
    return items.filter((item) => item.n.toLowerCase().includes(needle));
  }
  return items.filter((item) => matchesItem(item, records.get(item.id), terms));
}
