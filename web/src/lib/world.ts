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
/** 横向填满一条：连取若干张，最后按比例缩放到正好铺满 box.w（cover 裁切，不显变形）。 */
function pickStrip(
  items: readonly WallItem[],
  rng: () => number,
  box: { x: number; y: number; w: number; h: number },
  gap: number,
  cell: { b: number; c: number },
  used: Set<number>,
  out: PlacedTile[],
): void {
  const total = items.length;
  const start = Math.floor(rng() * total);
  const picked: { index: number; ratio: number }[] = [];
  let ratioSum = 0;
  // 太窄的卡放不下标题（390 屏上一张竖图可能只有 60px 宽），宁可跳过换下一张；
  // 一张都没挑到时再退回"不限宽度"的兜底，保证行不会空着。
  const minWidth = Math.min(box.w * 0.45, 96);
  for (let step = 0; step < total && picked.length < 8; step += 1) {
    const index = (start + step) % total;
    if (used.has(index)) continue;
    const entry = items[index];
    const ratio = entry.w > 0 && entry.h > 0 ? entry.w / entry.h : 1;
    if (picked.length > 0 && ratio * box.h < minWidth) continue;
    picked.push({ index, ratio });
    ratioSum += ratio;
    const natural = ratioSum * box.h;
    const gaps = gap * (picked.length - 1);
    if (natural + gaps >= box.w) break;
    // 剩下的宽度要拉伸 35% 以上才填得满？再补一张更划算。
    if ((box.w - gaps) / natural <= 1.35) break;
  }
  if (picked.length === 0) return;
  const gaps = gap * (picked.length - 1);
  const scale = (box.w - gaps) / Math.max(1, ratioSum * box.h);
  let x = box.x;
  for (const entry of picked) {
    const width = entry.ratio * box.h * scale;
    used.add(entry.index);
    out.push({
      item: items[entry.index],
      index: entry.index,
      rect: { x, y: box.y, w: width, h: box.h },
      cell,
    });
    x += width + gap;
  }
}

/** 竖向填满一列：宽度固定为 box.w，高度按自然比例分摊后缩放到正好铺满 box.h。 */
function pickColumn(
  items: readonly WallItem[],
  rng: () => number,
  box: { x: number; y: number; w: number; h: number },
  gap: number,
  cell: { b: number; c: number },
  used: Set<number>,
  out: PlacedTile[],
): void {
  const total = items.length;
  // 张数跟着列的高宽比走：宽扁的列放 1–2 张，别切成一堆 570×110 的宽扁条。
  const aspect = box.h / Math.max(1, box.w * 0.78);
  const wanted = Math.max(1, Math.min(4, Math.round(aspect)));
  const start = Math.floor(rng() * total);
  const picked: { index: number; ratio: number }[] = [];
  for (let step = 0; step < total && picked.length < wanted; step += 1) {
    const index = (start + step) % total;
    if (used.has(index)) continue;
    const entry = items[index];
    picked.push({ index, ratio: entry.w > 0 && entry.h > 0 ? entry.w / entry.h : 1 });
  }
  if (picked.length === 0) return;
  const natural = picked.reduce((sum, entry) => sum + box.w / entry.ratio, 0);
  const usable = box.h - gap * (picked.length - 1);
  const scale = usable / Math.max(1, natural);
  let y = box.y;
  for (const entry of picked) {
    const height = (box.w / entry.ratio) * scale;
    used.add(entry.index);
    out.push({
      item: items[entry.index],
      index: entry.index,
      rect: { x: box.x, y, w: box.w, h: height },
      cell,
    });
    y += height + gap;
  }
}

