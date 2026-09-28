# Write-time marks

Four marks a lesson-type record carries at the moment it's filed. All four are set once,
at write time — none is inferred later from context, because inference-after-the-fact is
exactly how a memory store accumulates unverifiable claims.

## 1. Destinations (`packages`, `always`)

Which store(s), namespaces, or lanes this memory should surface into. Many-to-many — a
lesson can belong to several. `always: true` marks a memory that should load
unconditionally regardless of namespace filtering (use sparingly; this is the "everyone
needs to see this" escape hatch, not a default).

```yaml
packages: [build-lane, reviewer]
always: false
```

## 2. Provenance (`provenance`)

Where the claim actually came from. A claim with no traceable origin is folklore by the
time it's read back a second time.

```yaml
provenance:
  source: "incident report 2026-09-24, dashboard.example/incidents/118"
  origin: "steward agent, post-incident write-up"
  verified_by: "human, 2026-09-25"   # optional but required for carryable: true
  captured_at: "2026-09-24T21:10:00Z"
  supersedes: null                    # or the slug of a prior memory this replaces
```

## 3. Carryability (`carryable`)

Whether this lesson's *shape* (with identifying specifics stripped) is safe to hand
forward to a different project or client engagement. **Never inferred by Liber.** A
person sets it directly, or a person-authored rule sets it deterministically (e.g. "any
memory tagged `internal-tooling` and zero confidentiality tags is carryable"). `true`
without a `verified_by` in provenance is refused.

```yaml
carryable: true
```

## 4. Durability (`durability: sugar | lignin`)

A metaphor borrowed from actual bark tissue: `sugar` is a fact that will go stale — a
hash, an id, a count, a specific number. `lignin` is structural — a defect class, a
recurring failure pattern, a design lesson that will still be true in a year. The
distinction matters for retention/pruning policy: `sugar`-durability memories are safe to
expire once their referent changes; `lignin`-durability memories are the ones worth
keeping even after the specific incident is forgotten.

```yaml
durability: lignin
```
