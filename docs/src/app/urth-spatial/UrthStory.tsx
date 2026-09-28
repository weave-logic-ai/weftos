'use client';

import { useRef, type ReactNode } from 'react';
import {
  motion,
  useReducedMotion,
  useScroll,
  useSpring,
  useTransform,
} from 'motion/react';
import {
  CLOSER,
  FUNCTIONS,
  HERO,
  INDEXES,
  LANDSCAPE,
  QUEUE_NEW,
  RESIDUALS,
  SHIPPED,
  THESIS,
} from './copy';
import ExpandCard from './components/ExpandCard';

export default function UrthStory() {
  const reduce = useReducedMotion();
  const pageRef = useRef<HTMLDivElement>(null);
  const { scrollYProgress } = useScroll({ target: pageRef, offset: ['start start', 'end end'] });
  const scaleX = useSpring(scrollYProgress, { stiffness: 120, damping: 30, mass: 0.3 });

  const heroRef = useRef<HTMLElement>(null);
  const { scrollYProgress: heroP } = useScroll({
    target: heroRef,
    offset: ['start start', 'end start'],
  });
  const heroY = useTransform(heroP, [0, 1], reduce ? ['0vh', '0vh'] : ['0vh', '-18vh']);
  const layerA = useTransform(heroP, [0, 1], reduce ? ['0%', '0%'] : ['0%', '28%']);
  const layerB = useTransform(heroP, [0, 1], reduce ? ['0%', '0%'] : ['0%', '12%']);
  const heroOp = useTransform(heroP, [0.15, 0.55], [1, 0]);

  return (
    <div ref={pageRef}>
      <motion.div className="urth-progress" style={{ scaleX }} aria-hidden="true" />

      <section ref={heroRef} style={{ height: reduce ? '100svh' : '165vh', position: 'relative' }}>
        <div
          className="sticky top-0 flex items-center justify-center overflow-hidden"
          style={{ height: '100svh' }}
        >
          <motion.div
            aria-hidden="true"
            style={{
              position: 'absolute',
              inset: '-20%',
              y: layerA,
              background:
                'radial-gradient(ellipse at 50% 40%, color-mix(in oklab, var(--urth-gold) 14%, transparent), transparent 55%)',
              pointerEvents: 'none',
            }}
          />
          <motion.div
            aria-hidden="true"
            className="urth-grid-bg"
            style={{ position: 'absolute', inset: 0, y: layerB, pointerEvents: 'none' }}
          />
          <motion.header
            style={{
              y: heroY,
              opacity: heroOp,
              textAlign: 'center',
              padding: '0 1.5rem',
              maxWidth: '52rem',
              position: 'relative',
              zIndex: 1,
            }}
          >
            <div
              className="urth-mono"
              style={{
                fontSize: '0.78rem',
                color: 'var(--urth-mute)',
                letterSpacing: '0.06em',
                textTransform: 'uppercase',
                marginBottom: '2rem',
              }}
            >
              {HERO.eyebrow}
            </div>
            <h1
              className="urth-editorial"
              style={{
                fontSize: 'clamp(2.6rem, 7vw, 5.2rem)',
                margin: 0,
                letterSpacing: '-0.03em',
                lineHeight: 1.05,
              }}
            >
              {HERO.title}
            </h1>
            <p
              className="urth-mono"
              style={{
                marginTop: '1.2rem',
                color: 'var(--urth-gold)',
                fontSize: 'clamp(0.95rem, 1.6vw, 1.12rem)',
              }}
            >
              {HERO.subtitle}
            </p>
            <p className="urth-mono" style={{ marginTop: '0.5rem', color: 'var(--urth-dim)', fontSize: '0.82rem' }}>
              {HERO.meta}
            </p>
            <p className="urth-mono" style={{ marginTop: '3rem', color: 'var(--urth-mute)', fontSize: '0.78rem' }}>
              ↓  {HERO.scrollHint}
            </p>
          </motion.header>
        </div>
      </section>

      <Scene kicker="S01 · taxonomy" title="Three jobs, one overloaded name.">
        <p style={{ color: 'var(--urth-mute)', maxWidth: '40rem', marginBottom: '1.6rem' }}>{THESIS}</p>
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fit, minmax(16rem, 1fr))',
            gap: '1rem',
          }}
        >
          {FUNCTIONS.map((fn) => (
            <ExpandCard
              key={fn.id}
              kicker={fn.kicker}
              title={fn.title}
              summary={fn.body}
              accent={`var(--urth-${fn.accent === 'mint' ? 'mint' : fn.accent === 'violet' ? 'violet' : 'amber'})`}
            >
              <p style={{ margin: 0 }}>
                <span className="urth-mono" style={{ color: 'var(--urth-gold)' }}>
                  WeftOS seam:{' '}
                </span>
                {fn.weftos}
              </p>
              <p className="urth-mono" style={{ margin: '0.7rem 0 0', fontSize: '0.78rem', color: 'var(--urth-dim)' }}>
                Try this · click another column. You should see · only one job per card. Why it matters · mixing them is how generative rooms become fake maps.
              </p>
            </ExpandCard>
          ))}
        </div>
        <Figure src="/urth-spatial/three-functions.svg" alt="Renderer, simulator, and planner as three columns. Urth treats the simulator as metric truth." />
      </Scene>

      <Scene kicker="S02 · indexes" title="Four lenses. One chain.">
        <p style={{ color: 'var(--urth-mute)', maxWidth: '40rem', marginBottom: '1.6rem' }}>
          ECC already had HNSW, causal edges, and UNID lookup. ADR-056 added the missing fourth: geometric overlap. They compose; they do not compete.
        </p>
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fit, minmax(16rem, 1fr))',
            gap: '1rem',
          }}
        >
          {INDEXES.map((ix) => (
            <ExpandCard
              key={ix.id}
              kicker={ix.status}
              title={ix.title}
              summary={ix.answers}
              accent="var(--urth-cyan)"
            >
              {ix.detail}
            </ExpandCard>
          ))}
        </div>
        <Figure src="/urth-spatial/weftos-indexes.svg" alt="BVH, HNSW, and causal indexes under chain sequence, with Graph Views promoting into the BVH." />
      </Scene>

      <Scene kicker="S03 · dual output" title="Pretty is not the map.">
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fit, minmax(18rem, 1fr))',
            gap: '1.2rem',
          }}
        >
          <div className="urth-panel" style={{ padding: '1.4rem', borderColor: 'var(--urth-amber)' }}>
            <div className="urth-mono" style={{ fontSize: '0.72rem', color: 'var(--urth-amber)' }}>
              Appearance
            </div>
            <h3 className="urth-editorial" style={{ margin: '0.4rem 0' }}>
              splat.sog
            </h3>
            <p style={{ margin: 0, color: 'var(--urth-mute)' }}>
              Humans, Spark-style viewers, Agent Workspace backdrops. Marble Gaussians belong here. Optional cosmetic fill must be labeled non-metric.
            </p>
          </div>
          <div className="urth-panel" style={{ padding: '1.4rem', borderColor: 'var(--urth-mint)' }}>
            <div className="urth-mono" style={{ fontSize: '0.72rem', color: 'var(--urth-mint)' }}>
              Structure
            </div>
            <h3 className="urth-editorial" style={{ margin: '0.4rem 0' }}>
              BVH leaves
            </h3>
            <p style={{ margin: 0, color: 'var(--urth-mute)' }}>
              Object vs Event, AABB, chain evidence. Marble collider meshes rhyme with this column. W1 export exists; live publish does not.
            </p>
          </div>
        </div>
      </Scene>

      <Scene kicker="S04 · join" title="A handle, not a second brain.">
        <p style={{ color: 'var(--urth-mute)', maxWidth: '42rem' }}>
          Every spatial payload may carry an optional VectorRef. Default is none — pure geometry. Feature kNN stays on HNSW. Geometric kNN on the BVH is AABB-center proximity, not cosine. Phase F helpers compose the two without inlining embeddings.
        </p>
      </Scene>

      <Scene kicker="S05 · now" title="Shipped is not the same as live.">
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fit, minmax(18rem, 1fr))',
            gap: '1.5rem',
          }}
        >
          <div>
            <h3 className="urth-mono" style={{ color: 'var(--urth-mint)', fontSize: '0.8rem' }}>
              Done on Plane
            </h3>
            <ul style={{ paddingLeft: '1.1rem', color: 'var(--urth-mute)', lineHeight: 1.7 }}>
              {SHIPPED.map((row) => (
                <li key={row}>{row}</li>
              ))}
            </ul>
          </div>
          <div>
            <h3 className="urth-mono" style={{ color: 'var(--urth-amber)', fontSize: '0.8rem' }}>
              Residual (docs, not board)
            </h3>
            <div style={{ display: 'grid', gap: '0.7rem' }}>
              {RESIDUALS.map((r) => (
                <ExpandCard key={r.title} kicker="open" title={r.title} summary={r.why} accent="var(--urth-amber)" >
                  Plane spatial tickets are all Done. These live as release-review residuals and code comments — they still need tickets if we work them.
                </ExpandCard>
              ))}
            </div>
          </div>
        </div>
        <Figure src="/urth-spatial/urth-lod.svg" alt="Urth LOD from planetary basemap down to object AABBs. Unobserved space stays honest." />
      </Scene>

      <Scene kicker="S06 · 2026" title="What turned up this session.">
        <p style={{ color: 'var(--urth-mute)', maxWidth: '42rem', marginBottom: '1.4rem' }}>
          New papers and products since the 0.8 cut. Click a card for the seam. Deep dives live under docs/research/spatial-intelligence-2026/.
        </p>
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fit, minmax(16rem, 1fr))',
            gap: '1rem',
          }}
        >
          {LANDSCAPE.map((item) => (
            <ExpandCard
              key={item.id}
              kicker={`${item.id} · ${item.pri}`}
              title={item.title}
              summary={item.blurb}
              accent="var(--urth-cyan)"
            >
              <p style={{ margin: '0 0 0.6rem' }}>{item.blurb}</p>
              <a href={item.href} style={{ color: 'var(--urth-gold)' }}>
                Open the deep dive →
              </a>
            </ExpandCard>
          ))}
        </div>
      </Scene>

      <Scene kicker="S07 · queue" title="Directions we are exploring.">
        <Figure src="/urth-spatial/research-queue.svg" alt="Shipped, residual, and new 2026 columns of the spatial research queue." />
        <h3 className="urth-mono" style={{ color: 'var(--urth-cyan)', fontSize: '0.8rem', marginTop: '1.6rem' }}>
          Proposed this session (not filed)
        </h3>
        <ul style={{ paddingLeft: '1.1rem', color: 'var(--urth-mute)', lineHeight: 1.7 }}>
          {QUEUE_NEW.map((row) => (
            <li key={row}>{row}</li>
          ))}
        </ul>
      </Scene>

      <section style={{ padding: '12vh 1.5rem 18vh', textAlign: 'center' }}>
        <h2
          className="urth-editorial"
          style={{ fontSize: 'clamp(2rem, 4vw, 3.2rem)', margin: '0 0 1rem' }}
        >
          {CLOSER.title}
        </h2>
        <p style={{ color: 'var(--urth-mute)', maxWidth: '36rem', margin: '0 auto 2rem' }}>{CLOSER.body}</p>
        <nav style={{ display: 'flex', flexWrap: 'wrap', gap: '0.8rem', justifyContent: 'center' }}>
          {CLOSER.links.map((l) => (
            <a
              key={l.href}
              href={l.href}
              className="urth-badge"
              style={{ color: 'var(--urth-gold)', borderColor: 'var(--urth-gold)' }}
            >
              {l.label}
            </a>
          ))}
        </nav>
        <p className="urth-mono" style={{ marginTop: '3rem', color: 'var(--urth-dim)', fontSize: '0.75rem' }}>
          weftos.weavelogic.ai/urth-spatial · 0.8-metaharness · 2026-09-21
        </p>
      </section>
    </div>
  );
}