/**
 * 排一个格子。b/c 可以是负数（往左上走），坐标直接由格号算出来。
 *
 * 两种排法：
 * - 常规：2–4 条横向满铺，行高由格子种子决定（0.72–1.42 倍权重）。
 * - 跨行（约 42% 的格子、且格子够宽）：左侧一张大图占掉 52–66% 宽、52–66% 高，
 *   右侧竖着排 2–3 张，下方再补 1–2 条横条 —— 这是"格位感"的来源：
 *   每走几步就有一张主图把整片节奏打断，而不是永远一条条等宽横带。
 */
export function layoutCell(spec: CellSpec, b: number, c: number): PlacedTile[] {
  const total = spec.items.length;
  if (total === 0) return [];
  const rng = mulberry32(mix32(spec.seed, [b, c]));
  const innerWidth = Math.max(1, spec.cellWidth - spec.gap);
  const innerHeight = Math.max(1, spec.cellHeight - spec.gap);
  const rowGap = spec.gap;
  const originX = c * spec.cellWidth;
  const originY = b * spec.cellHeight;
  const cell = { b, c };
  const used = new Set<number>();
  const tiles: PlacedTile[] = [];

  // 窄格子（390 屏）放不下"主图 + 侧栏"，直接走常规排法。
  const wantFeature = innerWidth >= 620 && rng() < 0.3;
  if (wantFeature) {
    const heroWidth = Math.round(innerWidth * (0.38 + rng() * 0.12));
    const heroHeight = Math.round(innerHeight * (0.42 + rng() * 0.13));
    // 主图挑一张比例最接近这个框的，少裁一点。
    const target = heroWidth / heroHeight;
    let heroIndex = -1;
    let bestDelta = Infinity;
    for (let probe = 0; probe < 8; probe += 1) {
      const index = Math.floor(rng() * total);
      const entry = spec.items[index];
      const ratio = entry.w > 0 && entry.h > 0 ? entry.w / entry.h : 1;
      const delta = Math.abs(Math.log(ratio / target));
      if (delta < bestDelta) {
        bestDelta = delta;
        heroIndex = index;
      }
    }
    if (heroIndex >= 0) {
      used.add(heroIndex);
      tiles.push({
        item: spec.items[heroIndex],
        index: heroIndex,
        rect: { x: originX, y: originY, w: heroWidth, h: heroHeight },
        cell,
      });
      const rightX = originX + heroWidth + rowGap;
      pickColumn(
        spec.items,
        rng,
        { x: rightX, y: originY, w: innerWidth - heroWidth - rowGap, h: heroHeight },
        rowGap,
        cell,
        used,
        tiles,
      );
      const bottomY = originY + heroHeight + rowGap;
      const bottomH = innerHeight - heroHeight - rowGap;
      // 下方按 55/45 拆两条（不够高就只留一条）。
      if (bottomH > rowGap * 3) {
        const first = Math.round((bottomH - rowGap) * 0.55);
        pickStrip(spec.items, rng, { x: originX, y: bottomY, w: innerWidth, h: first }, rowGap, cell, used, tiles);
        pickStrip(
          spec.items,
          rng,
          { x: originX, y: bottomY + first + rowGap, w: innerWidth, h: bottomH - first - rowGap },
          rowGap,
          cell,
          used,
          tiles,
        );
      } else {
        pickStrip(spec.items, rng, { x: originX, y: bottomY, w: innerWidth, h: bottomH }, rowGap, cell, used, tiles);
      }
      return tiles;
    }
  }

  // 常规：2–4 条横向满铺。
  const rows = 2 + Math.floor(rng() * 3);
  const weights = Array.from({ length: rows }, () => 0.72 + rng() * 0.7);
  const weightSum = weights.reduce((sum, value) => sum + value, 0);
  const usable = innerHeight - rowGap * (rows - 1);
  let worldY = originY;
  for (let row = 0; row < rows; row += 1) {
    const rowHeight = (usable * weights[row]) / weightSum;
    pickStrip(spec.items, rng, { x: originX, y: worldY, w: innerWidth, h: rowHeight }, rowGap, cell, used, tiles);
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

/* ===== v4 单击聚焦：原位重排（不是浮层缩放） =====
 *
 * 用户要的是"点开一张，周围的图被挤开，所有图仍在同一层"。
 * - 无限画布：重排被点中的那个格子（格子边界不变，邻格不动），英雄卡占左上大块，
 *   右边一列 + 底部一片网格装下该格其余条目；
 * - 有限墙：英雄卡放在世界顶部的大块，前几张其余图放右侧列，剩下的顺延到下方行。
 */

interface GridEntry {
  item: WallItem;
  index: number;
}

function gridRects(
  entries: readonly GridEntry[],
  box: { x: number; y: number; w: number; h: number },
  columns: number,
  gap: number,
): { item: WallItem; index: number; rect: TileRect }[] {
  const count = entries.length;
  if (count === 0) return [];
  const cols = Math.max(1, Math.min(columns, count));
  const rows = Math.ceil(count / cols);
  const cellW = Math.max(1, (box.w - gap * (cols - 1)) / cols);
  const cellH = Math.max(1, (box.h - gap * (rows - 1)) / rows);
  return entries.map((entry, position) => {
    const row = Math.floor(position / cols);
    const col = position % cols;
    return {
      item: entry.item,
      index: entry.index,
      rect: {
        x: box.x + col * (cellW + gap),
        y: box.y + row * (cellH + gap),
        w: cellW,
        h: cellH,
      },
    };
  });
}

/** 重排一个格子：hero 占左上大块，右侧列 + 底部网格装下同格其余条目。 */
export function layoutSpotlightCell(spec: CellSpec, b: number, c: number, heroId: string): PlacedTile[] {
  const normal = layoutCell(spec, b, c);
  const hero = normal.find((tile) => tile.item.id === heroId);
  if (!hero || normal.length <= 1) return normal;

  const gap = spec.gap;
  const innerW = Math.max(1, spec.cellWidth - gap);
  const innerH = Math.max(1, spec.cellHeight - gap);
  const originX = c * spec.cellWidth;
  const originY = b * spec.cellHeight;
  const cell = { b, c };
  const ratio = hero.item.w > 0 && hero.item.h > 0 ? hero.item.w / hero.item.h : 1;
  const narrow = innerW < 620;

  let heroW: number;
  let heroH: number;
  if (narrow) {
    // 窄格子：英雄整行，底下用 2–3 列网格装其余图。
    heroW = innerW;
    heroH = Math.min(innerH * 0.62, Math.max(innerH * 0.42, innerW / ratio));
  } else {
    const maxW = innerW * 0.62;
    const maxH = innerH * 0.7;
    heroW = maxW;
    heroH = maxW / ratio;
    if (heroH > maxH) {
      heroH = maxH;
      heroW = maxH * ratio;
    }
    // 至少给右侧列/底部留出 140/110px，不然周围图会缩成细条。
    heroW = Math.min(heroW, innerW - gap - 140);
    heroH = Math.min(heroH, innerH - gap - 110);
    heroW = Math.max(innerW * 0.46, heroW);
    heroH = Math.max(innerH * 0.46, heroH);
  }

  const out: PlacedTile[] = [
    { item: hero.item, index: hero.index, rect: { x: originX, y: originY, w: heroW, h: heroH }, cell },
  ];
  const rest = normal
    .filter((tile) => tile.item.id !== heroId)
    .map((tile) => ({ item: tile.item, index: tile.index }));

  if (narrow) {
    const bottom = { x: originX, y: originY + heroH + gap, w: innerW, h: Math.max(1, innerH - heroH - gap) };
    const cols = Math.max(2, Math.min(4, Math.ceil(rest.length / 2)));
    for (const entry of gridRects(rest, bottom, cols, gap)) {
      out.push({ ...entry, cell });
    }
    return out;
  }

  const rightCount = Math.min(rest.length, Math.max(2, Math.round(rest.length * 0.35)));
  const right = rest.slice(0, rightCount);
  const bottom = rest.slice(rightCount);
  const rightW = Math.max(1, innerW - heroW - gap);
  const rightBox = { x: originX + heroW + gap, y: originY, w: rightW, h: heroH };
  for (const entry of gridRects(right, rightBox, 1, gap)) {
    out.push({ ...entry, cell });
  }
  const bottomBox = {
    x: originX,
    y: originY + heroH + gap,
    w: innerW,
    h: Math.max(1, innerH - heroH - gap),
  };
  const cols = Math.max(2, Math.min(6, Math.ceil(Math.sqrt(Math.max(1, bottom.length) * (bottomBox.w / bottomBox.h)))));
  for (const entry of gridRects(bottom, bottomBox, cols, gap)) {
    out.push({ ...entry, cell });
  }
  return out;
}

/** 有限墙的聚焦重排：英雄卡居中大块，其余图右侧列 + 下方正常行，全部顺延。 */
export function layoutFeaturedWorld(
  items: readonly WallItem[],
  options: WorldOptions,
  heroId: string,
  focus: { centerX: number; viewHeight: number; bottomInset: number },
): { tiles: LaidTile[]; height: number } {
  const heroIndex = items.findIndex((item) => item.id === heroId);
  if (heroIndex < 0) return layoutWorld(items, options);
  const hero = items[heroIndex];
  const rest: GridEntry[] = items
    .map((item, index) => ({ item, index }))
    .filter((entry) => entry.index !== heroIndex);
  const { width, gap, targetHeight } = options;
  const ratio = hero.w > 0 && hero.h > 0 ? hero.w / hero.h : 1;

  // 英雄尺寸：优先按原图比例，夹到世界宽 58% / 2.3 行高；至少给右侧列留 180px。
  const maxH = targetHeight * 2.3;
  let heroW = Math.min(width * 0.58, maxH * ratio);
  let heroH = heroW / ratio;
  if (heroH > maxH) {
    heroH = maxH;
    heroW = maxH * ratio;
  }
  // 相机会把英雄移到内容区中心；世界从 x=0 起，英雄必须至少居中到 centerX。
  let heroX = Math.max(0, Math.round(focus.centerX - heroW / 2));
  heroW = Math.min(heroW, width - heroX);
  const rightW = width - heroX - heroW - gap;
  const useRightColumn = rightW >= 180;

  const rightItems = useRightColumn ? rest.slice(0, Math.min(rest.length, 4)) : [];
  const belowItems = rest.slice(rightItems.length);
  // 顶部留白让英雄在无 bottom pad 的有限相机(>=0)里也能落在可见区中心。
  const visibleHeight = Math.max(200, focus.viewHeight - focus.bottomInset);
  const topPad = Math.max(0, Math.round(visibleHeight / 2 - heroH / 2));

  const tiles: LaidTile[] = [
    {
      item: hero,
      index: heroIndex,
      rect: { x: heroX, y: topPad, w: heroW, h: heroH },
    },
  ];
  if (useRightColumn) {
    const rightBox = { x: heroX + heroW + gap, y: topPad, w: Math.max(1, rightW), h: heroH };
    for (const entry of gridRects(rightItems, rightBox, 1, gap)) {
      tiles.push({ ...entry });
    }
  }

  const belowOffset = topPad + heroH + gap;
  if (belowItems.length > 0) {
    const below = layoutWorld(
      belowItems.map((entry) => entry.item),
      {
        ...options,
        width,
        targetHeight,
        includePartialRow: true,
        stretchPartialRow: true,
      },
    );
    for (const tile of below.tiles) {
      const entry = belowItems[tile.index];
      tiles.push({
        item: entry.item,
        index: entry.index,
        rect: { ...tile.rect, y: tile.rect.y + belowOffset },
      });
    }
    return { tiles, height: belowOffset + below.height };
  }
  return { tiles, height: topPad + heroH };
}
