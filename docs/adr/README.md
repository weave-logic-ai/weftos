# Architecture Decision Records

This directory contains Architecture Decision Records (ADRs) for the WeftOS + clawft project, following the MADR 3.0 format. All decisions were derived from the Sprint 11 Symposium (2026-03-27) across 9 tracks plus supporting analysis documents.

## ADR Index

| ADR | Title | Status | Category | Source |
|-----|-------|--------|----------|--------|
| [ADR-001](adr-001-lockstep-semver.md) | Workspace-level lockstep semver versioning | Accepted | Release | Track 3 |
| [ADR-002](adr-002-cargo-dist.md) | cargo-dist for release artifact generation | Accepted | Release | Track 3 |
| [ADR-003](adr-003-codemirror.md) | CodeMirror 6 over Monaco for code editor block | Accepted | GUI | Track 4 |
| [ADR-004](adr-004-no-dockview.md) | No dockview -- CSS Grid + custom Lego engine | Accepted | GUI | Track 4 |
| [ADR-005](adr-005-xterm-js.md) | xterm.js for WeftOS console | Superseded by egui shell | GUI | Track 4 |
| [ADR-006](adr-006-custom-block-renderer.md) | Custom block renderer (json-render pattern) | Accepted | GUI | Track 4 |
| [ADR-007](adr-007-zustand-tauri-events.md) | Zustand + Tauri events for state management | Superseded by egui shell | GUI | Track 4 |
| [ADR-008](adr-008-weftos-cloud-side.md) | WeftOS cloud-side for Mentra (not on-device) | Accepted | Integration | Track 6 |
| [ADR-009](adr-009-sparse-lanczos.md) | Sparse Lanczos for spectral analysis | Accepted | Performance | Track 7 |
| [ADR-010](adr-010-keep-tokio.md) | Keep Tokio (do not adopt Asupersync) | Accepted | Architecture | Track 9 |
| [ADR-011](adr-011-no-frankensearch.md) | Do not add FrankenSearch (raw HNSW sufficient) | Accepted | Performance | Track 9 |
| [ADR-012](adr-012-inline-sha3.md) | Inline sha3 / blake3 fallback for rvf-crypto | Accepted | Release | Tracks 3, 8 |
| [ADR-013](adr-013-json-block-descriptor.md) | JSON block descriptor architecture | Superseded by egui shell | GUI | Track 4, Design Notes |
| [ADR-014](adr-014-fumadocs.md) | Fumadocs as single documentation source of truth | Accepted | Documentation | Track 5, Unification Plan |
| [ADR-015](adr-015-three-property-web.md) | Three-property web architecture | Accepted | Documentation | Web Presence Strategy |
| [ADR-016](adr-016-multi-target-theming.md) | Multi-target theming system | Accepted | GUI | Track 4, Theming Spec |
| [ADR-017](adr-017-gepa-prompt-evolution.md) | GEPA prompt evolution for pipeline/learner.rs | Accepted | Architecture | Hermes Analysis |
| [ADR-018](adr-018-hermes-llm-provider.md) | Hermes models as clawft-llm provider | Accepted | Integration | Hermes Analysis |
| [ADR-019](adr-019-registry-trait.md) | Registry trait in clawft-types | Accepted | Architecture | Track 1 |
| [ADR-020](adr-020-chainloggable.md) | ChainLoggable trait for audit gap closure | Accepted | Architecture | Tracks 1, 2, 4, 8 |

## Categories

| Category | ADRs | Description |
|----------|------|-------------|
| **Release** | 001, 002, 012 | Versioning, distribution, and build decisions |
| **GUI** | 003, 004, 005, 006, 007, 013, 016 | UI/UX technology and architecture decisions |
| **Architecture** | 010, 017, 019, 020 | Core system design decisions |
| **Performance** | 009, 011 | Algorithmic and optimization decisions |
| **Integration** | 008, 018 | External system integration decisions |
| **Documentation** | 014, 015 | Documentation and web presence decisions |

## Decision Sources

All decisions were produced during or immediately after the Sprint 11 Symposium (2026-03-27):

- **Track 1**: Code Pattern Extraction (ADR-019, ADR-020)
- **Track 3**: Release Engineering (ADR-001, ADR-002, ADR-012)
- **Track 4**: UI/UX Design Summit (ADR-003 through ADR-007, ADR-013, ADR-016)
- **Track 5**: Changelog and Documentation (ADR-014)
- **Track 6**: Mentra Integration (ADR-008)
- **Track 7**: Algorithmic Optimization (ADR-009)
- **Track 9**: Optimization Plan (ADR-010, ADR-011)
- **Hermes Integration Analysis**: ADR-017, ADR-018
- **Web Presence Strategy**: ADR-015
- **Theming System Spec**: ADR-016
- **Fumadocs Unification Plan**: ADR-014

## Adding New ADRs

1. Create a new file: `adr-NNN-short-title.md`
2. Use the template format (Context / Decision / Consequences)
3. Add the entry to the index table above
4. Set status to `Proposed` until reviewed and accepted
