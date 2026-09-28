import { Fraunces, JetBrains_Mono } from 'next/font/google';
import './urth.css';
import UrthStory from './UrthStory';

const fraunces = Fraunces({
  subsets: ['latin'],
  axes: ['SOFT', 'WONK', 'opsz'],
  variable: '--font-fraunces',
  display: 'swap',
});

const mono = JetBrains_Mono({
  subsets: ['latin'],
  weight: ['400', '500'],
  variable: '--font-mono-urth',
  display: 'swap',
});

export default function UrthSpatialPage() {
  return (
    <div
      className={`urth-scope ${fraunces.variable} ${mono.variable}`}
      style={{ position: 'relative', overflowX: 'hidden' }}
    >
      <div
        className="urth-grid-bg"
        aria-hidden="true"
        style={{ position: 'fixed', inset: 0, zIndex: 0, pointerEvents: 'none' }}
      />
      <main style={{ position: 'relative', zIndex: 1 }}>
        <UrthStory />
      </main>
    </div>
  );
}
