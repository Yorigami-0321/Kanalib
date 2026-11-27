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

export interface WorldRow {
  /** 行首下标（含）。 */
  start: number;
  /** 行尾下标（不含）。 */
  end: number;
  y: number;
  height: number;
  ratioSum: number;
  /** 满行/拉伸尾行居中；false 表示固定行高、左对齐。 */
  stretch: boolean;
}

/** 只含行几何、不含每张卡的 rect：大列表换筛选/排序时先算这个。 */
export interface WorldGeometry {
  rows: WorldRow[];
  height: number;
}

function ratioOf(item: WallItem): number {
  return item.w > 0 && item.h > 0 ? item.w / item.h : 1;
}

/** 满行高度：夹在 target×0.72–1.6 之后再看能不能拉满。 */
function justifiedHeight(
  ratioSum: number,
  count: number,
  width: number,
  gap: number,
  targetHeight: number,
): number {
  const available = width - gap * (count - 1);
  const raw = available / ratioSum;
  // 上限必须先和 raw 取小。窄屏上 target×0.72 可能比"刚好填满"还大
  // （390 宽、三张不齐比例的卡），硬夹上去会让整行比世界宽，最右一张被裁掉。
  const upper = Math.min(targetHeight * 1.6, raw);
  return Math.min(raw, Math.max(targetHeight * 0.72, upper));
}

/**
 * 扫一遍条目，只记录每行的起止下标与高度。
 * 纯函数：与 layoutWorldRange 组合后，坐标和旧版 layoutWorld 完全一致。
 */
export function measureWorld(
  items: readonly WallItem[],
  options: WorldOptions,
): WorldGeometry {
  const { width, gap, targetHeight, includePartialRow = true, stretchPartialRow = false } = options;
  const rows: WorldRow[] = [];
  let start = 0;
  let ratioSum = 0;
  let count = 0;
  let y = 0;

  for (let index = 0; index < items.length; index += 1) {
    ratioSum += ratioOf(items[index]);
    count += 1;
    if (ratioSum * targetHeight + gap * (count - 1) >= width) {
      const height = justifiedHeight(ratioSum, count, width, gap, targetHeight);
      rows.push({ start, end: index + 1, y, height, ratioSum, stretch: true });
      y += height + gap;
      start = index + 1;
      ratioSum = 0;
      count = 0;
    }
  }

  if (count > 0 && includePartialRow) {
    const stretch = stretchPartialRow;
    const height = stretch
      ? justifiedHeight(ratioSum, count, width, gap, targetHeight)
      : targetHeight;
    rows.push({ start, end: items.length, y, height, ratioSum, stretch });
    y += height + gap;
  }

  return { rows, height: Math.max(0, y - gap) };
}

/** 物化 [from, to) 行；范围外不生成 tile 对象。 */
export function layoutWorldRange(
  items: readonly WallItem[],
  options: WorldOptions,
  geometry: WorldGeometry,
  from: number,
  to: number,
): LaidTile[] {
  const { width, gap } = options;
  const tiles: LaidTile[] = [];
  const first = Math.max(0, from);
  const last = Math.min(geometry.rows.length, to);
  for (let rowIndex = first; rowIndex < last; rowIndex += 1) {
    const row = geometry.rows[rowIndex];
    const count = row.end - row.start;
    const totalWidth = row.height * row.ratioSum + gap * (count - 1);
    // 不拉伸时左对齐：居中会让追加一张卡就整行重新对中，等于抖动。
    let x = row.stretch ? Math.max(0, (width - totalWidth) / 2) : 0;
    for (let index = row.start; index < row.end; index += 1) {
      const item = items[index];
      const tileWidth = row.height * ratioOf(item);
      tiles.push({
        item,
        index,
        rect: { x, y: row.y, w: tileWidth, h: row.height },
      });
      x += tileWidth + gap;
    }
  }
  return tiles;
}

