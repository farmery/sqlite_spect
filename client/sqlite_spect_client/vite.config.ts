import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [react()],
  base: '/',
  build: {
    outDir: 'dist',
    emptyOutDir: true,
  },
  server: {
    // Proxy /ws to the running CLI so `npm run dev` hot-reloads against a live backend.
    // Start the CLI on port 8123 in another terminal, then open http://localhost:5173
    proxy: {
      '/ws': {
        target: 'ws://127.0.0.1:8123',
        ws: true,
      },
    },
  },
})
