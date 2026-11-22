export * from './types';
export { API_ROOT, DATA_ROOT, SOURCE_KIND, apiUrl, dataUrl, mergePaths } from './config';
export { StaticSource, DataError, normalizeItem } from './StaticSource';
export { ApiSource } from './ApiSource';

import { API_ROOT, DATA_ROOT, SOURCE_KIND } from './config';
import { ApiSource } from './ApiSource';
import { StaticSource } from './StaticSource';
import type { WallSource } from './types';

/** 构建期选择数据源：`VITE_WALL_SOURCE=api` 走动态后端，其余走静态分片。 */
export function createSource(): WallSource {
  return SOURCE_KIND === 'api' ? new ApiSource(API_ROOT) : new StaticSource(DATA_ROOT);
}
