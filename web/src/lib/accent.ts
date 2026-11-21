/**
 * 动态强调色：从当前视野里的图取样，合成一个"能用"的颜色。
 * 只在细线/描边/内发光上用，绝不铺大面积。
 */

const SAMPLE_SIZE = 16;
const cache = new Map<string, string | null>();

interface Oklch {
  l: number;
  c: number;
  h: number;
}

function srgbToLinear(value: number): number {
  const v = value / 255;
  return v <= 0.04045 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4);
}

function linearToSrgb(value: number): number {
  const v = value <= 0.0031308 ? value * 12.92 : 1.055 * Math.pow(value, 1 / 2.4) - 0.055;
  return Math.max(0, Math.min(255, Math.round(v * 255)));
}

function rgbToOklch(r: number, g: number, b: number): Oklch {
  const lr = srgbToLinear(r);
  const lg = srgbToLinear(g);
  const lb = srgbToLinear(b);
  const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb);
  const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb);
  const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb);
  const okL = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
  const okA = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const okB = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  const chroma = Math.sqrt(okA * okA + okB * okB);
  const hue = (Math.atan2(okB, okA) * 180) / Math.PI;
  return { l: okL, c: chroma, h: (hue + 360) % 360 };
}

function oklchToRgb(color: Oklch): [number, number, number] {
  const hue = (color.h * Math.PI) / 180;
  const a = color.c * Math.cos(hue);
  const b = color.c * Math.sin(hue);
  const l = Math.pow(color.l + 0.3963377774 * a + 0.2158037573 * b, 3);
  const m = Math.pow(color.l - 0.1055613458 * a - 0.0638541728 * b, 3);
  const s = Math.pow(color.l - 0.0894841775 * a - 1.291485548 * b, 3);
  const lr = 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
  const lg = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
  const lb = -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s;
  return [linearToSrgb(lr), linearToSrgb(lg), linearToSrgb(lb)];
}

function toHex(rgb: [number, number, number]): string {
  return (
    '#' +
    rgb
      .map((channel) => channel.toString(16).padStart(2, '0'))
      .join('')
  );
}

/** 把任意颜色夹到"能当强调色"的区间：够亮、够彩、但不满。 */
export function clampAccent(color: Oklch): string {
  const odd = ((color.h % 360) + 360) % 360;
  return toHex(
    oklchToRgb({
      l: Math.min(0.72, Math.max(0.62, color.l)),
      c: Math.min(0.16, Math.max(0.1, color.c)),
      h: odd,
    }),
  );
}

/** 一张图里最有代表性的颜色（16×16 取样，丢掉近黑/近白/灰）。 */
export function extractDominantColor(img: HTMLImageElement): Oklch | null {
  const canvas = document.createElement('canvas');
  canvas.width = SAMPLE_SIZE;
  canvas.height = SAMPLE_SIZE;
  const context = canvas.getContext('2d', { willReadFrequently: false });
  if (!context) return null;
  try {
    context.drawImage(img, 0, 0, SAMPLE_SIZE, SAMPLE_SIZE);
  } catch {
    return null;
  }
  let data: Uint8ClampedArray;
  try {
    data = context.getImageData(0, 0, SAMPLE_SIZE, SAMPLE_SIZE).data;
  } catch {
    return null;
  }
  let weight = 0;
  let sumL = 0;
  let sumA = 0;
  let sumB = 0;
  for (let index = 0; index < data.length; index += 4) {
    const r = data[index];
    const g = data[index + 1];
    const b = data[index + 2];
    const alpha = data[index + 3];
    if (alpha < 128) continue;
    const color = rgbToOklch(r, g, b);
    if (color.l < 0.18 || color.l > 0.92 || color.c < 0.04) continue;
    const sample = color.c * 2.4;
    const hue = (color.h * Math.PI) / 180;
    sumL += color.l * sample;
    sumA += Math.cos(hue) * color.c * sample;
    sumB += Math.sin(hue) * color.c * sample;
    weight += sample;
  }
  if (weight <= 0.0001) return null;
  const a = sumA / weight;
  const b = sumB / weight;
  return {
    l: sumL / weight,
    c: Math.sqrt(a * a + b * b),
    h: ((Math.atan2(b, a) * 180) / Math.PI + 360) % 360,
  };
}

/** 取样并缓存；失败返回 null（调用方保留旧值）。 */
export async function accentOf(id: string, url: string): Promise<string | null> {
  const cached = cache.get(id);
  if (cached !== undefined) return cached;
  const color = await new Promise<string | null>((resolve) => {
    const img = new Image();
    img.decoding = 'async';
    img.onload = () => {
      const dominant = extractDominantColor(img);
      resolve(dominant ? clampAccent(dominant) : null);
    };
    img.onerror = () => resolve(null);
    img.src = url;
  });
  cache.set(id, color);
  return color;
}

interface WeightedColor {
  hex: string;
  weight: number;
}

/** 多张图按权重在 OKLab 空间平均（RGB 平均会发脏）。 */
export function blendAccents(colors: WeightedColor[]): string | null {
  const usable = colors.filter((entry) => entry.weight > 0);
  if (usable.length === 0) return null;
  let sumL = 0;
  let sumA = 0;
  let sumB = 0;
  let total = 0;
  for (const entry of usable) {
    const hex = entry.hex.replace('#', '');
    const r = Number.parseInt(hex.slice(0, 2), 16);
    const g = Number.parseInt(hex.slice(2, 4), 16);
    const b = Number.parseInt(hex.slice(4, 6), 16);
    const color = rgbToOklch(r, g, b);
    const hue = (color.h * Math.PI) / 180;
    sumL += color.l * entry.weight;
    sumA += Math.cos(hue) * color.c * entry.weight;
    sumB += Math.sin(hue) * color.c * entry.weight;
    total += entry.weight;
  }
  const a = sumA / total;
  const b = sumB / total;
  return clampAccent({
    l: sumL / total,
    c: Math.sqrt(a * a + b * b),
    h: ((Math.atan2(b, a) * 180) / Math.PI + 360) % 360,
  });
}