function Scene({
  kicker,
  title,
  children,
}: {
  kicker: string;
  title: string;
  children: ReactNode;
}) {
  return (
    <section
      style={{
        padding: '8vh 1.5rem',
        display: 'flex',
        justifyContent: 'center',
      }}
    >
      <div className="urth-scene-grid">
        <header className="urth-scene-head">
          <span className="urth-badge">{kicker}</span>
          <h2
            className="urth-editorial"
            style={{
              marginTop: '0.7rem',
              fontSize: 'clamp(1.6rem, 3vw, 2.4rem)',
              letterSpacing: '-0.02em',
            }}
          >
            {title}
          </h2>
        </header>
        <div className="urth-scene-body">{children}</div>
      </div>
    </section>
  );
}

function Figure({ src, alt }: { src: string; alt: string }) {
  return (
    <figure style={{ margin: '2rem 0 0' }}>
      {/* eslint-disable-next-line @next/next/no-img-element */}
      <img
        src={src}
        alt={alt}
        style={{ width: '100%', height: 'auto', borderRadius: 4, border: '1px solid var(--urth-line)' }}
      />
      <figcaption className="urth-mono" style={{ marginTop: '0.6rem', fontSize: '0.72rem', color: 'var(--urth-dim)' }}>
        {alt}
      </figcaption>
    </figure>
  );
}
