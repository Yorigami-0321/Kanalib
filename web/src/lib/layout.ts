/** 画布/侧栏的几何常量：JS 与 CSS 必须保持一致（CSS 见 styles.css 的 --seam-w / --inspector-w）。 */
export const SEAM_WIDE = 120;
export const SEAM_NARROW = 64;
export const INSPECTOR_WIDTH = 300;
/** 与 styles.css 的 900px 断点一致：≤900 时缝收窄，检查器改底部 sheet。 */
export const NARROW_BREAKPOINT = 900;

export interface LayoutMetrics {
  /** 缝宽（桌面 120 / 窄屏 64）。 */
  seam: number;
  /** 检查器占用的水平宽度（桌面 300；窄屏是底部 sheet，不占水平空间）。 */
  inspector: number;
  /** 是否桌面宽屏（检查器为左侧栏而不是底部 sheet）。 */
  wide: boolean;
}

export function readLayoutMetrics(): LayoutMetrics {
  const width = typeof window === 'undefined' ? 1440 : window.innerWidth;
  const wide = width > NARROW_BREAKPOINT;
  return {
    seam: wide ? SEAM_WIDE : SEAM_NARROW,
    inspector: wide ? INSPECTOR_WIDTH : 0,
    wide,
  };
}
