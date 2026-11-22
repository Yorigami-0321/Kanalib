/** 画布/导航栏的几何常量：JS 与 CSS 必须保持一致（CSS 见 styles.css 的 --rail-w / --inspector-w）。 */
export const RAIL_WIDE = 208;
export const RAIL_COLLAPSED = 60;
export const INSPECTOR_WIDTH = 300;
/** 与 styles.css 的 900px 断点一致：≤900 时导航收成图标，检查器改底部 sheet。 */
export const NARROW_BREAKPOINT = 900;

export interface LayoutMetrics {
  /** 导航栏水平占位（桌面 208 / 折叠 60），兼容旧调用方字段名。 */
  seam: number;
  /** 检查器占用的水平宽度（桌面 300；窄屏是底部 sheet，不占水平空间）。 */
  inspector: number;
  /** 是否桌面宽屏（检查器为左侧栏而不是底部 sheet）。 */
  wide: boolean;
}

export function readLayoutMetrics(): LayoutMetrics {
  const width = typeof window === 'undefined' ? 1440 : window.innerWidth;
  const wide = width > NARROW_BREAKPOINT;
  const cssRailWidth =
    typeof window === 'undefined'
      ? Number.NaN
      : Number.parseFloat(
          window.getComputedStyle(document.documentElement).getPropertyValue('--rail-w'),
        );
  const railWidth = Number.isFinite(cssRailWidth)
    ? cssRailWidth
    : wide
      ? RAIL_WIDE
      : RAIL_COLLAPSED;
  return {
    seam: railWidth,
    inspector: wide ? INSPECTOR_WIDTH : 0,
    wide,
  };
}
