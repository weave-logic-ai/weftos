'use client';

import { useId, useState, type KeyboardEvent, type ReactNode } from 'react';
import { AnimatePresence, motion, useReducedMotion } from 'motion/react';

type Props = {
  kicker: string;
  title: string;
  summary: string;
  children: ReactNode;
  accent?: string;
};

export default function ExpandCard({
  kicker,
  title,
  summary,
  children,
  accent = 'var(--urth-gold)',
}: Props) {
  const [open, setOpen] = useState(false);
  const reduce = useReducedMotion();
  const panelId = useId();

  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      setOpen((v) => !v);
    }
  };

  return (
    <article
      className="urth-panel"
      style={{
        padding: '1.1rem 1.2rem',
        borderColor: open ? accent : undefined,
        cursor: 'pointer',
      }}
      role="button"
      tabIndex={0}
      aria-expanded={open}
      aria-controls={panelId}
      onClick={() => setOpen((v) => !v)}
      onKeyDown={onKey}
    >
      <div className="urth-mono" style={{ fontSize: '0.72rem', color: accent }}>
        {kicker}
      </div>
      <h3
        className="urth-editorial"
        style={{ margin: '0.35rem 0 0.4rem', fontSize: '1.15rem' }}
      >
        {title}
      </h3>
      <p style={{ margin: 0, color: 'var(--urth-mute)', fontSize: '0.92rem' }}>
        {summary}
      </p>
      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            id={panelId}
            initial={reduce ? false : { height: 0, opacity: 0 }}
            animate={{ height: 'auto', opacity: 1 }}
            exit={reduce ? { opacity: 1 } : { height: 0, opacity: 0 }}
            transition={{ duration: 0.35, ease: [0.16, 1, 0.3, 1] }}
            style={{ overflow: 'hidden' }}
          >
            <div
              style={{
                marginTop: '0.9rem',
                paddingTop: '0.9rem',
                borderTop: '1px solid var(--urth-line)',
                color: 'var(--urth-ink)',
                fontSize: '0.92rem',
                lineHeight: 1.55,
              }}
              onClick={(e) => e.stopPropagation()}
              onKeyDown={(e) => e.stopPropagation()}
            >
              {children}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </article>
  );
}
