import { defineConfig } from 'vite';
import { fileURLToPath } from 'node:url';

export default defineConfig({
  // Tauri 期望固定端口，且失败要直接报错而不是自动换端口
  server: { port: 1420, strictPort: true },
  clearScreen: false,
  build: {
    target: 'chrome110',
    rollupOptions: {
      input: {
        main: fileURLToPath(new URL('./index.html', import.meta.url)),
        quick: fileURLToPath(new URL('./quick.html', import.meta.url)),
      },
    },
  },
});
