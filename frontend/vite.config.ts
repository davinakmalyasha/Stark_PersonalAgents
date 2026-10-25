import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import { VitePWA } from 'vite-plugin-pwa'

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    // PWA (mobile/remote feature): installable app on the phone. Android
    // Chrome turns this into a real WebAPK on "Add to Home screen".
    VitePWA({
      registerType: 'autoUpdate',
      injectRegister: false,
      includeAssets: ['icons/pwa-192.png', 'icons/pwa-512.png'],
      manifest: {
        name: 'Stark — AI Workspace',
        short_name: 'Stark',
        description: 'Multi-agent AI desktop workspace, remotely controllable from your phone.',
        theme_color: '#0c0c0e',
        background_color: '#0c0c0e',
        display: 'standalone',
        orientation: 'any',
        start_url: '/',
        icons: [
          { src: '/icons/pwa-192.png', sizes: '192x192', type: 'image/png' },
          { src: '/icons/pwa-512.png', sizes: '512x512', type: 'image/png' },
          { src: '/icons/pwa-maskable-192.png', sizes: '192x192', type: 'image/png', purpose: 'maskable' },
          { src: '/icons/pwa-maskable-512.png', sizes: '512x512', type: 'image/png', purpose: 'maskable' },
        ],
      },
      workbox: {
        // Never cache API traffic or SSE streams.
        navigateFallbackDenylist: [/^\/api\//],
        runtimeCaching: [],
        globPatterns: ['**/*.{js,css,html,svg,png,woff2}'],
      },
    }),
  ],
  server: {
    port: 9000,
    strictPort: true,
    proxy: {
      '/api': {
        // Unique port 8123 — no clash with dev servers (8000/5173/etc)
        target: process.env.STARK_PORT ? `http://127.0.0.1:${process.env.STARK_PORT}` : 'http://127.0.0.1:8123',
        changeOrigin: true,
      },
    },
  },
})
