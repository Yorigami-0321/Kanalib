import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { detailSources, originalUrl, tierUrl } from '../lib/assetUrl';
import { expanding } from '../lib/expand';
import { categoryLabel, formatCount, formatDateTime, formatSize, ratingLabel } from '../lib/format';
import { hammingHex } from '../lib/hash';
import type { TierPaths, WallItem } from '../source/types';
import './detail.css';

const SIMILAR_DISTANCE = 12;
const SIMILAR_LIMIT = 6;

export interface DetailProps {
  item: WallItem;
  items: readonly WallItem[];
  paths: TierPaths;
  onBack: () => void;
  onOpenDetail: (id: string) => void;
  onTagClick: (tag: string) => void;
}

function pad(value: number): string {
  return value < 10 ? '0' + String(value) : String(value);
}

function similarOf(item: WallItem, items: readonly WallItem[]): WallItem[] {
  if (!item.ph) return [];
  const scored: { entry: WallItem; distance: number }[] = [];
  for (const entry of items) {
    if (entry.id === item.id) continue;
    const distance = hammingHex(item.ph, entry.ph);
    if (distance !== null && distance <= SIMILAR_DISTANCE) scored.push({ entry, distance });
  }
  scored.sort((left, right) => left.distance - right.distance);
  return scored.slice(0, SIMILAR_LIMIT).map((entry) => entry.entry);
}

interface PaneProps {
  item: WallItem;
  items: readonly WallItem[];
  paths: TierPaths;
  onOpenDetail: (id: string) => void;
  onTagClick: (tag: string) => void;
}

/**
 * 一屏 = 一张图 + 它自己的信息层。切换时整个 pane 左右平移（niri 那种平滑滑动），
 * 信息跟着图走，而不是只有图换、字硬切。
 */
