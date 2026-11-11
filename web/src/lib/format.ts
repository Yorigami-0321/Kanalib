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
  return RATING_LABELS[rating] ?? (rating || '未分级');
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
  style: '风格',
  meta: '元信息',
  rating: '分级',
  other: '其他',
};

export function categoryLabel(category: string): string {
  return CATEGORY_LABELS[category] ?? category;
}

export function truncateMiddle(value: string, head = 8, tail = 6): string {
  if (value.length <= head + tail + 1) return value;
  return `${value.slice(0, head)}…${value.slice(-tail)}`;
}

/** 图片扩展名（只认常见图片后缀，避免把 `v1.2` 这类名字误裁）。 */
const IMAGE_EXTENSION = /\.(?:avif|gif|heic|heif|jpe?g|png|tiff?|webp|bmp)$/i;

/** 长且无分隔的 token 中间省略：哈希、编号、导出串在卡面上只留头尾。 */
function compactToken(token: string): string {
  if (token.length > 24 && /^[0-9a-f]+$/i.test(token)) return truncateMiddle(token);
  if (token.length > 28) return truncateMiddle(token);
  return token;
}

/**
 * 卡面展示名：去图片扩展名、下划线转空格、压缩空白；长哈希/长串中间省略。
 * 完整文件名仍保留在 title/aria-label 与检查器标题里。
 */
export function displayTitle(name: string): string {
  if (!name) return '';
  const base = name.replace(IMAGE_EXTENSION, '').replace(/_+/g, ' ').replace(/\s+/g, ' ').trim();
  if (!base) return name;
  return base.split(' ').map(compactToken).join(' ');
}

/** 标签展示名：把 danbooru 风格的下划线换成空格，点击仍用原始 tag。 */
export function displayTag(name: string): string {
  if (!name) return '';
  return name.replace(/_+/g, ' ').replace(/\s+/g, ' ').trim() || name;
}

/** 分组展示名：`作品: honkai_impact_3rd` → `作品 · honkai impact 3rd`。 */
export function displayGroup(group: string): string {
  if (!group) return '未分组';
  const spaced = group.replace(/_+/g, ' ').replace(/\s+/g, ' ').trim();
  const match = /^([^:：]+)[:：]\s*(.+)$/.exec(spaced);
  if (!match) return spaced || group;
  return `${match[1].trim()} · ${match[2].trim()}`;
}

export function formatCount(value: number): string {
  return value.toLocaleString('zh-CN');
}

/** 面积足够的图给“大图突出”档位用。 */
export function isLargeImage(w: number, h: number): boolean {
  return Math.min(w, h) >= 900;
}
