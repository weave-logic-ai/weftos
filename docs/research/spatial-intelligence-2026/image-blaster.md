# image-blaster — one image to a splat world, object meshes and SFX (Claude skills)

**Status:** Initial research capture (not an ADR)  
**Date:** 2026-10-04  
**Parent:** [`README.md`](./README.md) (S10)  
**Companions:** [`world-labs-marble-atlas.md`](./world-labs-marble-atlas.md) (S1; image-blaster is a client of
the Marble World API), ADR-078 (dual output), ADR-079 §2 (generative fill is cosmetic),
[`splat-train-backends.md`](../../weftos/splat-train-backends.md) §3.4 (generative = mockup only),
[`../skill-3d/README.md`](../skill-3d/README.md), [`../agent-skills-design/`](../agent-skills-design/)

**Repo:** <https://github.com/neilsonnn/image-blaster> · MIT · TypeScript (bun) · created 2026-04-21,
last push 2026-05-15 (commit `4acb43ba`), about 7.3k stars at the time of reading. Read from a shallow
clone of that commit; nothing was run (it needs paid World Labs and FAL keys).

**Honesty rule (parent):** everything image-blaster makes is **generated**. The splat, the collider
and every object mesh are a model's guess at what the photo implies. In WeftOS terms all of it is
`generative_cosmetic` (S1 ticket T2): usable for mockups, previews and synthetic test scenes, never
as metric geometry or occupancy (ADR-079 §2, `splat-train-backends.md` policy).

---

## 1. What it is

A Claude Code project, not a library. You put a photo in `input/`, run `claude` in the repo and say
"blast it". Eight skills and six background agents in `.claude/` drive five hosted models, and a
React viewer shows the result. The README claims image to meshed environment in under 5 minutes.

| Step | Skill / agent | Model (endpoint) | Output |
|---|---|---|---|
| 1. Analyse the photo | `image-blast-uncover` (skill, in the main session) | Claude's own vision | `image.json` (literal scene description, lighting, ambient sound) and object candidates with per-image evidence |
| 2. User confirms objects | (stop point) | — | one `output/<object>/object.json` per approved object |
| 3. Clean plate | `image-blast-plate` → `image-blast-image-edit` | `fal-ai/nano-banana-2/edit` (default) or gpt-image-2 | the photo with confirmed objects removed (`N-<slug>-plate.png`) |
| 4. Static world | `image-blast-world` | World Labs `marble-1.1` (`api.worldlabs.ai/marble/v1/worlds:generate`) | splat `.spz` (100k / 150k / 500k / full_res), collider `.glb`, panorama, thumbnail, `metric_scale_factor`, `ground_plane_offset` |
| 5. One mesh per object | `image-blast-3d` (one agent per object) | image edit to isolate the object, then `fal-ai/hunyuan-3d/v3.1/pro/image-to-3d` (default) or `fal-ai/meshy/v6/image-to-3d` | `.glb` / `.obj`, 50k faces by default, PBR on |
| 6. Sound | `image-blast-sfx` | `fal-ai/elevenlabs/sound-effects/v2` | looping ambience and per-object impact sounds |
| (any) | `image-blast-wildcard` | any FAL endpoint, only after the user confirms it | whatever that endpoint returns |

The viewer (`app/`) is Vite + React Three Fiber + **Spark 2** (`@sparkjsdev/spark`, MIT) for the splat
+ **Rapier** for physics against the collider mesh. A placement editor writes `scene.json`: per object
instance a position, rotation, scale and physics mode (`rigidbody | static | ghost`), plus sun,
shadow catcher and ground-plane settings. Render modes are splat-only, objects-only or combined.

## 2. The two ideas worth keeping

### 2.1 Static scene vs movable objects, decided by a physical test

Step 1 is a decomposition rule we do not have written down anywhere. Object candidates are "single
cleanly segmentable items only". The test is **"could a human lift the item or move it by pushing
it"**. Rugs, floors, walls and built-in fixtures are scene, not objects. Compound assets are banned
("table with chairs", "table including the objects on top").

Then the static environment is generated **from a clean plate with those objects removed**, both as
an edited image and as a "text clean plate": the world prompt is the scene description with every
confirmed object subtracted. Each movable thing becomes its own mesh with its own physics mode.

That is ADR-078's leaf split, arrived at from the game-asset side:

| image-blaster | WeftOS (ADR-078 / ADR-056) |
|---|---|
| Static environment splat | appearance (`splat.sog` / SPZ) |
| Collider `.glb` of the empty room | `WM_SURFACE` / `WM_VOLUME` *candidates* (S1 T3) |
| Liftable/pushable object, one mesh each | `WM_OBJECT` instance with a movable affordance |
| `scene.json` physics `rigidbody / static / ghost` | affordance on the Object leaf (movable / fixed / non-colliding) |
| Per-object `evidence[]` back to source images | Event leaves (observations) supporting the Object |

**Steal:** the lift-or-push test as the default rule for what the structure stage (W1) proposes as a
`WM_OBJECT` versus a surface. It is concrete, cheap to apply, and matches how agents will ask
questions ("what can be moved here?").

### 2.2 Disk-first artifacts with a provenance file beside each one

Every generated file follows one convention (`.claude/rules/project.md`):

```text
N-slug.ext            # N = generation index; 0 is the source, higher = derived
.N-slug-request.json  # hidden: provider, endpoint, model, request (base64 stripped), result, status
```

- Provider URLs live only in the request JSON as provenance and resume data. The viewer loads local
  files only, and a repair tool refills missing files from the recorded responses.
- `object.json` holds **identity, intent and provenance only**, never generated state (no status,
  jobs or file lists). Generated state is whatever indexed files exist beside it.
