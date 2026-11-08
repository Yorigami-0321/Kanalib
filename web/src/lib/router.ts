import { useEffect, useState } from 'react';

export type Route = { view: 'wall'; tag: string | null } | { view: 'detail'; id: string };

/** `#/`（可带 `?tag=xxx`）与 `#/a/<id>` 两种地址；hash 路由保证静态托管零配置。 */
export function parseRoute(hash: string): Route {
  const clean = hash.replace(/^#/, '');
  const detail = /^\/a\/([^/?#]+)/.exec(clean);
  if (detail) {
    let id = detail[1];
    try {
      id = decodeURIComponent(detail[1]);
    } catch {
      /* 非法转义按原文处理 */
    }
    return { view: 'detail', id };
  }
  const query = clean.indexOf('?');
  if (query >= 0) {
    const params = new URLSearchParams(clean.slice(query + 1));
    const tag = params.get('tag');
    return { view: 'wall', tag: tag && tag.length > 0 ? tag : null };
  }
  return { view: 'wall', tag: null };
}

export function wallHref(tag?: string | null): string {
  return tag ? `#/?tag=${encodeURIComponent(tag)}` : '#/';
}

export function detailHref(id: string): string {
  return `#/a/${encodeURIComponent(id)}`;
}

export function navigate(href: string): void {
  if (window.location.hash === href) return;
  window.location.hash = href;
}

/** 订阅 hashchange；返回当前路由。 */
export function useRoute(): Route {
  const [route, setRoute] = useState<Route>(() => parseRoute(window.location.hash));
  useEffect(() => {
    const onChange = () => setRoute(parseRoute(window.location.hash));
    window.addEventListener('hashchange', onChange);
    return () => window.removeEventListener('hashchange', onChange);
  }, []);
  return route;
}