/** 按行 y 二分出可见行区间（含 overscan）。 */
export function worldRowSlice(
  rows: readonly WorldRow[],
  top: number,
  bottom: number,
  overscan = 600,
): { start: number; end: number } {
  const minY = top - overscan;
  const maxY = bottom + overscan;
  let low = 0;
  let high = rows.length;
  while (low < high) {
    const mid = (low + high) >> 1;
    if (rows[mid].y + rows[mid].height < minY) low = mid + 1;
    else high = mid;
  }
  const start = low;
  let end = start;
  while (end < rows.length && rows[end].y <= maxY) end += 1;
  return { start, end };
}

/**
 * 把条目排成一行行刚好填满世界宽度的"书脊"布局（justified rows）。
 * 纯函数：同样的条目顺序 + 同样的选项永远得到同样的坐标，虚拟化可以直接按 y 二分。
 */
export function layoutWorld(
  items: readonly WallItem[],
  options: WorldOptions,
): { tiles: LaidTile[]; height: number } {
  const geometry = measureWorld(items, options);
  return {
    tiles: layoutWorldRange(items, options, geometry, 0, geometry.rows.length),
    height: geometry.height,
  };
}

/**
 * 卡片标题的字号分级：按卡宽映射，不再用 vw——画布上的卡宽和视口宽度无关
 * （同一屏里既有 160px 的竖图也有 700px 的横图）。
 * 四档，够用且不会出现"同一行两种字号跳来跳去"以外的惊喜。
 */
