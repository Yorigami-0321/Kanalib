const probeCache = new Map<string, Promise<boolean>>();

/** 探测一个图片 URL 是否真的存在（用于跨档位回退）；结果按 URL 缓存。 */
export function probeImage(url: string, timeoutMs = 1800): Promise<boolean> {
  if (!url) return Promise.resolve(false);
  const cached = probeCache.get(url);
  if (cached) return cached;
  const promise = new Promise<boolean>((resolve) => {
    const image = new Image();
    let settled = false;
    const finish = (ok: boolean) => {
      if (settled) return;
      settled = true;
      image.onload = null;
      image.onerror = null;
      resolve(ok);
    };
    const timer = window.setTimeout(() => finish(false), timeoutMs);
    image.onload = () => {
      window.clearTimeout(timer);
      finish(image.naturalWidth > 0);
    };
    image.onerror = () => {
      window.clearTimeout(timer);
      finish(false);
    };
    image.src = url;
    if (image.complete) {
      window.clearTimeout(timer);
      finish(image.naturalWidth > 0);
    }
  });
  probeCache.set(url, promise);
  return promise;
}

/** 依次探测候选 URL，返回第一个可用的；全部不可用返回 null。 */
export async function firstAvailable(urls: readonly (string | undefined)[]): Promise<string | null> {
  for (const url of urls) {
    if (!url) continue;
    if (await probeImage(url)) return url;
  }
  return null;
}
