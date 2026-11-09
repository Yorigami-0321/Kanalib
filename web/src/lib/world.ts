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
