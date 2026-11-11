/**
 * 双击展开的共享元素动画（图片从卡片位置"放大"到全屏）。
 *
 * 不走 React 状态：卡片在墙上、详情页是另一棵子树，跨路由传 rect 只能靠模块级
 * 变量 + 时间窗。`beginExpand` 打开一个 ~500ms 的窗口，详情页首帧用 `expanding()`
 * 判断要不要把自己的内容先蒙住（StrictMode 双挂载下两次读到的都是同一个窗口，
 * 不会像"读后即焚"那样第二次拿不到）。
 */

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

const SLIDE_MS = 420;
let windowUntil = 0;

export function beginExpand(): void {
  windowUntil = performance.now() + SLIDE_MS + 80;
}

/** 详情页首帧是否处于展开窗口内（ true 时先蒙住真实内容，等飞行的克隆图落位）。 */
export function expanding(): boolean {
  return performance.now() < windowUntil;
}

/**
 * 真实内容揭示的延迟：对齐飞行时钟的 380ms（克隆 420ms 落位）。
 * 用挂载时刻反推延迟，挂载抖动不会把揭示推到晚于克隆落位（空窗来源）。
 */
export function expandRevealDelay(): number {
  return Math.max(0, windowUntil - 120 - performance.now());
}

/** contain 模式下图片实际占用的矩形；(offsetX, offsetY) 用于详情让开侧栏检查器后的内容区。 */
export function containRect(
  nw: number,
  nh: number,
  vw: number,
  vh: number,
  offsetX = 0,
  offsetY = 0,
): Rect {
  if (!nw || !nh || !vw || !vh) return { x: offsetX, y: offsetY, w: vw, h: vh };
  const scale = Math.min(vw / nw, vh / nh);
  const w = nw * scale;
  const h = nh * scale;
  return { x: offsetX + (vw - w) / 2, y: offsetY + (vh - h) / 2, w, h };
}

/**
 * 一张克隆图从卡片矩形飞到全屏 contain 矩形。
 * 用 object-fit:cover：起点和卡片完全一致（卡片也是 cover 裁切），
 * 终点矩形的宽高比等于原图，cover 退化成 contain，落位与详情页严丝合缝。
 */
export function flyImage(src: string, from: Rect, to: Rect): void {
  if (typeof document === 'undefined') return;
  const clone = document.createElement('img');
  clone.src = src;
  clone.alt = '';
  clone.setAttribute('aria-hidden', 'true');
  clone.style.cssText =
    'position:fixed;margin:0;z-index:200;object-fit:cover;pointer-events:none;' +
    `left:${from.x}px;top:${from.y}px;width:${from.w}px;height:${from.h}px;border-radius:12px;`;
  document.body.appendChild(clone);
  const animation = clone.animate(
    [
      {
        left: `${from.x}px`,
        top: `${from.y}px`,
        width: `${from.w}px`,
        height: `${from.h}px`,
        borderRadius: '12px',
      },
      {
        left: `${to.x}px`,
        top: `${to.y}px`,
        width: `${to.w}px`,
        height: `${to.h}px`,
        borderRadius: '0px',
      },
    ],
    { duration: SLIDE_MS, easing: 'cubic-bezier(0.4, 0, 0.2, 1)', fill: 'forwards' },
  );
  let done = false;
  const finish = () => {
    if (done) return;
    done = true;
    clone.remove();
  };
  animation.onfinish = finish;
  animation.oncancel = finish;
  // 兜底：headless 下 rAF 被节流时 onfinish 可能迟迟不来。
  window.setTimeout(finish, SLIDE_MS + 300);
}
