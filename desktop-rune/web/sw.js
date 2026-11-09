// Stark Rune app-shell service worker: the PWA opens offline with the last
// shell available. Mirrors frontend/public/sw.js policy: API responses are
// never cached (chats must be live; the API is cross-origin anyway, so the
// worker never even sees those requests).
//
// `/pkg/` (the wasm bundle) is network-first: a cache-first wasm once served
// a stale CRASHING build indefinitely (CACHE never bumped). App shell art
// stays cache-first for instant offline opens.
const CACHE = 'stark-rune-v2';
const SHELL = [
  './',
  './index.html',
  './manifest.webmanifest',
  './pkg/stark_rune_web.js',
  './pkg/stark_rune_web_bg.wasm',
  './icons/pwa-192.png',
  './icons/pwa-512.png',
];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.addAll(SHELL))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(keys.filter((key) => key !== CACHE).map((key) => caches.delete(key))),
      )
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (event) => {
  const request = event.request;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;

  if (request.mode === 'navigate') {
    event.respondWith(
      fetch(request)
        .then((response) => {
          const copy = response.clone();
          caches.open(CACHE).then((cache) => cache.put('./index.html', copy)).catch(() => {});
          return response;
        })
        .catch(() => caches.match('./index.html')),
    );
    return;
  }

  // Wasm bundle: network-first with cache fallback (fresh code wins,
  // offline still opens).
  if (url.pathname.startsWith('/pkg/')) {
    event.respondWith(
      fetch(request)
        .then((response) => {
          if (response.ok) {
            const copy = response.clone();
            caches.open(CACHE).then((cache) => cache.put(request, copy)).catch(() => {});
          }
          return response;
        })
        .catch(() => caches.match(request)),
    );
    return;
  }

  event.respondWith(
    caches.match(request).then((cached) => {
      if (cached) return cached;
      return fetch(request).then((response) => {
        if (response.ok) {
          const copy = response.clone();
          caches.open(CACHE).then((cache) => cache.put(request, copy)).catch(() => {});
        }
        return response;
      });
    }),
  );
});