- Unfinished requests resume from their request JSON; a regeneration is a new index, never an
  overwrite.

This is the same discipline we want for capture and train jobs: intent separate from outcome,
immutable numbered derivations, and the provider response kept as evidence. The request JSON is
effectively an Event leaf on disk. **Steal** the convention for splat-pipeline job folders and for
any WeftOS skill that calls a hosted generator (it fits next to the higgsfield-generate pattern in
`agent-skills-design/`).

Smaller pattern: every generation runs as **one background agent per artifact** (one world, one
object, one SFX), never a batch, and the agent stops and reports if its prompt names more than one.

## 3. Where it fits in WeftOS

| Use | Fit | Seam |
|---|---|---|
| **Urth / splat mockups** ("what could this room become", a site concept from one photo) | B — compose | Import a `worlds/<slug>/` folder as a Graph View foreign source: `source_system=image-blaster/worldlabs`, every leaf `generative_cosmetic`, never promoted (F9) as occupancy. Builds on S1 T2 + T3. |
| **Synthetic scenes for spatial-agent tests** (Skill-3D tool loop) | B — compose, with care | `scene.json` gives exact object poses *in the generated scene*, so questions like "which object is closest to X" have a known answer without a GPU. That truth is the **authored layout**, not the photo: object meshes are generated separately and placed by hand or by the agent, and Marble's scale is approximate (`metric_scale_factor`). Label as synthetic; keep out of the scene-disjoint real eval set (Skill-3D R0.7). |
| **Spark + Rapier viewer reference** | A — pattern | A working MIT example of Spark 2 splat + collider physics + placed GLBs with grab/drop. Reference for S1 T4 (Spark overlay of BVH AABBs in the harness). |
| **Object-candidate prompt** (literal, evidence-linked, no compounds) | A — pattern | Wording for a W1 semantic proposer prompt; output shape maps to `WM_OBJECT` proposals with `vector: None`. |
| **As a capture / reconstruction path** | No | Generative. `splat-train-backends.md` already rules generative image→3D out of the world model; this does not change that. |
| **Vendoring the repo or its skills** | No | It is a workspace for one person's API keys, with no tests around the generation scripts; we take the patterns, not the code. |

## 4. Risks and limits

- **Everything is invented.** One photo cannot show the back of a room; Marble fills it. Object
  meshes are reconstructed from a single isolated crop. Treat geometry, scale and hidden sides as
  fiction (Li's taxonomy note in S1: generative geometry "can look correct while being
  self-intersecting or wrong-scale").
- **Images leave the machine.** The world call sends the photo inline as base64 to World Labs; edits,
  meshes and SFX go to FAL and through FAL to Google (nano-banana), Tencent (Hunyuan3D), Meshy and
  ElevenLabs. Never run it on client or confidential imagery (the oil-rig material, client meeting
  photos, anything under NDA).
- **Paid and closed.** World Labs API credits and FAL credits per run; Marble worlds live behind the
  World API until downloaded. The closed API must never be a WeftOS SpatialBackend (S1 §4).
- **Licences to check before any commercial use:** World Labs output terms; the Hunyuan3D model
  licence as served through FAL (Tencent's Hunyuan community licences carry territory and use
  restrictions; verify the exact terms for v3.1 Pro on FAL); Meshy and ElevenLabs output terms. The
  repo's own MIT licence covers only its code.
- **Fragile by design.** The pipeline is prompt-driven (Claude follows `SKILL.md` contracts); the
  only tests cover the viewer's loader and config, not the generation scripts.
- **Young repo.** About 4 weeks of commits (2026-04-21 to 2026-05-15) and nothing since; popular, but
  treat the API shapes as a snapshot of Marble 1.1.

## 5. Recommendation

**S10 — priority B overall, with two A patterns.**

1. **Apply now (docs only):** add the lift-or-push rule and the "no compound objects" rule to the W1
   structure-stage proposer notes in `docs/weftos/splat-to-world-model.md`, and the indexed
   `N-slug` + hidden request JSON convention to the splat job-folder layout. No code.
2. **Compose later:** an image-blaster world folder as a `generative_cosmetic` Graph View import, once
   S1 T2 (non-metric tag) and T3 (collider-GLB proposer) exist. Optional synthetic-scene fixtures for
   the Skill-3D tool loop, labelled synthetic.
3. **Do not:** vendor the repo, wire its hosted calls into WeftOS, or let any of its geometry reach
   BVH as occupancy.

Possible tickets (acceptance criteria only, **not filed**):

- **T6 — Generative world import.** Given a `worlds/<slug>/` folder, emit appearance (SPZ) plus
  `WM_SURFACE` / `WM_OBJECT` proposals with `generative_cosmetic` provenance and the request JSON as
  Event evidence; a test proves none of them can be promoted as occupied volume. Depends on T2, T3.
- **T7 — Synthetic spatial fixtures.** Turn a `scene.json` into Skill-3D tool-loop fixtures with
  known object positions, marked synthetic and excluded from the real held-out set.

## 6. Sources

- Repository, README, `.claude/skills/*/SKILL.md`, `.claude/agents/*.md`, `.claude/rules/project.md`,
  `.claude/scripts/world/generate-world.mjs`, `.claude/scripts/asset-pipeline/*.mjs`,
  `app/src/types/world.ts`, `app/package.json` — <https://github.com/neilsonnn/image-blaster> at
  `4acb43ba` (2026-05-14).
- World Labs Marble and World API: see S1 sources in [`world-labs-marble-atlas.md`](./world-labs-marble-atlas.md) §7.
- Spark: <https://github.com/sparkjsdev/spark>.
