import { DEFAULT_TIER_PATHS, type TierPaths } from './types';

/**
 * 数据根：默认 `./data/`（相对站点根），可被构建期环境变量 `VITE_WALL_DATA` 覆盖。
 * 既可以是子路径（`/wall-data/`、`../data/`），也可以是完整 URL（CDN 场景）。
 * 契约 §1：不得拼出绝对主机名——所以这里只做拼接，不做域名假设。
 */
function normalizeRoot(root: string): string {
  const trimmed = root.trim();
  const withSlash = trimmed.endsWith('/') ? trimmed : `${trimmed}/`;
  return withSlash.replace(/\\/g, '/');
}

const RAW = (import.meta.env.VITE_WALL_DATA as string | undefined) ?? '';

export const DATA_ROOT = normalizeRoot(RAW.trim().length > 0 ? RAW : './data/');

/**
 * 动态后端根：默认 `/api/`，可由 `VITE_WALL_API` 覆盖（反代子路径或完整 URL）。
 */
const RAW_API = (import.meta.env.VITE_WALL_API as string | undefined) ?? '';

export const API_ROOT = normalizeRoot(RAW_API.trim().length > 0 ? RAW_API : '/api/');

/** 构建期数据源选择：`static`（默认）或 `api`。 */
export type SourceKind = 'static' | 'api';

export const SOURCE_KIND: SourceKind =
  ((import.meta.env.VITE_WALL_SOURCE as string | undefined) ?? '').trim().toLowerCase() === 'api'
    ? 'api'
    : 'static';

/** 服务端分页大小；与 `kanalib-web` 的默认 limit 对齐。 */
export const API_PAGE_SIZE = 60;

/** 把数据根下的相对路径解析成可 fetch 的 URL（相对 `document.baseURI`，hash 路由不干扰）。 */
export function dataUrl(rel: string): string {
  const base = typeof document !== 'undefined' ? document.baseURI : 'http://localhost/';
  try {
    return new URL(rel, new URL(DATA_ROOT, base)).href;
  } catch {
    return `${DATA_ROOT}${rel}`;
  }
}

export function apiUrl(rel: string): string {
  const base = typeof document !== 'undefined' ? document.baseURI : 'http://localhost/';
  try {
    return new URL(rel, new URL(API_ROOT, base)).href;
  } catch {
    return `${API_ROOT}${rel}`;
  }
}

/** `index.paths` 为空/缺键时的兜底（契约 §2 `paths` 说明）。 */
export function mergePaths(paths: Partial<TierPaths> | undefined): TierPaths {
  const merged: TierPaths = { ...DEFAULT_TIER_PATHS };
  if (paths) {
    for (const tier of Object.keys(DEFAULT_TIER_PATHS) as (keyof TierPaths)[]) {
      const value = paths[tier];
      if (typeof value === 'string' && value.length > 0) {
        merged[tier] = value.endsWith('/') ? value : `${value}/`;
      }
    }
  }
  return merged;
}
