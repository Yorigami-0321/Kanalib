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

/** 锐利档从高到低；`sharp` 缺省（旧数据）时两档都要试探。 */
const SHARP_TIERS = [
  { edge: 2048, tier: 't2048' },
  { edge: 1024, tier: 't1024' },
] as const;

function availableSharpTiers(item: WallItem): ('t2048' | 't1024')[] {
  const sharp = item.sharp;
  if (sharp === undefined) return SHARP_TIERS.map(({ tier }) => tier);
  return SHARP_TIERS.filter(({ edge }) => sharp >= edge).map(({ tier }) => tier);
}

/**
 * 网格 srcset：按卡片在屏幕上的实际显示宽度选档（相机缩放已经乘进 sizes）。
 * 只列条目声明的档位：旧数据没有 `sharp` 时退回 t256/t512，避免大图白白 404。
 */
export function gridImageAttrs(
  paths: TierPaths,
  item: WallItem,
  displayWidth: number,
): { srcSet: string; sizes: string } {
  const entries = [
    `${tierUrl(paths, 't256', item.id)} 256w`,
    `${tierUrl(paths, 't512', item.id)} 512w`,
  ];
  if (item.sharp !== undefined && item.sharp >= 1024) {
    entries.push(`${tierUrl(paths, 't1024', item.id)} 1024w`);
  }
  if (item.sharp !== undefined && item.sharp >= 2048) {
    entries.push(`${tierUrl(paths, 't2048', item.id)} 2048w`);
  }
  return {
    srcSet: entries.join(', '),
    sizes: `${Math.max(1, Math.round(displayWidth))}px`,
  };
}

/** 聚焦卡：只取 sharp 声明存在的锐利档，最后 t512；不拿未产出的档去 404。 */
export function spotlightSources(paths: TierPaths, item: WallItem): string[] {
  const sources: string[] = [];
  const sharp = item.sharp;
  if (sharp !== undefined && sharp >= 2048) sources.push(tierUrl(paths, 't2048', item.id));
  if (sharp !== undefined && sharp >= 1024) sources.push(tierUrl(paths, 't1024', item.id));
  sources.push(tierUrl(paths, 't512', item.id));
  return sources;
}

/** 详情页大图：原图 → 声明的锐利档（高→低）→ t512；旧数据缺 sharp 时逐档试探。 */
export function detailSources(paths: TierPaths, item: WallItem): string[] {
  const sources: string[] = [];
  const original = originalUrl(paths, item);
  if (original) sources.push(original);
  for (const tier of availableSharpTiers(item)) sources.push(tierUrl(paths, tier, item.id));
  sources.push(tierUrl(paths, 't512', item.id));
  return sources;
}
