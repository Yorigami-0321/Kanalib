import { dataUrl } from '../source/config';
import type { TierPaths, WallItem } from '../source/types';

/** 档位前缀 + id + 扩展名 → 可 fetch 的 URL（相对数据根，不拼主机名）。 */
export function tierUrl(
  paths: TierPaths,
  tier: 't256' | 't512' | 't1024' | 't2048',
  id: string,
): string {
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

/**
 * 网格 srcset：按卡片在屏幕上的实际显示宽度选档（相机缩放已经乘进 sizes）。
 * 只列有硬保证的两档——t1024 是可选产出，混进来会让没生成的条目白跑一次 404。
 */
export function gridImageAttrs(
  paths: TierPaths,
  item: WallItem,
  displayWidth: number,
): { srcSet: string; sizes: string } {
  return {
    srcSet: `${tierUrl(paths, 't256', item.id)} 256w, ${tierUrl(paths, 't512', item.id)} 512w`,
    sizes: `${Math.max(1, Math.round(displayWidth))}px`,
  };
}

/** 详情页大图：原图 → t2048 → t1024 → t512；缺档由 onError 逐级回退（契约 §6.2）。 */
export function detailSources(paths: TierPaths, item: WallItem): string[] {
  const sources: string[] = [];
  const original = originalUrl(paths, item);
  if (original) sources.push(original);
  sources.push(tierUrl(paths, 't2048', item.id));
  sources.push(tierUrl(paths, 't1024', item.id));
  sources.push(tierUrl(paths, 't512', item.id));
  return sources;
}
