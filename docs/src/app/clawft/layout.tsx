import type { ReactNode } from 'react';

export const metadata = {
  title: 'clawft Sandbox',
  description:
    'Run WeftOS in your browser. Chat with an AI agent powered by clawft-wasm, backed by an RVF knowledge base of Fumadocs, ADRs, guides, and research.',
};

export default function ClawftLayout({ children }: { children: ReactNode }) {
  return <>{children}</>;
}
