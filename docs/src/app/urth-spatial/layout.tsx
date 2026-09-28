import type { ReactNode } from 'react';

export const metadata = {
  title: 'Urth · spatial intelligence',
  description:
    'Where WeftOS is with spatial indexes, Urth, and the 2026 world-model landscape. Metric geometry stays in the BVH.',
  openGraph: {
    title: 'Urth · spatial intelligence · WeftOS',
    description:
      'BVH, HNSW, Graph Views, and the 2026 renderer / simulator / planner split.',
    siteName: 'WeftOS',
  },
};

export default function UrthSpatialLayout({ children }: { children: ReactNode }) {
  return <>{children}</>;
}
