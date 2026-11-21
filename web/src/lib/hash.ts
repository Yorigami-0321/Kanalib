/** 16 位 hex（64 bit）的 Hamming 距离；任一侧不是 16 位 hex 时返回 null。 */
export function hammingHex(a: string | undefined, b: string | undefined): number | null {
  if (!a || !b) return null;
  if (a.length !== 16 || b.length !== 16) return null;
  if (!/^[0-9a-f]{16}$/.test(a) || !/^[0-9a-f]{16}$/.test(b)) return null;
  let distance = 0;
  for (let offset = 0; offset < 16; offset += 8) {
    const left = Number.parseInt(a.slice(offset, offset + 8), 16);
    const right = Number.parseInt(b.slice(offset, offset + 8), 16);
    if (!Number.isFinite(left) || !Number.isFinite(right)) return null;
    distance += popcount32((left ^ right) >>> 0);
  }
  return distance;
}

function popcount32(value: number): number {
  let v = value - ((value >>> 1) & 0x55555555);
  v = (v & 0x33333333) + ((v >>> 2) & 0x33333333);
  v = (v + (v >>> 4)) & 0x0f0f0f0f;
  return (v * 0x01010101) >>> 24;
}
