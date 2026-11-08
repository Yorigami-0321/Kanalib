import { dataUrl } from '../source/config';
import type { TierPaths, WallItem } from '../source/types';

/** 档位前缀 + id + 扩展名 → 可 fetch 的 URL（相对数据根，不拼主机名）。 */
export function tierUrl(paths: TierPaths, tier: 't256' | 't512' | 't1024', id: string): string {
  return dataUrl(`${paths[tier]}${id}.jpg`);
}

/** 原图 URL；`orig` 缺失时返回 null（契约 §4：缺省表示该条无原图）。 */
export function originalUrl(paths: TierPaths, item: WallItem): string | null {
  if (!item.orig) return null;
  return dataUrl(`${paths.original}${item.id}.${item.orig}`);
}

/** 网格图：t256 优先，404 时回退 t512（契约 §6.2）。 */
export function gridSources(paths: TierPaths, item: WallItem): string[] {
  return [tierUrl(paths, 't256', item.id), tierUrl(paths, 't512', item.id)];
}

/** 详情页大图：有原图用原图，否则 t512（契约 §6.2）。 */
export function detailSources(paths: TierPaths, item: WallItem): string[] {
  const original = originalUrl(paths, item);
  return original ? [original, tierUrl(paths, 't512', item.id)] : [tierUrl(paths, 't512', item.id)];
}

/** 按长边缩放尺寸（给 PhotoSwipe 报 width/height，避免小图被放大到糊）。 */
export function fitDims(w: number, h: number, longEdge: number): { width: number; height: number } {
  const longest = Math.max(w, h);
  if (!Number.isFinite(longest) || longest <= 0) return { width: longEdge, height: longEdge };
  if (longest <= longEdge) return { width: Math.round(w), height: Math.round(h) };
  const scale = longEdge / longest;
  return { width: Math.max(1, Math.round(w * scale)), height: Math.max(1, Math.round(h * scale)) };
}
