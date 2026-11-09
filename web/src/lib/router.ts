import { useEffect, useState } from 'react';

export type Route =
  | { view: 'wall'; tag: string | null }
  | { view: 'tags' }
  | { view: 'tag'; tag: string }
  | { view: 'detail'; id: string };

function safeDecode(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

/**
 * 四种地址：首页随机墙（#/）、标签总览（#/tags）、单标签墙（#/t/<tag>）、
 * 详情（#/d/<id>）。hash 路由保证静态托管零配置。
 */
export function parseRoute(hash: string): Route {
  const clean = hash.replace(/^#/, '');
  // 详情地址以 #/d/<id> 为准；#/a/<id> 是 v3 早期的写法，保留为兼容别名。
  const detail = /^\/d\/([^/?#]+)/.exec(clean) ?? /^\/a\/([^/?#]+)/.exec(clean);
  if (detail) return { view: 'detail', id: safeDecode(detail[1]) };
  const tagWall = /^\/t\/([^/?#]+)/.exec(clean);
  if (tagWall) return { view: 'tag', tag: safeDecode(tagWall[1]) };
  if (/^\/tags\/?$/.test(clean)) return { view: 'tags' };
  const query = clean.indexOf('?');
  if (query >= 0) {
    const params = new URLSearchParams(clean.slice(query + 1));
    const tag = params.get('tag');
    return { view: 'wall', tag: tag && tag.length > 0 ? tag : null };
  }
  return { view: 'wall', tag: null };
}

export function wallHref(tag?: string | null): string {
  return tag ? '#/?tag=' + encodeURIComponent(tag) : '#/';
}

export function tagsHref(): string {
  return '#/tags';
}

export function tagHref(tag: string): string {
  return '#/t/' + encodeURIComponent(tag);
}

export function detailHref(id: string): string {
  return '#/d/' + encodeURIComponent(id);
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
