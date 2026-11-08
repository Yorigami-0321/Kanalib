import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// 墙是纯静态 SPA：产物必须能放在任意子路径下（相对 base），
// 数据目录由运行时 fetch 决定（见 src/config.ts 的 VITE_WALL_DATA）。
export default defineConfig({
  base: './',
  plugins: [react()],
  build: {
    outDir: 'dist',
    target: 'es2022',
    sourcemap: false,
    assetsInlineLimit: 0,
  },
  server: {
    port: 5273,
    strictPort: false,
    host: '127.0.0.1',
  },
  preview: {
    port: 4273,
    strictPort: false,
    host: '127.0.0.1',
  },
});