function Pane({ item, items, paths, onOpenDetail, onTagClick }: PaneProps) {
  const sources = detailSources(paths, item);
  // P2-3：失败档位跟 item.id 绑在一起存，不在 effect 里重置 —— 换 item 的那一帧
  // effect 还没跑，旧 srcIndex 会去索引新的 sources（可能越界成 undefined）。
  const [loadStage, setLoadStage] = useState<{ id: string; index: number }>({ id: item.id, index: 0 });
  const srcIndex = loadStage.id === item.id ? Math.min(loadStage.index, sources.length - 1) : 0;

  const similar = useMemo(() => similarOf(item, items), [items, item.id, item.ph]);
  const original = originalUrl(paths, item);

  return (
    <div className="view__pane">
      <div className="view__media">
        <img
          className="view__img"
          src={sources[srcIndex]}
          alt={item.n}
          decoding="async"
          onError={() =>
            setLoadStage((current) => {
              const base = current.id === item.id ? current.index : 0;
              return { id: item.id, index: Math.min(base + 1, sources.length - 1) };
            })
          }
        />
      </div>
      <div className="view__scroll">
        <div className="view__inner">
          <div className="view__info">
            <div className="view__text">
              <h1 className="view__title">{item.n}</h1>
              <p className="view__meta">
                <span>{formatSize(item.w, item.h)}</span>
                {item.mtime ? <span>{formatDateTime(item.mtime)}</span> : null}
                {item.rt ? <span>{ratingLabel(item.rt)}</span> : null}
                {item.st > 0 ? <span>{'★'.repeat(Math.min(5, Math.round(item.st)))}</span> : null}
              </p>

              <div className="view__tags">
                {item.tags.map(([name, category]) => (
                  <button
                    key={name + '|' + category}
                    type="button"
                    className="chip"
                    title={categoryLabel(category)}
                    onClick={() => onTagClick(name)}
                  >
                    {name}
                  </button>
                ))}
                {original ? (
                  <a className="chip chip--accent" href={original} target="_blank" rel="noreferrer">
                    原图
                  </a>
                ) : null}
              </div>
            </div>

            {similar.length > 0 ? (
              <div className="view__similar">
                <span className="view__similar-label">相似</span>
                <div className="view__similar-strip">
                  {similar.map((entry) => (
                    <button
                      key={entry.id}
                      type="button"
                      className="similar__item"
                      onClick={() => onOpenDetail(entry.id)}
                      title={entry.n}
                      aria-label={entry.n}
                    >
                      <img src={tierUrl(paths, 't256', entry.id)} alt="" loading="lazy" decoding="async" />
                    </button>
                  ))}
                </div>
              </div>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
}

/**
 * 全屏作品页：整页就是那张图（contain、不裁切），信息压在底部一块。
 * 左右切换走水平滑动（niri 式）：当前屏滑出去、下一屏从旁边滑进来。
 * 图层不再依赖 body 滚动，`.view__scroll` 自己滚。
 */
export function Detail({ item, items, paths, onBack, onOpenDetail, onTagClick }: DetailProps) {
  // 屏幕正中正在展示的那张：滑动动画结束后才提交，连按时以上一次的目标为基准。
  const [center, setCenter] = useState<WallItem>(() => items.find((e) => e.id === item.id) ?? item);
  const [slide, setSlide] = useState<{ dir: 1 | -1; from: WallItem; to: WallItem } | null>(null);
  const centerRef = useRef(center);
  centerRef.current = center;
  const slideRef = useRef(slide);
  slideRef.current = slide;
  const trackRef = useRef<HTMLDivElement | null>(null);

  // 路由换了图：与"正中那张"比索引定方向。不相邻（相似条跳转 / 深链）就直接落位。
  useEffect(() => {
    const base = slideRef.current ? slideRef.current.to : centerRef.current;
    if (item.id === base.id) return;
    const fromIndex = items.findIndex((entry) => entry.id === base.id);
    const toIndex = items.findIndex((entry) => entry.id === item.id);
    if (fromIndex < 0 || toIndex < 0 || Math.abs(toIndex - fromIndex) !== 1) {
      setCenter(item);
      setSlide(null);
      return;
    }
    setSlide({ dir: toIndex > fromIndex ? 1 : -1, from: base, to: item });
  }, [item, items]);

  // 滑动动画走完：提交中心、卸下旧屏。target 必须是 track 本身——pane 内部
  // 元素的 animationend 会冒泡上来，不能误当成滑动结束。
  const commitSlide = useCallback((event: React.AnimationEvent<HTMLDivElement>) => {
    if (event.target !== trackRef.current) return;
    const active = slideRef.current;
    if (!active) return;
    setCenter(active.to);
    setSlide(null);
  }, []);

  // ←/→/Esc。滑动进行中不接新指令（连点会跳帧，直接忽略比排队稳）。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target;
      if (target instanceof Element && target.closest('input, textarea, select')) return;
      if (event.key === 'Escape') {
        event.preventDefault();
        onBack();
        return;
      }
      if (slideRef.current) return;
      const index = items.findIndex((entry) => entry.id === centerRef.current.id);
      if (event.key === 'ArrowLeft' && index > 0) {
        event.preventDefault();
        onOpenDetail(items[index - 1].id);
      } else if (event.key === 'ArrowRight' && index + 1 < items.length) {
        event.preventDefault();
        onOpenDetail(items[index + 1].id);
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [items, onBack, onOpenDetail]);

  // 双击展开的窗口期内先把真实内容蒙住，等克隆图飞到位再露出（lib/expand.ts）。
  // 窗口过了必须把类摘掉：.view--expanding 的 animation 声明特异性更高，
  // 不摘会一直压住 pane-next / pane-prev，滑动动画就永远跑不起来。
  const [veiled, setVeiled] = useState(() => expanding());
  useEffect(() => {
    if (!veiled) return;
    const timer = window.setTimeout(() => setVeiled(false), 520);
    return () => window.clearTimeout(timer);
  }, [veiled]);

  const routeIndex = items.findIndex((entry) => entry.id === item.id);
  const prevId = routeIndex > 0 ? items[routeIndex - 1]?.id : undefined;
  const nextId = routeIndex >= 0 && routeIndex + 1 < items.length ? items[routeIndex + 1]?.id : undefined;

  const panes: (WallItem | undefined)[] = slide
    ? slide.dir === 1
      ? [slide.from, slide.to, undefined]
      : [undefined, slide.to, slide.from]
    : [
        routeIndex > 0 ? items[routeIndex - 1] : undefined,
        item,
        routeIndex >= 0 && routeIndex + 1 < items.length ? items[routeIndex + 1] : undefined,
      ];

  const trackClass =
    'view__track' + (slide ? (slide.dir === 1 ? ' view__track--next' : ' view__track--prev') : '');

  return (
    <article className={'view view--detail' + (veiled ? ' view--expanding' : '')}>
      <div className="view__stage">
        <div className={trackClass} ref={trackRef} onAnimationEnd={commitSlide}>
          {panes.map((pane, paneIndex) =>
            pane ? (
              <Pane
                key={pane.id}
                item={pane}
                items={items}
                paths={paths}
                onOpenDetail={onOpenDetail}
                onTagClick={onTagClick}
              />
            ) : (
              <div className="view__pane" key={'empty-' + paneIndex} aria-hidden="true" />
            ),
          )}
        </div>
      </div>
      <div className="view__scrim" aria-hidden="true" />

      <header className="view__bar">
        <button type="button" className="icon-btn" onClick={onBack} title="返回（Esc）" aria-label="返回">
          <svg width="17" height="17" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M19 12H5" />
            <path d="m11 18-6-6 6-6" />
          </svg>
        </button>
        <div className="view__bar-end">
          <button
            type="button"
            className="view__step"
            onClick={() => prevId && onOpenDetail(prevId)}
            disabled={!prevId}
            title="上一张（←）"
            aria-label="上一张"
          >
            ←
          </button>
          <button
            type="button"
            className="view__step"
            onClick={() => nextId && onOpenDetail(nextId)}
            disabled={!nextId}
            title="下一张（→）"
            aria-label="下一张"
          >
            →
          </button>
          <span className="view__seq">
            {routeIndex >= 0 ? pad(routeIndex + 1) : '--'} / {formatCount(items.length)}
          </span>
        </div>
      </header>
    </article>
  );
}
