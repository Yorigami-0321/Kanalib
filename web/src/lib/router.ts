import { useEffect, useState } from 'react';

export type Route =
  | { view: 'wall' }
  | { view: 'tags' }
  | { view: 'tag'; tag: string }
  | { view: 'favs' }
  | { view: 'fav' }
  | { view: 'ratings' }
  | { view: 'rating'; rating: string }
  | { view: 'groups' }
  | { view: 'group'; group: string }
  | { view: 'detail'; id: string }
  | { view: 'notfound' };

/** 浏览状态里跟排序有关的取值；与 Sidebar 的 SortMode 是同一组字面量。 */
export type SortParam = 'random' | 'time' | 'name' | 'stars';

/** 写进 hash query 的浏览状态：刷新、复制链接、前进后退都要原样恢复。 */
export interface UiParams {
  q: string;
  sort: SortParam;
  mono: boolean;
  matte: boolean;
}

export const DEFAULT_UI: UiParams = { q: '', sort: 'random', mono: false, matte: false };

const SORTS: readonly SortParam[] = ['random', 'time', 'name', 'stars'];

function safeDecode(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

/** 把 `#/path?query` 拆成路径与参数；路径匹配不受 query 干扰。 */
function splitHash(hash: string): { path: string; params: URLSearchParams } {
  const clean = hash.replace(/^#/, '');
  const at = clean.indexOf('?');
  if (at < 0) return { path: clean, params: new URLSearchParams() };
  return { path: clean.slice(0, at), params: new URLSearchParams(clean.slice(at + 1)) };
}

function flag(value: string | null): boolean {
  if (value === null) return false;
  return value !== '0' && value.toLowerCase() !== 'false';
}

export function parseUiParams(hash: string): UiParams {
  const { params } = splitHash(hash);
  const sortRaw = params.get('sort');
  const sort = SORTS.includes(sortRaw as SortParam) ? (sortRaw as SortParam) : 'random';
  return {
    q: params.get('q') ?? '',
    sort,
    mono: flag(params.get('mono')),
    matte: flag(params.get('matte')),
  };
}

function uiQuery(ui: UiParams): string {
  const params = new URLSearchParams();
  if (ui.q.trim().length > 0) params.set('q', ui.q);
  if (ui.sort !== 'random') params.set('sort', ui.sort);
  if (ui.mono) params.set('mono', '1');
  if (ui.matte) params.set('matte', '1');
  const search = params.toString();
  return search.length > 0 ? '?' + search : '';
}

/** 目标路径 + 当前浏览状态。所有跳转都保留 q/sort/mono/matte。 */
function withUi(href: string): string {
  const path = splitHash(href).path || '/';
  return '#' + path + uiQuery(parseUiParams(window.location.hash));
}

/** 最后一次由应用写入的 hash：区分程序化跳转与用户前进/后退/手改地址。 */
let lastWritten = typeof window === 'undefined' ? '' : window.location.hash;

/**
 * 地址：首页随机墙（#/）、标签总览/单标签墙（#/tags、#/t/<tag>）、
 * 收藏总览/收藏墙（#/favs、#/fav）、分级总览/分级墙（#/ratings、#/r/<G|S|Q|E>）、
 * 分组总览/分组墙（#/groups、#/g/<group>）、详情（#/d/<id>）。
 * 其余非空路径归入 notfound，由 App 渲染轻量缺失页，不再静默回落首页。
 * query 部分（q/sort/mono/matte）由 `parseUiParams` 处理，不参与路径匹配。
 */
export function parseRoute(hash: string): Route {
  const clean = splitHash(hash).path;
  if (clean === '' || clean === '/') return { view: 'wall' };
  // 详情地址以 #/d/<id> 为准；#/a/<id> 是 v3 早期的写法，保留为兼容别名。
  const detail = /^\/d\/([^/?#]+)/.exec(clean) ?? /^\/a\/([^/?#]+)/.exec(clean);
  if (detail) return { view: 'detail', id: safeDecode(detail[1]) };
  const tagWall = /^\/t\/([^/?#]+)/.exec(clean);
  if (tagWall) return { view: 'tag', tag: safeDecode(tagWall[1]) };
  const ratingWall = /^\/r\/([^/?#]+)/.exec(clean);
  if (ratingWall) return { view: 'rating', rating: safeDecode(ratingWall[1]) };
  const groupWall = /^\/g\/([^/?#]+)/.exec(clean);
  if (groupWall) return { view: 'group', group: safeDecode(groupWall[1]) };
  if (/^\/tags\/?$/.test(clean)) return { view: 'tags' };
  if (/^\/favs\/?$/.test(clean)) return { view: 'favs' };
  if (/^\/fav\/?$/.test(clean)) return { view: 'fav' };
  if (/^\/ratings\/?$/.test(clean)) return { view: 'ratings' };
  if (/^\/groups\/?$/.test(clean)) return { view: 'groups' };
  return { view: 'notfound' };
}

export function wallHref(): string {
  return '#/';
}

export function tagsHref(): string {
  return '#/tags';
}

export function tagHref(tag: string): string {
  return '#/t/' + encodeURIComponent(tag);
}

export function favsHref(): string {
  return '#/favs';
}

export function favHref(): string {
  return '#/fav';
}

export function ratingsHref(): string {
  return '#/ratings';
}

export function ratingHref(rating: string): string {
  return '#/r/' + encodeURIComponent(rating);
}

export function groupsHref(): string {
  return '#/groups';
}

export function groupHref(group: string): string {
  return '#/g/' + encodeURIComponent(group);
}

export function detailHref(id: string): string {
  return '#/d/' + encodeURIComponent(id);
}

/** 普通跳转（新增一条历史）。保留当前浏览状态。 */
export function navigate(href: string): void {
  const next = withUi(href);
  if (window.location.hash === next) return;
  lastWritten = next;
  window.location.hash = next;
}

/**
 * 应用内部的路由变更事件。
 * `history.replaceState` 只改地址，不产生 hashchange；靠它显式通知 useRoute，
 * 否则详情内翻页（replace）会出现「URL 换了、画面没换」。
 * 不补发原生 hashchange：useUiParamsSync 会把它当成用户操作，与 lastWritten 去重逻辑纠缠。
 */
const ROUTE_CHANGE = 'kanalib:routechange';

/** 详情内翻页/放映：改写当前历史条目，一次后退直接回列表。 */
export function replaceNavigate(href: string): void {
  const next = withUi(href);
  lastWritten = next;
  if (window.location.hash === next) return;
  window.history.replaceState(null, '', next);
  window.dispatchEvent(new Event(ROUTE_CHANGE));
}

/** 把 q/sort/mono/matte 写回当前 hash（replace，不产生历史条目）。 */
export function writeUiParams(ui: UiParams): void {
  const path = splitHash(window.location.hash).path || '/';
  const query = uiQuery(ui);
  // 裸地址（没有 hash）且全是默认值时保持原样，避免仅仅打开首页就多出一截 #/。
  if (window.location.hash === '' && query.length === 0) {
    lastWritten = '';
    return;
  }
  const next = '#' + path + query;
  if (window.location.hash === next) {
    lastWritten = next;
    return;
  }
  lastWritten = next;
  window.history.replaceState(null, '', next);
}

/**
 * 浏览器前进/后退或用户手改地址时，把 URL 上的浏览状态喂回 React。
 * 应用自己写入的 hash 会跳过，避免与状态更新互相打架。
 */
export function useUiParamsSync(onChange: (ui: UiParams) => void): void {
  useEffect(() => {
    const sync = () => {
      const hash = window.location.hash;
      if (hash === lastWritten) return;
      lastWritten = hash;
      onChange(parseUiParams(hash));
    };
    window.addEventListener('popstate', sync);
    window.addEventListener('hashchange', sync);
    return () => {
      window.removeEventListener('popstate', sync);
      window.removeEventListener('hashchange', sync);
    };
  }, [onChange]);
}

/** 订阅 hashchange；返回当前路由。 */
export function useRoute(): Route {
  const [route, setRoute] = useState<Route>(() => parseRoute(window.location.hash));
  useEffect(() => {
    const onChange = () => setRoute(parseRoute(window.location.hash));
    window.addEventListener('hashchange', onChange);
    window.addEventListener(ROUTE_CHANGE, onChange);
    return () => {
      window.removeEventListener('hashchange', onChange);
      window.removeEventListener(ROUTE_CHANGE, onChange);
    };
  }, []);
  return route;
}
