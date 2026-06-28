# WeftOS / clawft — Project Brain

**Compiled**: 2026-06-28 · **Branch at compile**: `feat/weftos-579-591-graduations` · **Workspace version**: 0.6.19 (released) → 0.7.0/0.8.x in flight

This `brain/` directory is a consolidated, cross-referenced map of the entire
project — planned features, shipped features, architecture, decisions, bugs,
phases, and research streams — synthesized from the roadmap, git history,
`.planning/`, ADRs, reviews, and session handoffs.

It is the human-readable half of a two-part "brain". The machine-readable half
lives in the **RVF / ruvector vector store** (queryable by meaning via the
`claude-flow` memory tools, namespaces `weftos/*`) and in **persistent
file-memory** at `~/.claude/projects/-Users-mathewbeane-weftos/memory/`.

## How the brain is structured

The brain mirrors the project's own RVF intelligence layer (see
[`05-rvf-brain-and-research.md`](05-rvf-brain-and-research.md)): each fact is a
small self-contained chunk, tagged with a namespace, a type, and a status, and
linked to its causal parents. That is exactly how WeftOS itself stores memory
in `.rvf` files (VEC + INDEX + HNSW + WITNESS segments).

## Index

| Dimension | File | What it covers |
|---|---|---|
| **Roadmap & Phases** | [`01-roadmap-and-phases.md`](01-roadmap-and-phases.md) | Phase taxonomy (K0–K8, Sprints 08–17, Phases 1–5, version cycles, WEFT-NNN), planned features, timeline, product vision |
| **Releases & Shipped Features** | [`02-release-history-and-features.md`](02-release-history-and-features.md) | Every release 0.1.0→0.6.19, the 10 commit "waves", implemented-feature inventory, WEFT ticket map, recent session narrative |
| **Architecture & ADRs** | [`03-architecture-and-adrs.md`](03-architecture-and-adrs.md) | 44-crate map, K0–K8 kernel layer model, full ADR-001→057 index, key patterns, `weave.toml` config surface |
| **Bugs, Gaps & Current State** | [`04-bugs-gaps-and-current-state.md`](04-bugs-gaps-and-current-state.md) | Known open bugs, audit/review findings, phase gaps, the uncommitted-work hazard, TODO density, operational gotchas |
| **RVF Brain & Research** | [`05-rvf-brain-and-research.md`](05-rvf-brain-and-research.md) | RVF/ruvector primer, ECC cognitive substrate, how the brain is chunked, the sonobuoy/sensors/actors/symposium research streams |

## The one-paragraph picture

WeftOS is a two-layer system. At the **agent layer** it is a high-performance
Rust rewrite of the `nanobot` Python assistant — a single sub-10 MB binary
(`weft`) with multi-channel messaging, a permission-aware tiered LLM router, and
a 6-stage pluggable pipeline. At the **kernel layer** it is a cognitive OS
substrate (44 crates, ~175K lines): process supervision, post-quantum crypto,
three-branch constitutional governance, WASM tool sandboxing, mesh networking
(Kademlia/SWIM/Noise/QUIC/ML-KEM-768), an ExoChain dual-signed audit chain, and
an ECC cognitive substrate (CausalGraph + HNSW + DEMOCRITUS cognitive loop +
Weaver modeler) whose memory is stored in **RVF** files. Kernel layers K0–K6 are
complete; K8 (GUI) is in flight. The most recent work is an uncommitted
vector-first leaf-display pivot for ESP32-S3 hardware.

## ⚠️ Live hazards (read before touching the working tree)

1. **Uncommitted vector-display subsystem** — ~8 new crates (`weftos-leaf-*`,
   `lgfx-bus-rgb-rs`, `weftos-scene-builder`), ~300 tests, 2 ADRs, 1 design doc
   have **zero git history**. `git diff --stat HEAD` ≈ 588 files / +19.5K / −14.4K.
   One `git checkout` from loss. Commit a focused diff before anything else.
2. **Daemon binary swap** — `cp weaver ~/.cargo/bin/` while the daemon runs →
   "Text file busy"; use atomic `mv` + restart. A running daemon keeps the old
   inode and hides new features until restarted.
3. **`.env` shadows `[kernel.llm]`** — `dotenvy` loads `LLM_SERVICE_URL`/
   `LLM_MODEL` before `weave.toml`; stale `.env` values silently win.
4. **Open CRITICAL governance gate gap** — `auth_service.rs` `rotate_credential`
   (L325) and `request_token` (L354) chain-log but do not gate on governance.
