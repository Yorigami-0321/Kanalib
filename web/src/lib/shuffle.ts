import type { WallItem } from '../source/types';

/** FNV-1a 32 位字符串散列：给「随机但固定」的封面选取用。 */
export function hashString(value: string): number {
  let hash = 0x811c9dc5;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193) >>> 0;
  }
  return hash >>> 0;
}

/** 由一个 32 位种子起头的线性同余序列，返回 [0,1) 的稳定伪随机数。 */
export function seededRandom(seed: number): () => number {
  let state = seed >>> 0 || 0x9e3779b9;
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 4294967296;
  };
}

/** Fisher-Yates，返回新数组；同一个种子得到同一个排列。 */
export function seededShuffle<T>(items: readonly T[], seed: number): T[] {
  const random = seededRandom(seed);
  const copy = [...items];
  for (let index = copy.length - 1; index > 0; index -= 1) {
    const swap = Math.floor(random() * (index + 1));
    const value = copy[index];
    copy[index] = copy[swap];
    copy[swap] = value;
  }
  return copy;
}

/**
 * 首页的随机顺序：分片是逐片追加的，只打乱「新到的那一片」并接在已排好的结果后面，
 * 这样滚动中途不会因为重新排序而整墙跳动。
 */
export function appendShuffled(
  previous: readonly WallItem[],
  incoming: readonly WallItem[],
  seed: number,
): WallItem[] {
  if (incoming.length === 0) return previous.length === 0 ? [] : [...previous];
  const tail = seededShuffle(incoming, seed + incoming.length);
  return [...previous, ...tail];
}
