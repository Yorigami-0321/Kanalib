/** `round` 版本的取整，供尺寸/时间显示。 */
export function formatSize(w: number, h: number): string {
  const gcd = (a: number, b: number): number => (b === 0 ? a : gcd(b, a % b));
  const left = Math.round(w);
  const right = Math.round(h);
  if (left <= 0 || right <= 0) return '—';
  const divisor = gcd(left, right) || 1;
  return `${left} × ${right}（${left / divisor}:${right / divisor}）`;
}

export function formatDateTime(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return '—';
  const date = new Date(seconds * 1000);
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

export function formatStars(stars: number): string {
  const count = Math.min(5, Math.max(0, Math.round(stars)));
  return count === 0 ? '未评级' : '★'.repeat(count);
}

export const RATING_LABELS: Record<string, string> = {
  G: 'G 一般',
  S: 'S 敏感',
  Q: 'Q 存疑',
  E: 'E 限制',
};

export function ratingLabel(rating: string): string {
  return RATING_LABELS[rating] ?? rating;
}

/**
 * 契约 §2 的类别取值；用于标签 chips 着色与标签总览的分组标题。
 * 顺序与 TagGallery 的 CATEGORY_ORDER 一致（general/character/copyright/artist/manual/meta/rating），
 * 缺项会回落成英文原值（D3：rating 之前就漏在这里，标签总览第四段显示成了 "rating"）。
 */
export const CATEGORY_LABELS: Record<string, string> = {
  general: '通用',
  character: '角色',
  copyright: '作品',
  artist: '画师',
  manual: '手动',
  meta: '元信息',
  rating: '分级',
};

export function categoryLabel(category: string): string {
  return CATEGORY_LABELS[category] ?? category;
}

export function truncateMiddle(value: string, head = 8, tail = 6): string {
  if (value.length <= head + tail + 1) return value;
  return `${value.slice(0, head)}…${value.slice(-tail)}`;
}

export function formatCount(value: number): string {
  return value.toLocaleString('zh-CN');
}

/** 面积足够的图给“大图突出”档位用。 */
export function isLargeImage(w: number, h: number): boolean {
  return Math.min(w, h) >= 900;
}
