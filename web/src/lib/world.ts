import type { WallItem } from '../source/types';

export interface TileRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface LaidTile {
  item: WallItem;
  index: number;
  rect: TileRect;
}

export interface WorldOptions {
  /** 世界宽度（像素）。比视口宽才有横向可平移的空间。 */
  width: number;
  gap: number;
  /** 目标行高；行会被拉伸/压缩到宽度刚好填满。 */
  targetHeight: number;
  /**
   * 末尾那条没排满的行要不要进布局。默认 true。
   *
   * 置 false 时它被"暂扣"：世界高度只算完整的行，不满的那一行等下一片到达、
   * 排满之后再一次性出现。代价是底部最多少一行（正好落在 onLoadMore 的预取
   * 盲区里，看不到）；换来的是**已经渲染出来的卡片坐标永远不变**——追加分片
   * 时不会出现"已看内容抖一下"。
   */
  includePartialRow?: boolean;
  /**
   * 末尾不满的那一行是否拉伸到整宽（默认 false，按 targetHeight 自然摆放并左对齐）。
   *
   * 只有确定"不会再有条目追加进来"（最后一片已到 / 标签墙）时才该置 true：
   * 拉伸会重新分配这一行里每张卡的宽度，只要还会追加，整行就会二次跳动。
   */
  stretchPartialRow?: boolean;
}

/**
 * 把条目排成一行行刚好填满世界宽度的"书脊"布局（justified rows）。
 * 纯函数：同样的条目顺序 + 同样的选项永远得到同样的坐标，虚拟化可以直接按 y 二分。
 */
export function layoutWorld(
  items: readonly WallItem[],
  options: WorldOptions,
): { tiles: LaidTile[]; height: number } {
  const { width, gap, targetHeight, includePartialRow = true, stretchPartialRow = false } = options;
  const tiles: LaidTile[] = [];
  let row: { item: WallItem; index: number; ratio: number }[] = [];
  let ratioSum = 0;
  let y = 0;

  const flush = (stretch: boolean) => {
    if (row.length === 0) return;
    const available = width - gap * (row.length - 1);
    const raw = available / ratioSum;
    // 满行：夹在 target×0.72–1.6 之后再看能不能拉满；
    // 不满的行：不拉伸，固定用 targetHeight（追加条目时宽度/高度都不变）。
    //
    // 关键：上限必须先和 raw 取小。窄屏上 target×0.72 可能比"刚好填满"还大
    // （390 宽、三张不齐比例的卡），硬夹上去会让整行比世界宽，最右一张被画布裁掉。
    const upper = Math.min(targetHeight * 1.6, raw);
    const rowHeight = stretch
      ? Math.min(raw, Math.max(targetHeight * 0.72, upper))
      : targetHeight;
    const totalWidth = rowHeight * ratioSum + gap * (row.length - 1);
    // 不拉伸时左对齐：居中会让追加一张卡就整行重新对中，等于抖动。
    let x = stretch ? Math.max(0, (width - totalWidth) / 2) : 0;
    for (const entry of row) {
      const tileWidth = rowHeight * entry.ratio;
      tiles.push({
        item: entry.item,
        index: entry.index,
        rect: { x, y, w: tileWidth, h: rowHeight },
      });
      x += tileWidth + gap;
    }
    y += rowHeight + gap;
    row = [];
    ratioSum = 0;
  };

  items.forEach((item, index) => {
    const ratio = item.w > 0 && item.h > 0 ? item.w / item.h : 1;
    row.push({ item, index, ratio });
    ratioSum += ratio;
    if (ratioSum * targetHeight + gap * (row.length - 1) >= width) flush(true);
  });

  if (row.length > 0 && includePartialRow) flush(stretchPartialRow);

  return { tiles, height: Math.max(0, y - gap) };
}

/**
 * 卡片标题的字号分级：按卡宽映射，不再用 vw——画布上的卡宽和视口宽度无关
 * （同一屏里既有 160px 的竖图也有 700px 的横图）。
 * 四档，够用且不会出现"同一行两种字号跳来跳去"以外的惊喜。
 */
export function titleSizeForWidth(width: number): number {
  if (!Number.isFinite(width) || width <= 0) return 13;
  if (width < 220) return 13;
  if (width < 340) return 15;
  if (width < 520) return 18;
  if (width < 760) return 22;
  return 26;
}

/** 按 y 二分出可见区间（含 overscan），避免每帧线性扫 1 万条。 */
export function visibleSlice(
  tiles: readonly LaidTile[],
  top: number,
  bottom: number,
  overscan = 600,
): { start: number; end: number } {
  const minY = top - overscan;
  const maxY = bottom + overscan;
  let low = 0;
  let high = tiles.length;
  while (low < high) {
    const mid = (low + high) >> 1;
    if (tiles[mid].rect.y + tiles[mid].rect.h < minY) low = mid + 1;
    else high = mid;
  }
  const start = low;
  high = tiles.length;
  let scanEnd = start;
  while (scanEnd < high && tiles[scanEnd].rect.y <= maxY) scanEnd += 1;
  return { start, end: scanEnd };
}

/* ===== 无限空间：格子（cell）按需生成 =====
 *
 * §1.1 要求"世界无尺寸、按环带取格子、同一 (种子, 格子索引) 恒得到同一批图"。
 * 这里把空间切成 cellWidth × cellHeight 的格子：格子里排 2–4 行（行高由种子决定），
 * 每行从左到右取连续的条目，宽度不够就补图片、富余的宽度摊进列间距（不拉伸图片）。
 * 每个格子自己正好填满自己的盒子，所以环带之间不会重叠也不会留白带。
 */

