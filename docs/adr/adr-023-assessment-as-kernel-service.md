# ADR-023: Assessment as a Kernel Service

**Date**: 2026-04-03 (Proposed) / 2026-04-28 (Accepted)
**Status**: Accepted (as of 2026-04-28, WEFT-141)
**Deciders**: Architecture review, Sprint 14; accepted at the 0.7.0
release-gate now that the dependent mesh-assess code (sprint 16,
v0.6.13–v0.6.19) has shipped against this decision.
**Depends-On**: ADR-048 (Kernel Phase Responsibilities — formerly
ADR-020, renumbered 2026-04-28 / WEFT-140), ADR-021 (CLI Kernel
Compliance), ADR-022 (ExoChain Mandatory Audit)

## Acceptance rationale (2026-04-28)

The kernel-governance audit
(`.planning/reviews/0.7.0-release-gate/02-kernel-governance.md`)
records that mesh assessment transport (mesh-assess, AssessmentSync
frame 0x0E) ships under this ADR's design and is wired into boot at
phase 5d. The shipping code is therefore the source-of-truth and the
ADR is flipped from "Proposed" to "Accepted" without further design
revision. No outstanding open questions block acceptance.

## Context

`weft assess` currently performs file scanning, analysis, and reporting directly in the CLI process. Per ADR-021, this must be migrated to a kernel service. The assessment workflow also needs to spawn sub-agents (AI assessor, tree-sitter parser, git miner) which requires the kernel's supervisor.

## Decision

Assessment becomes a `SystemService` in the kernel, registered during boot when the `assessment` feature gate is enabled.

### Service Architecture

```
AssessmentService (SystemService)
  ├── FileScanner        — walks directories, respects capability gates
  ├── AnalyzerRegistry   — pluggable analyzers (extensible)
  │     ├── ComplexityAnalyzer     (built-in)
  │     ├── DependencyAnalyzer     (built-in)
  │     ├── TechDebtAnalyzer       (built-in)
  │     ├── SecurityAnalyzer       (delegates to clawft-security)
  │     ├── TopologyAnalyzer       (discovers services, queues, endpoints)
  │     ├── NetworkAnalyzer        (maps egress, firewalls, DNS)
  │     ├── DataSourceAnalyzer     (databases, caches, object stores)
  │     ├── LlmAssessorAgent       (spawned via supervisor, LLM-powered)
  │     └── ... (future: custom analyzers via WASM plugins)
  ├── ReportGenerator    — formats findings (table, JSON, GitHub annotations)
  ├── PeerCoordinator    — cross-project link/compare (local + HTTP)
  └── ChainLogger        — ExoChain event for every assessment operation
```

### Daemon RPC Endpoints

| RPC Method | Description |
|-----------|-------------|
| `assess.run` | Trigger assessment with scope + format |
| `assess.status` | Return last assessment report |
| `assess.init` | Initialize .weftos/ (bootstrap exception — also works CLI-side) |
| `assess.link` | Register a peer project |
| `assess.peers` | List linked peers |
| `assess.compare` | Cross-project comparison |
| `assess.analyzers` | List registered analyzers |

### Analyzer Plugin Interface

Each analyzer implements a trait:

```rust
pub trait Analyzer: Send + Sync {
    /// Unique identifier for this analyzer.
    fn id(&self) -> &str;

    /// Human-readable name.
    fn name(&self) -> &str;

    /// Finding categories this analyzer produces.
    fn categories(&self) -> &[&str];

    /// Run analysis on the given files within a project context.
    fn analyze(
        &self,
        project: &Path,
        files: &[PathBuf],
        context: &AnalysisContext,
    ) -> Vec<Finding>;
}
```

This enables progressive discovery: a `TopologyAnalyzer` can discover RabbitMQ exchanges by reading config files, then the service can spawn a follow-up scan targeting the discovered endpoints. Each discovery broadens the graph.

### Progressive Discovery Pattern

```
Initial scan (files) → discovers infrastructure references
  → TopologyAnalyzer finds rabbitmq.conf, docker-compose.yml
    → discovers queue names, service endpoints
      → NetworkAnalyzer probes discovered endpoints
        → DataSourceAnalyzer finds database connection strings
          → each discovery adds nodes to the knowledge graph
            → next assessment has richer context
```

## Consequences

### Positive
- All assessment operations go through governance gates
- Every file read is logged to ExoChain
- Analyzers are pluggable — add RabbitMQ, Kubernetes, Terraform analyzers over time
- LLM assessor agent spawned via supervisor with proper capabilities
- Progressive discovery builds depth over repeated assessments

### Negative
- Requires running kernel for assessment (except bootstrap `init`)
- More complex than the current CLI-only approach
- Analyzer interface needs stabilization before third-party plugins

### Neutral
- Current CLI code can be reused inside the service — logic doesn't change, just the entry point
- Feature-gated: projects that don't use assessment don't pay for it
