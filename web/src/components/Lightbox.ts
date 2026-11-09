import PhotoSwipeLightbox from 'photoswipe/lightbox';
import type PhotoSwipe from 'photoswipe';
import type { SlideData } from 'photoswipe';
import 'photoswipe/style.css';
import './lightbox.css';
import { fitDims, tierUrl } from '../lib/assetUrl';
import { probeImage } from '../lib/probe';
import type { TierPaths, WallItem } from '../source/types';

type TierName = 't256' | 't512' | 't1024';

const TIER_LONG_EDGE: Record<TierName, number> = { t256: 256, t512: 512, t1024: 1024 };

let activePswp: PhotoSwipe | null = null;

/** t1024 是否真的产出过（契约 §2：paths 里总有该键，但文件可能不存在）。 */
let tierSupport: Promise<boolean> | null = null;
let t1024Available = false;

/**
 * 首次拿到条目后探测一次 t1024 是否存在（整份导出一致，探一张即可）。
 * 存在则 lightbox 优先用它；不存在就老老实实用 t512，避免每张都 404 闪错。
 */
export function primeLightboxTiers(paths: TierPaths, sampleId: string | undefined): void {
  if (!sampleId || tierSupport) return;
  tierSupport = probeImage(tierUrl(paths, 't1024', sampleId)).then((ok) => {
    t1024Available = ok;
    return ok;
  });
}

export function isLightboxOpen(): boolean {
  return activePswp !== null;
}

export function closeLightbox(): void {
  activePswp?.close();
}

export interface OpenLightboxArgs {
  items: WallItem[];
  startIndex: number;
  paths: TierPaths;
  /** 工具栏「详情」按钮 / 双击：路由到 #/d/<id>。 */
  onDetail: (id: string) => void;
  onClose?: () => void;
}

function tierChain(): TierName[] {
  return t1024Available ? ['t1024', 't512', 't256'] : ['t512', 't256'];
}

function slideFor(item: WallItem, paths: TierPaths, tier: TierName): SlideData {
  const dims = fitDims(item.w, item.h, TIER_LONG_EDGE[tier]);
  return {
    src: tierUrl(paths, tier, item.id),
    width: dims.width,
    height: dims.height,
    alt: item.n,
  };
}

/**
 * 打开 lightbox（PhotoSwipe）。图片档位：t1024（若产出）→ t512 → t256，
 * 单张 404 时自动降档重试；工具栏常驻「详情」按钮兜底移动端。
 */
export function openLightbox({
  items,
  startIndex,
  paths,
  onDetail,
  onClose,
}: OpenLightboxArgs): void {
  closeLightbox();
  if (items.length === 0) return;

  const chain = tierChain();
  const slides: SlideData[] = items.map((item) => slideFor(item, paths, chain[0]));
  const tierIndex = new Array<number>(items.length).fill(0);

  const lightbox = new PhotoSwipeLightbox({
    dataSource: slides,
    pswpModule: () => import('photoswipe'),
    bgOpacity: 0.95,
    showHideAnimationType: 'fade',
    padding: { top: 64, bottom: 64, left: 12, right: 12 },
    escKey: true,
    arrowKeys: true,
    trapFocus: true,
    returnFocus: true,
    wheelToZoom: true,
    preload: [1, 2],
    indexIndicatorSep: ' / ',
    closeTitle: '关闭（Esc）',
    zoomTitle: '缩放',
    arrowPrevTitle: '上一张（←）',
    arrowNextTitle: '下一张（→）',
    errorMsg: '这张图加载失败',
  });

  lightbox.on('uiRegister', () => {
    const pswp = lightbox.pswp;
    if (!pswp || !pswp.ui) return;
    // P1-1：实例只能在这里拿到。lightbox.loadAndOpen 内部先 await options.pswpModule()，
    // Promise resolve 之后才 this.pswp = pswp（photoswipe-lightbox.esm.js 的 preload），
    // 所以在 openLightbox 末尾读 lightbox.pswp 永远是 undefined。
    activePswp = pswp;
    pswp.ui.registerElement({
      name: 'detail-button',
      order: 9,
      isButton: true,
      tagName: 'button',
      className: 'pswp-detail-button',
      html: '详情',
      ariaLabel: '查看这一张的详情',
      title: '详情（双击墙上的图也可以）',
      onClick: () => {
        const id = items[pswp.currIndex]?.id;
        pswp.close();
        if (id) onDetail(id);
      },
    });
  });

  // 单张图 404：自动降档（t1024 → t512 → t256）后刷新当前 slide。
  lightbox.on('loadError', (event) => {
    const pswp = lightbox.pswp;
    const index = event?.slide?.index ?? pswp?.currIndex;
    if (pswp === undefined || index === undefined || index === null) return;
    const next = (tierIndex[index] ?? 0) + 1;
    if (next >= chain.length) return;
    tierIndex[index] = next;
    slides[index] = slideFor(items[index], paths, chain[next]);
    pswp.refreshSlideContent(index);
  });

  lightbox.on('close', () => {
    activePswp = null;
    // destroy 幂等：已经销毁过就不再调一次（close 可能被 closeLightbox 重复触发）
    if (lightbox.pswp) lightbox.destroy();
    onClose?.();
  });

  lightbox.init();
  lightbox.loadAndOpen(Math.min(Math.max(startIndex, 0), items.length - 1));
}