export interface CellSpec {
  items: readonly WallItem[];
  /** 世界种子：决定每个格子拿哪批图。 */
  seed: number;
  cellWidth: number;
  cellHeight: number;
  gap: number;
}

export interface PlacedTile {
  item: WallItem;
  /** 在 items 里的下标（打开灯箱要用）。 */
  index: number;
  /** 世界坐标（绝对，不受 rebase 影响）。 */
  rect: TileRect;
  /** 所属格子，用于 key 与波次计算。 */
  cell: { b: number; c: number };
}

/** 32 位整数混合（FNV-1a 变体 + 乘法抖动），给每个格子一个稳定种子。 */
function mix32(seed: number, parts: number[]): number {
  let hash = seed >>> 0;
  for (const part of parts) {
    hash ^= part >>> 0;
    hash = Math.imul(hash, 0x01000193) >>> 0;
    hash ^= hash >>> 15;
    hash = Math.imul(hash, 0x27220a95) >>> 0;
  }
  return hash >>> 0;
}

/** 确定性伪随机：同一 seed 永远同样的序列。 */
export function mulberry32(seed: number): () => number {
  let state = seed >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** 排一个格子。b/c 可以是负数（往左上走），坐标直接由格号算出来。 */
export function layoutCell(spec: CellSpec, b: number, c: number): PlacedTile[] {
  const total = spec.items.length;
  if (total === 0) return [];
  const rng = mulberry32(mix32(spec.seed, [b, c]));
  const rows = 2 + Math.floor(rng() * 3);
  const innerWidth = Math.max(1, spec.cellWidth - spec.gap);
  const innerHeight = Math.max(1, spec.cellHeight - spec.gap);
  const rowGap = spec.gap;
  // 行高权重在 0.72–1.42 之间浮动：原来只有 0.82–1.18，几行几乎等高，整屏是等宽横带；
  // 拉到 0.55–1.75 又太狠（一行能占半格，画面只剩两三张巨图）。这里是折中：
  // 一眼能看出大小行交错，但每行仍能放 3–4 张（2026-11-09）。
  const weights = Array.from({ length: rows }, () => 0.72 + rng() * 0.7);
  const weightSum = weights.reduce((sum, value) => sum + value, 0);
  const usable = innerHeight - rowGap * (rows - 1);

  const tiles: PlacedTile[] = [];
  let worldY = b * spec.cellHeight;
  for (let row = 0; row < rows; row += 1) {
    const rowHeight = (usable * weights[row]) / weightSum;
    const start = Math.floor(rng() * total);
    // 先按"加进来还不超宽"取图；一张都放不下时退回单张并压到整宽。
    const picked: { index: number; width: number }[] = [];
    let used = 0;
    for (let step = 0; step < total; step += 1) {
      const index = (start + step) % total;
      const entry = spec.items[index];
      const ratio = entry.w > 0 && entry.h > 0 ? entry.w / entry.h : 1;
      const width = rowHeight * ratio;
      if (picked.length > 0 && used + rowGap + width > innerWidth) break;
      if (picked.length === 0 && width > innerWidth) {
        picked.push({ index, width: innerWidth });
        used = innerWidth;
        break;
      }
      picked.push({ index, width });
      used += width + (picked.length > 1 ? rowGap : 0);
      if (used >= innerWidth) break;
    }
    if (picked.length === 0) continue;
    // 富余宽度摊进列间距（单张图时居中），图片一律不拉伸。
    const leftover = Math.max(0, innerWidth - used);
    const extraGap = picked.length > 1 ? leftover / (picked.length - 1) : 0;
    let x = c * spec.cellWidth + (picked.length === 1 ? leftover / 2 : 0);
    for (const entry of picked) {
      tiles.push({
        item: spec.items[entry.index],
        index: entry.index,
        rect: { x, y: worldY, w: entry.width, h: rowHeight },
        cell: { b, c },
      });
      x += entry.width + rowGap + extraGap;
    }
    worldY += rowHeight + rowGap;
  }
  return tiles;
}

/** 视野 + 缓冲里所有格子的卡（按世界坐标过滤），格子本身在 DOM 里不落地。 */
export function cellTilesInView(
  spec: CellSpec,
  view: { x: number; y: number; w: number; h: number },
  padding = 420,
): PlacedTile[] {
  const left = view.x - padding;
  const right = view.x + view.w + padding;
  const top = view.y - padding;
  const bottom = view.y + view.h + padding;
  const c0 = Math.floor(left / spec.cellWidth);
  const c1 = Math.floor(right / spec.cellWidth);
  const b0 = Math.floor(top / spec.cellHeight);
  const b1 = Math.floor(bottom / spec.cellHeight);
  const tiles: PlacedTile[] = [];
  for (let b = b0; b <= b1; b += 1) {
    for (let c = c0; c <= c1; c += 1) {
      for (const tile of layoutCell(spec, b, c)) {
        if (tile.rect.x + tile.rect.w < left || tile.rect.x > right) continue;
        if (tile.rect.y + tile.rect.h < top || tile.rect.y > bottom) continue;
        tiles.push(tile);
      }
    }
  }
  return tiles;
}
