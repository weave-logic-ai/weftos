# ADR-013: JSON Block Descriptor Architecture (json-render + A2UI Synthesis)

**Date**: 2026-03-28
**Status**: Superseded by egui shell (2026-04-28)
**Deciders**: Sprint 11 Symposium Track 4 + Design Notes

> **Superseded note (2026-04-28, WEFT-242)**: The JSON block descriptor system described below was retired in [0.6.19] alongside the Tauri+React stack. WeftOS now uses the egui canon (`crates/clawft-gui-egui/`) for in-process rendering with 21 canon primitives (`ui://stack`, `ui://field`, `ui://heatmap`, `ui://waveform`, etc.) wired through the surface IR composer in `clawft-surface`. The flat-adjacency JSON descriptor + Zod catalog model has been replaced by the surface description IR + composer. See ADR-001 row-aligned canon primitives. This ADR is retained for historical context only.

## Context

WeftOS needs a single UI description format that renders across 8+ targets: React (Tauri desktop), Terminal (xterm.js/Ink), 3D (React Three Fiber), Voice (TTS), Mentra HUD (400x240 constraint), MCP (tool outputs), PDF (reports), and Shell (plain text). Two validated open-source approaches exist: vercel-labs/json-render (spec-centric, flat adjacency list, catalog + registry) and google/a2ui (protocol-centric, streaming typed messages, surface lifecycle).

## Decision

Adopt a JSON block descriptor architecture that synthesizes json-render's spec model with A2UI's streaming protocol, backed by the WeftOS kernel command set as the action catalog. Each block is a JSON descriptor with:

- **Flat adjacency list** of elements (json-render pattern)
- **`$state` bindings** that resolve to kernel state paths (`/kernel/metrics/cpu_percent`)
- **Catalog validation** via Zod schemas constraining AI generation
- **Registry** mapping element types to renderer implementations per target
- **Actions** that map to kernel commands via ShellAdapter

18 block types defined in v0.2.0 schema: Column, Row, Metric, DataTable, ConsolePan, CausalGraph, Button, Text, CodeEditor, DiffView, ChainTimeline, GovernanceStatus, AgentCard, ResourceBrowser, WebBrowserPane, JourneyStep, ApprovalGate, Alert.

## Consequences

### Positive
- One descriptor, 8+ rendering targets
- Agents generate validated JSON, not arbitrary code
- Composable: blocks nest and connect via `$state` references
- Descriptor format is versioned independently from renderers
- Enables the Mentra HUD and terminal renderers from the same source

### Negative
- Custom format means no off-the-shelf tools for editing descriptors
- Must maintain catalog schemas and registry mappings for each target

### Neutral
- v0.2.0 schema documented in `docs/weftos/specs/block-catalog.md`
- Descriptor validation uses Zod on the frontend and serde on the backend
