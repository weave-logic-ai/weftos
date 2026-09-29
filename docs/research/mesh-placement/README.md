# Governed workload placement (cogs first, then inference and accelerators)

Design for a mesh-wide, governed placement layer. Cogs are the first workload kind, local inference the second, accelerator jobs (NPU/TPU/TSU) later.
- ADRs (all Proposed, 2026-09-28): [ADR-099 placement layer](../../adr/adr-099-governed-workload-placement.md), [COG-001 cog kind](../../adr/cog-001-cog-workload-kind.md), [ADR-101 inference kind and local-hosting migration](../../adr/adr-101-inference-workload-kind.md)
- Goal and cards: [`goal-and-cards.md`](goal-and-cards.md) (formatted for `scripts/dashboard-board.mjs create`; none created yet)
- Container findings (Apple Silicon, 2026-09-28): OrbStack runs aarch64 natively and armv7 emulated; Apple `container` 1.0 runs aarch64 only (armv7 gives Exec format error). Of 107 aarch64 cogs run with `--once` against a fake UDP feed and stub ingest: 93 clean, 5 persistent-listener health cogs need `--interval`, 7 need seed peers or other CLI; `presence-field` has no aarch64 build. The x86_64 dev host is not an ARM target.
