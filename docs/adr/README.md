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
| [ADR-021](adr-021-cli-kernel-compliance.md) | CLI commands must route through kernel daemon | Accepted | Architecture | Sprint 14 |
| [ADR-022](adr-022-exochain-mandatory-audit.md) | All state-changing operations must log to ExoChain | Accepted | Architecture | Sprint 14 |
| [ADR-023](adr-023-assessment-as-kernel-service.md) | Assessment as a kernel service | Accepted | Architecture | Sprint 16 |
| [ADR-024](adr-024-noise-protocol-encryption.md) | Noise protocol for mesh encryption | Accepted | Security | K6 |
| [ADR-025](adr-025-ed25519-node-identity.md) | Ed25519 node identity | Accepted | Security | K6 |
| [ADR-026](adr-026-quic-primary-transport.md) | QUIC as primary transport | Accepted | Architecture | K6 |
| [ADR-027](adr-027-selective-libp2p.md) | Selective libp2p adoption | Accepted | Architecture | K6 |
| [ADR-028](adr-028-post-quantum-dual-signing.md) | Mandatory dual signing (Ed25519 + ML-DSA-65) | Accepted | Security | K2 / K5 Symposium |
| [ADR-029](adr-029-rvf-crypto-fork-strategy.md) | weftos-rvf-crypto fork strategy | Accepted | Release | K2 Symposium |
| [ADR-030](adr-030-cbor-exochain-codec.md) | CBOR exochain codec | Accepted | Architecture | K6 |
| [ADR-031](adr-031-rvf-wire-mesh-format.md) | RVF wire mesh format | Accepted | Architecture | K6 |
| [ADR-032](adr-032-dashmap-concurrency.md) | DashMap for concurrent registry | Accepted | Performance | K2 |
| [ADR-033](adr-033-three-branch-governance.md) | Three-branch governance model | Accepted | Architecture | Governance |
| [ADR-034](adr-034-effect-algebra-scoring.md) | Effect-algebra scoring | Accepted | Architecture | Governance |
| [ADR-035](adr-035-serviceapi-layered-protocol.md) | ServiceApi layered protocol | Accepted | Architecture | K3 |
| [ADR-036](adr-036-hierarchical-tool-registry.md) | Hierarchical tool registry | Accepted | Architecture | K3 |
| [ADR-037](adr-037-rust-edition-2024-msrv.md) | Rust edition 2024 / MSRV policy | Accepted | Release | Sprint 14 |
| [ADR-038](adr-038-tauri-desktop-shell.md) | Tauri for desktop shell | Accepted | GUI | GUI Track |
| [ADR-039](adr-039-swim-failure-detection.md) | SWIM failure detection | Accepted | Architecture | K6 |
| [ADR-040](adr-040-lww-crdt-process-table.md) | LWW-CRDT for process table | Accepted | Architecture | K6 |
| [ADR-041](adr-041-chainanchor-trait.md) | ChainAnchor trait | Accepted | Architecture | K4 |
| [ADR-042](adr-042-three-operating-modes.md) | Three operating modes | Accepted | Architecture | Sprint 16 |
| [ADR-043](adr-043-blake3-shake256-migration.md) | BLAKE3 / SHAKE-256 migration | Accepted | Security | Sprint 16 |
| [ADR-044](adr-044-wasm-wasip1-target.md) | WASM wasip1 target (alias for wasip2) | Accepted | Release | Sprint 14 |
| [ADR-045](adr-045-tiered-router-permissions.md) | Tiered router permissions | Accepted | Architecture | Sprint 16 |
| [ADR-046](adr-046-forest-of-trees-architecture.md) | Forest-of-trees architecture | Accepted | Architecture | Sprint 16 |
| [ADR-047](adr-047-self-calibrating-tick.md) | Self-calibrating cognitive tick | Accepted | Architecture | DEMOCRITUS |
| [ADR-048](adr-048-kernel-phase-responsibilities.md) | Kernel phase (K-level) responsibilities | Accepted | Architecture | Sprint 14 (formerly ADR-020 — renumbered 2026-04-28 / WEFT-140) |
| [ADR-049](adr-049-weftos-kernel.md) | WeftOS kernel architecture overview | Accepted | Architecture | K0 (formerly `architecture/adr-028-weftos-kernel.md` — renumbered + relocated 2026-04-28 / WEFT-140) |
| [ADR-053](adr-053-voice-stt-canonical-path.md) | Voice STT canonical path — substrate-side whisper | Accepted | Architecture | 0.7.0 release-gate audit (WEFT-205) |
| [ADR-054](adr-054-claude-flow-integration.md) | claude-flow integration — user-installed, not first-party | Accepted | Integration | 0.7.0 release-gate audit (WEFT-488) |
| [ADR-055](adr-055-backend-adapter-contract.md) | BackendAdapter contract for the agent dashboard | Accepted | GUI | 0.7.0 release-gate audit (WEFT-319) |

## Categories

| Category | ADRs | Description |
|----------|------|-------------|
| **Release** | 001, 002, 012, 029, 037, 044 | Versioning, distribution, and build decisions |
| **GUI** | 003, 004, 005, 006, 007, 013, 016, 038, 055 | UI/UX technology and architecture decisions |
| **Architecture** | 010, 017, 019, 020, 021, 022, 023, 026, 027, 030, 031, 033, 034, 035, 036, 039, 040, 041, 042, 045, 046, 047, 048, 049, 053 | Core system design decisions |
| **Security** | 024, 025, 028, 043 | Cryptography, identity, and chain-integrity decisions |
| **Performance** | 009, 011, 032 | Algorithmic and optimization decisions |
| **Integration** | 008, 018, 054 | External system integration decisions |
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
