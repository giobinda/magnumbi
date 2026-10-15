import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// In dev, the Vite server proxies /api to the Magnum BI backend (Rust, :8000). changeOrigin stays false so the
// backend sees the browser's Host (localhost:5173): sign-in callback URLs and cookies must use
// the address the user is actually on.
export default defineConfig({
  plugins: [react()],
  server: { port: 5173, proxy: { '/api': { target: 'http://127.0.0.1:8000', changeOrigin: false } } },
})
