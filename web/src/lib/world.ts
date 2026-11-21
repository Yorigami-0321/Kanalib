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

/* ===== v5.2：聚焦挤压（保留 v4 的重排语义，修正细条） =====
 *
 * 英雄卡放大后，同格其余图被装进英雄四周的剩余区域（上下通宽横带 + 左右中柱）；
 * 列数不写死 1，按“区域宽高比 × 条目平均图片比例”选列，让每张卡接近原图比例，
 * 不再出现 300×90 那种横向细条；末行不满时 justified 拉平，区域内不留空槽。
 */

interface GridEntry {
  item: WallItem;
  index: number;
}

function entryRatio(entry: GridEntry): number {
  return entry.item.w > 0 && entry.item.h > 0 ? entry.item.w / entry.item.h : 1;
}

/**
 * 在 box 里为 entries 选列数：让单元格宽高比接近平均图片比例，并惩罚过小的短边。
 * 这是“挤压但不切条”的关键——单纯一行/一列都会在某个数量下退化为细条。
 */
function gridColumns(
  entries: readonly GridEntry[],
  box: { w: number; h: number },
  gap: number,
): number {
  const count = entries.length;
  if (count <= 1 || box.w <= 0 || box.h <= 0) return 1;
  const average = entries.reduce((sum, entry) => sum + entryRatio(entry), 0) / count;
  // 行高/列宽硬下限 70：矮横带里图多时旧评分会选两行，产出 <70px 的
  // 细条卡（回归闸门抓到的 flake）。先算行数上限再反推列数下限。
  const maxRows = Math.max(1, Math.floor((box.h + gap) / (70 + gap)));
  const minCols = Math.ceil(count / maxRows);
  const maxCols = Math.max(minCols, Math.min(count, Math.floor((box.w + gap) / (70 + gap))));
  let best = minCols;
  let bestScore = Infinity;
  for (let cols = minCols; cols <= maxCols; cols += 1) {
    const rows = Math.ceil(count / cols);
    const cellW = (box.w - gap * (cols - 1)) / cols;
    const cellH = (box.h - gap * (rows - 1)) / rows;
    if (cellW <= 0 || cellH <= 0) continue;
    const aspectDelta = Math.abs(Math.log(cellW / cellH / Math.max(0.2, average)));
    const shortSide = Math.min(cellW, cellH);
    const score = aspectDelta + (shortSide < 88 ? 1.6 : 0);
    if (score < bestScore) {
      bestScore = score;
      best = cols;
    }
  }
  return best;
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
  const cellH = Math.max(1, (box.h - gap * (rows - 1)) / rows);
  return entries.map((entry, position) => {
    const row = Math.floor(position / cols);
    const rowStart = row * cols;
    // 末行不满时按实际张数均分整行宽（justified）：box 内不留空槽，
    // 旧版会把第 4 格留空，聚焦时看起来就是英雄旁边的“空缺”。
    const inRow = row === rows - 1 ? count - rowStart : cols;
    const cellW = Math.max(1, (box.w - gap * (inRow - 1)) / inRow);
    const col = position - rowStart;
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

/** 重排一个格子：英雄卡在原地等比放大，邻图装进四周剩余区域（上下横带 + 左右中柱）。 */
export function layoutSpotlightCell(
  spec: CellSpec,
  b: number,
  c: number,
  heroId: string,
  anchorRect?: TileRect,
): PlacedTile[] {
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

  // 英雄卡只做等比缩放：把原图比例装进包围盒（宽支路 60% 宽×62% 高，
  // 窄支路全宽×62% 高），碰到哪条边就以哪条边为准。不做任何单边的
  // 最小/最大钳制——旧版 46% 宽度下限会把竖图硬拉成横框，object-fit
  // 裁切掉头脚、挤压出周围空缺；等比后竖图就是竖框、横图就是横框。
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
    ({ w: heroW, h: heroH } = fitHero(innerW, innerH * 0.62));
  } else {
    ({ w: heroW, h: heroH } = fitHero(innerW * 0.6, innerH * 0.62));
  }

  // 聚焦 = 放大：英雄面积不得小于被点卡片的可见面积（用户反馈“点击后
  // 图片还没原来的大”——主图大卡被 60%×62% 包围盒收小了）。基础包围盒
  // 不够就等比放大到 1.05 倍原面积；上限留出邻图区域：每轴至少 88px，
  // 否则上下横带/左右中柱会被压成 <70px 的细条（v5.2 用户投诉过）。
  // 极端比例碰顶时面积保底让位于几何必然。
  if (anchorRect && anchorRect.w > 0 && anchorRect.h > 0) {
    const minArea = anchorRect.w * anchorRect.h * 1.05;
    const area = heroW * heroH;
    if (area < minArea) {
      const grow = Math.sqrt(minArea / area);
      heroW *= grow;
      heroH *= grow;
    }
  }
  const maxW = narrow ? innerW : innerW - 88;
  const maxH = innerH - 88;
  const shrink = Math.min(1, maxW / heroW, maxH / heroH);
  heroW *= shrink;
  heroH *= shrink;

  // 锚定被点图片的当前位置：点哪张，哪张就在原地放大；邻图围到它周围，
  // 不再把新英雄搬到格子左上角、旧英雄挪到新位置（那会像“交换图片”）。
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

  // 贴边收缝：某一侧剩余空间不足 96px（放不下一条 usable 的邻图带/列）时
  // 向近侧贴齐，把细缝并给另一侧。旧版 48px 只收细缝；面积保底把英雄放大后
  // 48–96px 的剩余会排成 <70px 的细条卡，所以阈值提到 96。
  const SNAP = 96;
  if (heroX - originX - gap < SNAP) heroX = originX;
  else if (originX + innerW - heroX - heroW - gap < SNAP) heroX = originX + innerW - heroW;
  if (heroY - originY - gap < SNAP) heroY = originY;
  else if (originY + innerH - heroY - heroH - gap < SNAP) heroY = originY + innerH - heroH;

  const out: PlacedTile[] = [
    { item: hero.item, index: hero.index, rect: { x: heroX, y: heroY, w: heroW, h: heroH }, cell },
  ];
  const rest = normal
    .filter((tile) => tile.item.id !== heroId)
    .map((tile) => ({ item: tile.item, index: tile.index }));

  // 英雄四周全部利用（旧版只装“一侧整列 + 一带”，锚在格子中部时会留下
  // L 形空白）：顶部/底部两条通宽横带 + 英雄左右两段中柱。四块互不重叠，
  // 拼起来正好补满格子剩余空间；配合 gridRects 的末行 justified，不再有洞。
  const MIN_REGION = 48;
  const leftW = Math.max(0, heroX - originX - gap);
  const rightW = Math.max(0, originX + innerW - heroX - heroW - gap);
  const topH = Math.max(0, heroY - originY - gap);
  const bottomH = Math.max(0, originY + innerH - heroY - heroH - gap);
  const regions: { x: number; y: number; w: number; h: number }[] = [];
  if (topH >= MIN_REGION) regions.push({ x: originX, y: originY, w: innerW, h: topH });
  if (bottomH >= MIN_REGION) {
    regions.push({ x: originX, y: heroY + heroH + gap, w: innerW, h: bottomH });
  }
  if (leftW >= MIN_REGION) regions.push({ x: originX, y: heroY, w: leftW, h: heroH });
  if (rightW >= MIN_REGION) {
    regions.push({ x: heroX + heroW + gap, y: heroY, w: rightW, h: heroH });
  }

  if (rest.length > 0) {
    if (regions.length === 0) {
      // 防御兜底（理论上到不了：英雄高度有 62% 上限，上下带必有一条 ≥ 48px）：
      // 退回英雄下方整宽重排，至少邻图不丢。
      const box = {
        x: originX,
        y: heroY + heroH + gap,
        w: innerW,
        h: Math.max(2, originY + innerH - heroY - heroH - gap),
      };
      const cols = Math.max(1, Math.min(rest.length, gridColumns(rest, box, gap)));
      for (const entry of gridRects(rest, box, cols, gap)) out.push({ ...entry, cell });
    } else {
      // 图少于区域数时丢最小的区域（只在低密度格子出现，损失一条窄带好过留洞）。
      const kept = [...regions]
        .sort((a, b) => b.w * b.h - a.w * a.h)
        .slice(0, Math.min(regions.length, rest.length));
      // 水位分配：每块至少 1 张，剩余名额反复给“单张摊到面积最大”的块，
      // 让各区域的图密度大致均匀。
      const areas = kept.map((region) => region.w * region.h);
      const counts = kept.map(() => 1);
      for (let extra = rest.length - kept.length; extra > 0; extra -= 1) {
        let neediest = 0;
        for (let i = 1; i < kept.length; i += 1) {
          if (areas[i] / counts[i] > areas[neediest] / counts[neediest]) neediest = i;
        }
        counts[neediest] += 1;
      }
      let cursor = 0;
      for (let i = 0; i < kept.length; i += 1) {
        const entries = rest.slice(cursor, cursor + counts[i]);
        cursor += counts[i];
        if (entries.length === 0) continue;
        const box = kept[i];
        const cols = Math.max(1, Math.min(entries.length, gridColumns(entries, box, gap)));
        for (const entry of gridRects(entries, box, cols, gap)) {
          out.push({ ...entry, cell });
        }
      }
    }
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
  const useRightColumn = rightW >= 180;

  const rightItems = useRightColumn ? rest.slice(0, Math.min(rest.length, 4)) : [];
  const belowItems = rest.slice(rightItems.length);

  const tiles: LaidTile[] = [
    {
      item: hero,
      index: heroIndex,
      rect: { x: heroX, y: heroY, w: heroW, h: heroH },
    },
  ];
  if (useRightColumn) {
    const rightBox = { x: heroX + heroW + gap, y: heroY, w: Math.max(1, rightW), h: heroH };
    const rightCols = Math.max(1, Math.min(rightItems.length, gridColumns(rightItems, rightBox, gap)));
    for (const entry of gridRects(rightItems, rightBox, rightCols, gap)) {
      tiles.push({ ...entry });
    }
  }

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
