import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

const apiProxyTarget = process.env.VITE_API_PROXY_TARGET ?? process.env.VITE_API_BASE ?? 'http://127.0.0.1:12457'

// https://vite.dev/config/
export default defineConfig({
  plugins: [vue()],
  server: {
    proxy: {
      '/api': {
        target: apiProxyTarget,
        changeOrigin: true,
      },
    },
  },
})
