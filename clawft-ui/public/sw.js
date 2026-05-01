/**
 * ClawFT Agent Dashboard service worker (WEFT-311).
 *
 * Provides an offline shell for browser-mode users and pre-caches
 * the WASM binary so subsequent loads are network-free. API/WS
 * traffic is always bypassed — those endpoints are stateful and
 * must hit the live gateway.
 *
 * Cache strategy:
 *   - Navigation requests: network-first with offline shell fallback.
 *   - Static assets (JS/CSS/SVG/WASM under /assets/, /clawft_wasm*):
 *     cache-first, lazily populate on miss.
 *   - Everything matching /api/ or /ws: bypass.
 *
 * Push notifications are intentionally NOT wired here — that requires
 * server-side VAPID setup and is tracked separately.
 */

const CACHE_VERSION = "clawft-shell-v1";
const SHELL_URLS = [
  "/",
  "/index.html",
  "/manifest.webmanifest",
  "/vite.svg",
];

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE_VERSION)
      .then((cache) => cache.addAll(SHELL_URLS))
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(
          keys
            .filter((key) => key !== CACHE_VERSION)
            .map((key) => caches.delete(key)),
        ),
      )
      .then(() => self.clients.claim()),
  );
});

function isApiOrWs(url) {
  return url.pathname.startsWith("/api/") || url.pathname.startsWith("/ws");
}

function isCacheableAsset(url) {
  return (
    url.pathname.startsWith("/assets/") ||
    url.pathname.startsWith("/clawft_wasm") ||
    url.pathname.endsWith(".wasm") ||
    url.pathname.endsWith(".js") ||
    url.pathname.endsWith(".css") ||
    url.pathname.endsWith(".svg") ||
    url.pathname.endsWith(".webmanifest")
  );
}

self.addEventListener("fetch", (event) => {
  const request = event.request;

  // Only handle GETs to our origin.
  if (request.method !== "GET") return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin) return;

  // Always bypass API + WebSocket.
  if (isApiOrWs(url)) return;

  // Navigation: try network first, fall back to cached shell.
  if (request.mode === "navigate") {
    event.respondWith(
      fetch(request).catch(() =>
        caches
          .open(CACHE_VERSION)
          .then((cache) => cache.match("/index.html") || cache.match("/")),
      ),
    );
    return;
  }

  // Static assets / WASM: cache-first, populate on miss.
  if (isCacheableAsset(url)) {
    event.respondWith(
      caches.open(CACHE_VERSION).then((cache) =>
        cache.match(request).then((cached) => {
          if (cached) return cached;
          return fetch(request).then((response) => {
            if (response.ok) cache.put(request, response.clone());
            return response;
          });
        }),
      ),
    );
  }
});
