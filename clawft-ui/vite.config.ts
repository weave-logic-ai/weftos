import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { fileURLToPath } from 'node:url'

export default defineConfig({
  plugins: [react(), tailwindcss()],

  server: {
    port: 5173,
    proxy: {
      '/api': {
        target: 'http://localhost:18789',
        changeOrigin: true,
      },
      '/ws': {
        target: 'ws://localhost:18789',
        ws: true,
      },
    },
    headers: {
      // Required for WASM in development mode
      'Cross-Origin-Opener-Policy': 'same-origin',
      'Cross-Origin-Embedder-Policy': 'require-corp',
    },
  },

  optimizeDeps: {
    // Exclude WASM glue from pre-bundling (loaded dynamically)
    exclude: ['clawft_wasm'],
  },

  build: {
    // Enable top-level await for WASM initialization
    target: 'es2022',
    rollupOptions: {
      // Two entries: the dashboard SPA and the standalone /playground page
      // (ADR-102 D2), which must not share the SPA's token storage or SW.
      input: {
        main: fileURLToPath(new URL('./index.html', import.meta.url)),
        playground: fileURLToPath(new URL('./playground.html', import.meta.url)),
      },
    },
  },

  // Ensure .wasm files are served with the correct MIME type
  assetsInclude: ['**/*.wasm'],
})
