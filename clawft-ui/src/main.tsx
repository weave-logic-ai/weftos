import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'

/**
 * Register the PWA service worker (WEFT-311).
 *
 * Skipped in development (Vite dev server already handles HMR + the
 * MSW worker conflicts with shell caching) and when MSW is explicitly
 * requested for offline mocking.
 */
function registerServiceWorker() {
  if (!('serviceWorker' in navigator)) return;
  if (import.meta.env.DEV) return;
  if (import.meta.env.VITE_MOCK_API === 'true') return;

  // Defer to `load` so SW registration doesn't compete with first paint.
  window.addEventListener('load', () => {
    navigator.serviceWorker
      .register('/sw.js', { scope: '/' })
      .catch((err) => {
        console.warn('[clawft] SW registration failed:', err);
      });
  });
}

async function boot() {
  // Start MSW mock API only when explicitly requested via VITE_MOCK_API=true.
  if (import.meta.env.VITE_MOCK_API === 'true') {
    const { worker } = await import('./mocks/browser');
    await worker.start({ onUnhandledRequest: 'bypass' });
  }

  registerServiceWorker();

  createRoot(document.getElementById('root')!).render(
    <StrictMode>
      <App />
    </StrictMode>,
  );
}

boot();
