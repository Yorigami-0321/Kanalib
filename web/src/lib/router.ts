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
  | { view: 'detail'; id: string };

function safeDecode(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

/**
 * 地址：首页随机墙（#/）、标签总览/单标签墙（#/tags、#/t/<tag>）、
 * 收藏总览/收藏墙（#/favs、#/fav）、分级总览/分级墙（#/ratings、#/r/<G|S|Q|E>）、
 * 分组总览/分组墙（#/groups、#/g/<group>）、详情（#/d/<id>）。
 * 四个入口共用「封面总览 → 二级墙」同一套结构（photo-wall-v3 Q8）。
 */
export function parseRoute(hash: string): Route {
  const clean = hash.replace(/^#/, '');
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
  return { view: 'wall' };
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
