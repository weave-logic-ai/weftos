# Plane close-comment template

Required body when transitioning a work item to `Done`. The wrapper's
`scripts/plane.sh close <id> --shipped ... --commits ... --tests ...
--build ... --followups ...` builds this for you, but here's the raw
shape if you're posting by hand.

```markdown
**Closed: <YYYY-MM-DD>**

**Shipped**
<one-paragraph summary of what landed; what behaviour is now real>

**Commit(s)**
- <short SHA> <subject>
- <short SHA> <subject>

**Tests**
- `scripts/build.sh test` (or specific `cargo test -p <crate>` invocation)
- N pass / 0 fail (or "<N> pass; <flaky test name> intermittent —
  see WEFT-XX")

**Build gate**
- `scripts/build.sh check` clean
- `scripts/build.sh clippy` clean
- (if applicable) `scripts/build.sh gate` clean

**Follow-ups spawned**
- WEFT-NN: <one-line title>
- WEFT-NN: <one-line title>
- (or: "none — fully scoped here")

**Source-of-truth doc updated**
- .planning/reviews/0.7.0-release-gate/NN-*.md: row removed / annotated
- .planning/sparc/.../tracker.md: line updated
- docs/handoff.md: <if relevant>
```

## Defer-comment shape (when bumping cycle, NOT closing)

```markdown
**Deferred to <new-cycle> on <YYYY-MM-DD>**

**Reason**
<one of: blocked-by-upstream | scope-cut | superseded-by-WEFT-NN |
research-not-yet-promoted | dependency-not-built>

**Detail**
<one paragraph: what specifically blocked this, what would unblock it,
when to re-evaluate>

**Re-eval signal**
<concrete trigger: "ruvllm-wasm releases >2.0.1 with HNSW cap lifted",
"agent-core-v1.1 cost circuit-breaker lands", "0.8.x cycle opens", etc.>
```