export function titleSizeForWidth(width: number): number {
  if (!Number.isFinite(width) || width <= 0) return 12;
  // 窄卡（390 屏三列约 100px）上 13px 会占掉三分之一卡宽并挤成四行；
  // 分级同时向上延伸，让大卡/聚焦卡有更明显的视觉重量。
  if (width < 120) return 10;
  if (width < 170) return 11;
  if (width < 240) return 12.5;
  if (width < 340) return 15;
  if (width < 520) return 18;
  if (width < 760) return 23;
  return 27;
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
  // 太窄的卡放不下标题（真实库里 390 屏上一张竖图可能只有 60px 宽）。
  // 第一张也必须是宽卡；只有整条行都找不到时才退回“不限宽度”的兜底。
  const minWidth = Math.min(box.w * 0.45, 96);
  for (const enforceWidth of [true, false]) {
    if (picked.length > 0) break;
    // 拉伸比最接近 1 的前缀胜出：旧版固定补到“拉伸 ≤ 1.35”就停，
    // 竖图会被统一拉到 1.3 倍横框（cover 裁掉上下）。这里逐个前缀记录
    // |log(scale)|，越过 1（开始压缩）就停——再多补只会更变形。
    let bestCount = 0;
    let bestScore = Infinity;
    for (let step = 0; step < total && picked.length < 8; step += 1) {
      const index = (start + step) % total;
      if (used.has(index)) continue;
      const entry = items[index];
      const ratio = entry.w > 0 && entry.h > 0 ? entry.w / entry.h : 1;
      if (enforceWidth && ratio * box.h < minWidth) continue;
      picked.push({ index, ratio });
      ratioSum += ratio;
      const natural = ratioSum * box.h;
      const gaps = gap * (picked.length - 1);
      const score = Math.abs(Math.log((box.w - gaps) / Math.max(1, natural)));
      if (score < bestScore) {
        bestScore = score;
        bestCount = picked.length;
      }
      if (natural + gaps >= box.w) break;
    }
    if (bestCount > 0 && bestCount < picked.length) {
      for (const drop of picked.slice(bestCount)) ratioSum -= drop.ratio;
      picked.length = bestCount;
    }
    // 末位换牌：前缀只能“截断”，最后一张比例不合时整行仍会被拉变形
    // （两张 1.78+0.71 拉成 1.68 倍横框）。在扫描窗口里给末位找一张
    // 让整行 scale 更接近 1 的替换，竖图不再被横条裁切。
    if (picked.length > 0) {
      const baseSum = ratioSum - picked[picked.length - 1].ratio;
      const gaps = gap * (picked.length - 1);
      let bestScale = (box.w - gaps) / Math.max(1, ratioSum * box.h);
      let bestSwap: { index: number; ratio: number } | null = null;
      for (let step = 0; step < total && step < 64; step += 1) {
        const index = (start + step) % total;
        if (used.has(index)) continue;
        if (picked.some((entry) => entry.index === index)) continue;
        const entry = items[index];
        const ratio = entry.w > 0 && entry.h > 0 ? entry.w / entry.h : 1;
        if (picked.length === 1 && enforceWidth && ratio * box.h < minWidth) continue;
        const scale = (box.w - gaps) / Math.max(1, (baseSum + ratio) * box.h);
        if (Math.abs(Math.log(scale)) < Math.abs(Math.log(bestScale))) {
          bestScale = scale;
          bestSwap = { index, ratio };
        }
      }
      if (bestSwap) {
        ratioSum = baseSum + bestSwap.ratio;
        picked[picked.length - 1] = bestSwap;
      }
    }
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
  // 按格位比例选图：每格 slotRatio，挑比例最接近的 wanted 张，
  // 填满后 scale 自然接近 1——旧版“顺次取前 N 张”会把竖图塞进宽扁列
  // （947×496 的列装 0.75 竖图，cover 只剩 39%）。
  const usable = box.h - gap * (wanted - 1);
  const slotRatio = (box.w * wanted) / Math.max(1, usable);
  const candidates: { index: number; ratio: number; delta: number }[] = [];
  for (let step = 0; step < total && step < 64; step += 1) {
    const index = (start + step) % total;
    if (used.has(index)) continue;
    const entry = items[index];
    const ratio = entry.w > 0 && entry.h > 0 ? entry.w / entry.h : 1;
    candidates.push({ index, ratio, delta: Math.abs(Math.log(ratio / slotRatio)) });
  }
  if (candidates.length === 0) return;
  candidates.sort((left, right) => left.delta - right.delta);
  const picked = candidates.slice(0, wanted);
  const natural = picked.reduce((sum, entry) => sum + box.w / entry.ratio, 0);
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
    for (let probe = 0; probe < 12; probe += 1) {
      const index = Math.floor(rng() * total);
      const entry = spec.items[index];
      const ratio = entry.w > 0 && entry.h > 0 ? entry.w / entry.h : 1;
      const delta = Math.abs(Math.log(ratio / target));
      if (delta < bestDelta) {
        bestDelta = delta;
        heroIndex = index;
      }
    }
    // 比例差超过 30% 就不做主图：竖图装进横框会被 cover 裁掉头脚
    // （用户反馈“一张竖图你给我按横图截”），宁可退回常规横条。
    if (heroIndex >= 0 && bestDelta <= Math.log(1.3)) {
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

/** 视野 + 缓冲里所有格子的卡（按世界坐标过滤），格子本身在 DOM 里不落地。
 *  传入 cache（同一 spec 生命周期内复用）可让平移时卡片对象保持引用稳定，
 *  只重算当前视野覆盖的格子，避免每帧重新随机排布。 */
export function cellTilesInView(
  spec: CellSpec,
  view: { x: number; y: number; w: number; h: number },
  padding = 420,
  cache?: Map<string, PlacedTile[]>,
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
  if (cache) {
    for (const key of cache.keys()) {
      const [keyB, keyC] = key.split(',').map(Number);
      if (keyB < b0 || keyB > b1 || keyC < c0 || keyC > c1) cache.delete(key);
    }
  }
  for (let b = b0; b <= b1; b += 1) {
    for (let c = c0; c <= c1; c += 1) {
      let cell = cache?.get(b + ',' + c);
      if (!cell) {
        cell = layoutCell(spec, b, c);
        cache?.set(b + ',' + c, cell);
      }
      for (const tile of cell) {
        if (tile.rect.x + tile.rect.w < left || tile.rect.x > right) continue;
        if (tile.rect.y + tile.rect.h < top || tile.rect.y > bottom) continue;
        tiles.push(tile);
      }
    }
  }
  return tiles;
}

/* ===== v6：聚焦挤压（保比例 + 最小短边） =====
 *
 * 英雄卡放大后，同格其余图装进英雄四周的区域。旧版为了让区域“填满”，
 * 会把剩余图片等分进格子，宽而矮的区域就会产出 200×70 的横条、窄而高的
 * 区域会产出 47×130 的竖条。v6 反过来：先保证每张邻图短边不低于阈值，
 * 再按原图比例排；放不下就减少邻图数量（优先保留离英雄最近的），
 * 而不是把图片压成不可读的条。
 *
 * 排法：区域内部按图片比例排序，横图一行、竖图一行；每行独立选高度，
 * 行内按宽度居中。这样横图和竖图不会互相拖累，也不会为了填满而拉宽。
 */

interface GridEntry {
  item: WallItem;
  index: number;
}

function entryRatio(entry: GridEntry): number {
  return entry.item.w > 0 && entry.item.h > 0 ? entry.item.w / entry.item.h : 1;
}

/** 邻图短边下限：随格子尺寸缩放，桌面约 104px，移动端约 80px。 */
function spotlightMinShort(innerW: number, innerH: number): number {
  return Math.round(Math.max(80, Math.min(104, Math.min(innerW, innerH) * 0.17)));
}

interface RowGroup {
  entries: readonly GridEntry[];
  sumRatio: number;
  minHeight: number;
  widthHeight: number;
}

interface RowEvaluation {
  groups: RowGroup[];
  heights: number[];
  score: number;
}

interface SpotlightRegion {
  kind: 'top' | 'bottom' | 'left' | 'right';
  x: number;
  y: number;
  w: number;
  h: number;
  wide: boolean;
  assigned: GridEntry[];
  packed: ReturnType<typeof packSpotlightRows> | null;
}

/**
 * 给一组行计算各自高度：每行按宽度填满得到一个“宽度高度”，
 * 再在总高度不够时按各行可压缩空间等比收缩，但绝不低于短边下限。
 */
function evaluateRowGroups(
  groups: GridEntry[][],
  box: { w: number; h: number },
  gap: number,
  minShort: number,
): RowEvaluation | null {
  const rowCount = groups.length;
  let sumMin = gap * (rowCount - 1);
  let sumWant = gap * (rowCount - 1);
  const meta: RowGroup[] = [];
  for (const entries of groups) {
    let sumRatio = 0;
    let minHeight = 0;
    for (const entry of entries) {
      const ratio = entryRatio(entry);
      sumRatio += ratio;
      minHeight = Math.max(minHeight, minShort / Math.min(1, ratio));
    }
    if (sumRatio <= 0) return null;
    const widthHeight = (box.w - gap * (entries.length - 1)) / sumRatio;
    meta.push({ entries, sumRatio, minHeight, widthHeight });
    sumMin += minHeight;
    sumWant += widthHeight;
  }
  if (sumMin > box.h + 1e-6) return null;
  let heights: number[];
  if (sumWant <= box.h + 1e-6) {
    heights = meta.map((row) => row.widthHeight);
  } else {
    const target = box.h - gap * (rowCount - 1);
    const excess = sumWant - target;
    const slack = meta.reduce((sum, row) => sum + (row.widthHeight - row.minHeight), 0);
    if (slack < excess - 1e-6) return null;
    const scale = excess / slack;
    heights = meta.map((row) => row.widthHeight - scale * (row.widthHeight - row.minHeight));
  }
  for (let index = 0; index < meta.length; index += 1) {
    if (heights[index] + 1e-6 < meta[index].minHeight) return null;
  }
  return { groups: meta, heights, score: Math.min(...heights) };
}

/** 固定条目集合的行排布；行数从少到多试，第一次可行即返回。 */
function packRowsExact(
  entries: readonly GridEntry[],
  box: { x: number; y: number; w: number; h: number },
  gap: number,
  minShort: number,
  align: 'center' | 'start',
): { rects: { item: WallItem; index: number; rect: TileRect }[]; rows: number } | null {
  const total = entries.length;
  if (total === 0 || box.w < minShort || box.h < minShort) return null;
  const sorted = [...entries].sort((left, right) => entryRatio(right) - entryRatio(left));
  const maxRows = Math.min(
    total,
    Math.max(1, Math.floor((box.h + gap) / (minShort + gap)) + 1),
  );
  const state: { best: RowEvaluation | null } = { best: null };
  for (let rows = 1; rows <= maxRows; rows += 1) {
    const consider = (groups: GridEntry[][]) => {
      const evaluated = evaluateRowGroups(groups, box, gap, minShort);
      if (!evaluated) return;
      const best = state.best;
      if (
        !best ||
        evaluated.score > best.score + 0.5 ||
        (Math.abs(evaluated.score - best.score) <= 0.5 && evaluated.groups.length < best.groups.length)
      ) {
        state.best = evaluated;
      }
    };
    const walk = (start: number, row: number, acc: GridEntry[][]) => {
      if (row === rows - 1) {
        const tail = sorted.slice(start);
        if (tail.length > 0) consider([...acc, tail]);
        return;
      }
      const maxEnd = total - (rows - row - 1);
      for (let end = start + 1; end <= maxEnd; end += 1) {
        walk(end, row + 1, [...acc, sorted.slice(start, end)]);
      }
    };
    walk(0, 0, []);
    if (state.best) break;
  }
  const best = state.best;
  if (!best) return null;
  const rects: { item: WallItem; index: number; rect: TileRect }[] = [];
  const totalHeight = best.heights.reduce((sum, height) => sum + height, 0) + gap * (best.heights.length - 1);
  let y = align === 'start' ? box.y : box.y + (box.h - totalHeight) / 2;
  for (let index = 0; index < best.groups.length; index += 1) {
    const row = best.groups[index];
    const height = best.heights[index];
    const widths = row.entries.map((entry) => height * entryRatio(entry));
    const rowWidth = widths.reduce((sum, width) => sum + width, 0) + gap * (row.entries.length - 1);
    let x = box.x + (box.w - rowWidth) / 2;
    for (let position = 0; position < row.entries.length; position += 1) {
      rects.push({
        item: row.entries[position].item,
        index: row.entries[position].index,
        rect: { x, y, w: widths[position], h: height },
      });
      x += widths[position] + gap;
    }
    y += height + gap;
  }
  return { rects, rows: best.groups.length };
}

/**
 * 在区域内尽量多放：二分找最大可保留数量（数量越少越容易放下），
 * 返回的 entries 是实际放下的前缀数量。
 */
function packSpotlightRows(
  entries: readonly GridEntry[],
  box: { x: number; y: number; w: number; h: number },
  gap: number,
  minShort: number,
  align: 'center' | 'start' = 'center',
): { rects: { item: WallItem; index: number; rect: TileRect }[]; rows: number; entries: number } | null {
  let low = 1;
  let high = entries.length;
  let best: { rects: { item: WallItem; index: number; rect: TileRect }[]; rows: number; entries: number } | null = null;
  const cache = new Map<number, ReturnType<typeof packRowsExact>>();
  while (low <= high) {
    const middle = (low + high) >> 1;
    let packed = cache.get(middle);
    if (packed === undefined) {
      packed = packRowsExact(entries.slice(0, middle), box, gap, minShort, align);
      cache.set(middle, packed);
    }
    if (packed) {
      best = { ...packed, entries: middle };
      low = middle + 1;
    } else {
      high = middle - 1;
    }
  }
  return best;
}

/** 重排一个格子：英雄卡在原地等比放大，邻图按比例装进四周剩余区域。 */
export function layoutSpotlightCell(
  spec: CellSpec,
  b: number,
  c: number,
  heroId: string,
  anchorRect?: TileRect,
  heroIndex?: number,
): PlacedTile[] {
  const normal = layoutCell(spec, b, c);
  const hero =
    heroIndex != null
      ? normal.find((tile) => tile.index === heroIndex && tile.item.id === heroId)
      : normal.find((tile) => tile.item.id === heroId);
  if (!hero || normal.length <= 1) return normal;

  const gap = spec.gap;
  const innerW = Math.max(1, spec.cellWidth - gap);
  const innerH = Math.max(1, spec.cellHeight - gap);
  const originX = c * spec.cellWidth;
  const originY = b * spec.cellHeight;
  const cell = { b, c };
  const ratio = entryRatio(hero);
  const narrow = innerW < 620;
  const minShort = spotlightMinShort(innerW, innerH);

  // 密集格适当收小英雄：同格邻图越多，留给邻图的空间越大；面积仍由下面的
  // 1.05 倍保底兜住，不会出现“点了反而更小”。
  const restCount = normal.length - 1;
  const density = Math.max(0, Math.min(1, (restCount - 2) / 14));
  const heroScale = 1 - 0.3 * density;
  const fitHero = (maxW: number, maxH: number) => {
    let w = maxW;
    let h = maxW / ratio;
    if (h > maxH) {
      h = maxH;
      w = maxH * ratio;
    }
    return { w, h };
  };
  let heroW: number;
  let heroH: number;
  if (narrow) {
    // 窄屏留一条侧柱（约 22% 宽）给竖图，避免只剩上下两条横带。
    ({ w: heroW, h: heroH } = fitHero(innerW * 0.78 * heroScale, innerH * 0.62 * heroScale));
  } else {
    ({ w: heroW, h: heroH } = fitHero(innerW * 0.6 * heroScale, innerH * 0.62 * heroScale));
  }

  // 聚焦 = 放大：英雄面积不得小于被点卡片的可见面积。
  const minArea =
    anchorRect && anchorRect.w > 0 && anchorRect.h > 0
      ? anchorRect.w * anchorRect.h * 1.05
      : 0;
  if (minArea > 0) {
    const area = heroW * heroH;
    if (area < minArea) {
      const grow = Math.sqrt(minArea / area);
      heroW *= grow;
      heroH *= grow;
    }
  }
  const maxW = Math.max(1, innerW - minShort - gap);
  const maxH = Math.max(1, innerH - minShort - gap);
  const shrink = Math.min(1, maxW / heroW, maxH / heroH);
  heroW *= shrink;
  heroH *= shrink;
  // 满宽的卡片（移动端常见）没有空间同时留侧柱：先放弃侧柱把宽度放回整格，
  // 面积还不够时只补高度，最多带来 5% 左右的比例偏差——也好过“点了反而变小”。
  if (minArea > 0 && heroW * heroH < minArea - 1e-6) {
    if (heroW < innerW) {
      const grow = Math.min(innerW / heroW, Math.sqrt(minArea / (heroW * heroH)));
      heroW *= grow;
      heroH *= grow;
    }
    if (heroW > innerW) {
      heroH *= innerW / heroW;
      heroW = innerW;
    }
    if (heroW * heroH < minArea - 1e-6) {
      heroH = Math.min(maxH, minArea / heroW);
    }
    if (heroH > innerH) {
      heroW *= innerH / heroH;
      heroH = innerH;
    }
  }

  // 锚定被点图片的当前位置：点哪张，哪张就在原地放大；只有离边缘不足
  // 一个短边下限时才贴边，把细缝并给另一侧。
  const anchorX = anchorRect ? anchorRect.x + anchorRect.w / 2 : hero.rect.x + hero.rect.w / 2;
  const anchorY = anchorRect ? anchorRect.y + anchorRect.h / 2 : hero.rect.y + hero.rect.h / 2;
  let heroX = Math.min(
    Math.max(originX, anchorX - heroW / 2),
    originX + innerW - heroW,
  );
  let heroY = Math.min(
    Math.max(originY, anchorY - heroH / 2),
    originY + innerH - heroH,
  );
  const SNAP = minShort;
  if (heroX - originX - gap < SNAP) heroX = originX;
  else if (originX + innerW - heroX - heroW - gap < SNAP) heroX = originX + innerW - heroW;
  if (heroY - originY - gap < SNAP) heroY = originY;
  else if (originY + innerH - heroY - heroH - gap < SNAP) heroY = originY + innerH - heroH;

  const heroRect = { x: Math.round(heroX), y: Math.round(heroY), w: heroW, h: heroH };
  const rest = normal
    .filter((tile) => tile.index !== hero.index)
    .map((tile) => ({ item: tile.item, index: tile.index, rect: tile.rect }));
  // 优先保留离被点图片最近的邻图；容量不足时丢最远的，而不是随机丢。
  rest.sort((left, right) => {
    const leftDistance =
      (left.rect.x + left.rect.w / 2 - anchorX) ** 2 +
      (left.rect.y + left.rect.h / 2 - anchorY) ** 2;
    const rightDistance =
      (right.rect.x + right.rect.w / 2 - anchorX) ** 2 +
      (right.rect.y + right.rect.h / 2 - anchorY) ** 2;
    return leftDistance - rightDistance || left.index - right.index;
  });

  const leftW = Math.max(0, heroRect.x - originX - gap);
  const rightW = Math.max(0, originX + innerW - heroRect.x - heroRect.w - gap);
  const topH = Math.max(0, heroRect.y - originY - gap);
  const bottomH = Math.max(0, originY + innerH - heroRect.y - heroRect.h - gap);
  const regions: SpotlightRegion[] = [];
  const addRegion = (kind: SpotlightRegion['kind'], x: number, y: number, w: number, h: number) => {
    if (Math.min(w, h) < minShort) return;
    regions.push({ kind, x, y, w, h, wide: w / Math.max(1, h) >= 1.4, assigned: [], packed: null });
  };
  addRegion('top', originX, originY, innerW, topH);
  addRegion('bottom', originX, heroRect.y + heroRect.h + gap, innerW, bottomH);
  addRegion('left', originX, heroRect.y, leftW, heroRect.h);
  addRegion('right', heroRect.x + heroRect.w + gap, heroRect.y, rightW, heroRect.h);

  const out: PlacedTile[] = [
    { item: hero.item, index: hero.index, rect: heroRect, cell },
  ];
  if (regions.length === 0) return out;

  // 逐张试放：按“最适合的区域”排序，放得下才保留；所有区域都放不下
  // 就丢掉这张（最远的先丢），保证留下的每一张都达到短边下限。
  for (const entry of rest) {
    const entryAspect = entryRatio(entry);
    const candidates = regions
      .map((region) => {
        const fitHeight = Math.min(region.h, region.w / entryAspect);
        const fitWidth = fitHeight * entryAspect;
        if (Math.min(fitHeight, fitWidth) + 1e-6 < minShort) return null;
        const categoryMatch = entryAspect >= 1 ? region.wide : !region.wide;
        return {
          region,
          score: fitHeight + (categoryMatch ? 24 : 0) + (region.w * region.h) / 1e6,
        };
      })
      .filter((candidate): candidate is { region: SpotlightRegion; score: number } => candidate != null)
      .sort((left, right) => right.score - left.score);
    for (const { region } of candidates) {
      region.assigned.push(entry);
      const packed = packSpotlightRows(region.assigned, region, gap, minShort);
      if (packed && packed.entries === region.assigned.length) {
        region.packed = packed;
        break;
      }
      region.assigned.pop();
    }
  }

  for (const region of regions) {
    if (!region.packed) continue;
    for (const entry of region.packed.rects) out.push({ ...entry, cell });
  }
  return out;
}

/** 有限墙的聚焦重排：英雄卡居中大块，其余图右侧列 + 下方正常行，全部顺延。 */
export function layoutFeaturedWorld(
  items: readonly WallItem[],
  options: WorldOptions,
  heroId: string,
  focus: { centerX: number; viewHeight: number; bottomInset: number },
  anchorRect?: TileRect,
  heroIndex?: number,
): { tiles: LaidTile[]; height: number } {
  const resolvedHeroIndex =
    heroIndex != null && items[heroIndex]?.id === heroId
      ? heroIndex
      : items.findIndex((item) => item.id === heroId);
  if (resolvedHeroIndex < 0) return layoutWorld(items, options);
  const hero = items[resolvedHeroIndex];
  const rest: GridEntry[] = items
    .map((item, index) => ({ item, index }))
    .filter((entry) => entry.index !== resolvedHeroIndex);
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
  // 与无限墙同一保底：聚焦后的面积不小于被点卡片（有限墙可以纵向滚动，
  // 高度不设上限，只夹世界宽）。
  if (anchorRect && anchorRect.w > 0 && anchorRect.h > 0) {
    const minArea = anchorRect.w * anchorRect.h * 1.05;
    const area = heroW * heroH;
    if (area < minArea) {
      const grow = Math.min(Math.sqrt(minArea / area), width / heroW);
      heroW *= grow;
      heroH *= grow;
    }
  }
  // 有锚点时英雄留在被点图片的位置；没有锚点（兼容旧调用）才回到内容区中心。
  let heroX: number;
  let heroY: number;
  if (anchorRect) {
    const centerX = anchorRect.x + anchorRect.w / 2;
    const centerY = anchorRect.y + anchorRect.h / 2;
    heroX = Math.max(0, Math.min(width - heroW, Math.round(centerX - heroW / 2)));
    heroY = Math.max(0, Math.round(centerY - heroH / 2));
  } else {
    heroX = Math.max(0, Math.round(focus.centerX - heroW / 2));
    const visibleHeight = Math.max(200, focus.viewHeight - focus.bottomInset);
    heroY = Math.max(0, Math.round(visibleHeight / 2 - heroH / 2));
  }
  heroW = Math.min(heroW, width - heroX);
  const rightW = width - heroX - heroW - gap;
  const minShort = spotlightMinShort(width, targetHeight * 2.3);

  const tiles: LaidTile[] = [
    {
      item: hero,
      index: resolvedHeroIndex,
      rect: { x: heroX, y: heroY, w: heroW, h: heroH },
    },
  ];

  // 右列同样按“短边下限 + 原图比例”装：装得下几张放几张，其余顺延到下方。
  const rightPlaced = new Set<number>();
  if (rightW >= minShort) {
    const rightBox = { x: heroX + heroW + gap, y: heroY, w: rightW, h: heroH };
    // 极端竖长图单张就超过区域高度，先把这类剔掉，避免一张不可放的图
    // 把后面所有可放的邻图一起挡在右列之外。
    const candidates = rest.slice(0, Math.min(rest.length, 8)).filter((entry) => {
      const entryAspect = entryRatio(entry);
      return minShort / Math.min(1, entryAspect) <= heroH + 1e-6;
    });
    const packed = packSpotlightRows(
      candidates,
      rightBox,
      gap,
      minShort,
      'start',
    );
    if (packed) {
      for (const entry of packed.rects) {
        rightPlaced.add(entry.index);
        tiles.push({ ...entry });
      }
    }
  }
  const belowItems = rest.filter((entry) => !rightPlaced.has(entry.index));

  const belowOffset = heroY + heroH + gap;
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
  return { tiles, height: heroY + heroH };
}
