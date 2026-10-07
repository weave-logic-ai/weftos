# Sensor contracts

Shared formats for sensor data, defined by [ADR-111](../../docs/adr/adr-111-sensor-evidence-and-readings-contracts.md).

| Path | What it is |
|------|------------|
| `quantities.v1.json` | The readings vocabulary: each quantity name maps to exactly one SenML unit, with a physical range and a subject. Also the declared cues and covariates, each with a test vector |
| `vectors/` | Golden test lines. `evidence/` holds `spatial.evidence.v1` lines for all 13 record types, `senml/` holds readings packs. `valid/` must be accepted, `invalid/` must be rejected. `manifest.json` lists every file, what it expects, and for evidence rejections the reason class |

Each vector file holds one line. In `manifest.json`, `class` is one of `version`,
`unknown_type`, `shape`, `range`, `text` or `size`.

## Versioning

- The evidence format is `spatial.evidence.v1`. New optional fields and new record types stay
  in v1. Unknown types are counted, not fatal. Anything that changes the meaning of a field
  needs v2.
- The vocabulary has a semantic `version`. Adding a quantity, cue or covariate is a minor
  version. Changing or removing a unit or range needs a new file (`quantities.v2.json`).
- The vectors have their own `vectors_version` in the manifest.

## Using it

- **Producers** (cogs, firmware readers): emit lines that pass every `valid/` vector's rules
  and fail the `invalid/` ones in your own tests. Take unit strings and ranges from
  `quantities.v1.json`, not from memory.
- **Consumers** (RuView and others): load the vocabulary, run the vectors against your
  validator, and compare the rejection class. RuView's conformance cross-check consumes these
  files directly.
- **Reference validator for evidence:** `weftos-spatial-core` in the spatial workspace. Its
  tests run the evidence vectors.

To add or change a contract, change ADR-111 and these files in one commit, then update
consumers in the same change.
