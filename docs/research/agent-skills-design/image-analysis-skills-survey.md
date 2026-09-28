# Image-analysis skills survey (awesomeskill.ai, image-analysis tag)

Survey date: 2026-09-28. Source: https://awesomeskill.ai/tag/image-analysis (10 listed skills,
7 distinct SKILL.md sources after dedup). Repos shallow-cloned read-only to
`~/dev/_skill-survey/`; nothing installed, no paid API called.

## Summary

| Skill | Repo (canonical source) | License (repo / skill) | What it does | Verdict | Destination |
|---|---|---|---|---|---|
| geolocate-from-pixels | useosint/osint-skills | MIT / MIT | Method + confidence-grading for photo geolocation/chronolocation from visual clues | **ADAPT** | (a) Urth ingest pipeline, honest-geometry-wrapped |
| ctf-osint | ljagiello/ctf-skills | MIT / MIT | Quick-reference OSINT techniques for CTF flags (recon, geolocation, stego, dorking) | **PATTERN** | (d) Agent Directory, CTF/security team only |
| omero-integration | K-Dense-AI/scientific-agent-skills (canonical; alias claude-scientific-skills) | MIT / MIT | Scoped OMERO microscopy server access: connect, inventory, ROIs, tables, scripts | **ADOPT** | (c) Episteme |
| pathml | K-Dense-AI/scientific-agent-skills (canonical) | MIT / MIT | Local computational-pathology pipelines: WSI tiling, QC, spatial graphs, bounded inference | **ADOPT** | (c) Episteme |
| pydicom | K-Dense-AI/scientific-agent-skills (canonical) | MIT / MIT | DICOM I/O, pixel/transfer-syntax handling, bounded de-identification review | **ADOPT** | (c) Episteme |
| omero-integration (davila7 copy) | davila7/claude-code-templates | MIT / none declared | Stripped subset of the K-Dense skill | **SKIP** (use canonical) | n/a |
| pathml (davila7 copy) | davila7/claude-code-templates | MIT / none declared | Stripped subset, safety boundary removed | **SKIP** (use canonical) | n/a |
| qwencloud-vision | qwencloud/qwencloud-ai | Apache-2.0 / none declared | Qwen VL/QVQ image+video understanding via QwenCloud API | **SKIP for adoption / WATCH as pattern** | (d) reference only; prefer local Qwen3-VL |
| fal-vision | nexu-io/open-design → upstream fal-ai-community/skills | Apache-2.0 (open-design) | Catalogue stub; **upstream skill no longer exists** (404 as of 2026-09-28) | **SKIP** | n/a — dead reference |
| processing-computer-vision-tasks | jeremylongshore/claude-code-plugins-plus-skills | MIT / MIT | Auto-generated boilerplate; script only does directory/file-size stats, no CV | **SKIP** | n/a |

## 1. Generic / vision-API skills

### processing-computer-vision-tasks (jeremylongshore/claude-code-plugins-plus-skills)
Identical copies live at `plugins/ai-ml/computer-vision-processor/skills/...` and
`skills/.curated/processing-computer-vision-tasks/`. The SKILL.md frontmatter and prose promise
object detection/classification/segmentation via a `/process-vision` command "provided by the
computer-vision-processor plugin," but the bundled `scripts/image_analyzer.py` is a generic
**file/directory statistics tool** (counts files by extension, flags large/empty files) — it
contains no image, tensor, or model code at all. `references/README.md` and `scripts/README.md`
are one-line stubs ("Bundled resources for..."), and the file header says "Generated:
2025-12-10," consistent with a templated skill-farm output never filled in. No progressive
disclosure of real substance, no guardrails, no external API. MIT, low risk only because it does
nothing. **Verdict: SKIP** — not a real computer-vision skill.

### fal-vision (nexu-io/open-design)
The open-design copy (`skills/fal-vision/SKILL.md`) is explicitly a **catalogue stub**: it has no
`scripts/` or `references/`, and its "How to use" section instructs the agent to fetch the real
implementation from `https://github.com/fal-ai-community/skills` at install/run time (`npx skills
add ... --skill fal-vision`). Checking that upstream repo directly (`gh api
repos/fal-ai-community/skills/contents/skills/claude.ai/fal-vision` → 404; raw.githubusercontent
main branch → 404) shows the skill has been **removed from the upstream repo's default branch**
as of 2026-09-28, even though third-party mirrors (officialskills.sh, skills.sh, atskills.one)
still describe it from a stale cache. Two findings: (1) the "skill re-fetches its own code from a
third-party GitHub URL at use time" pattern is a supply-chain concern independent of this
specific skill — it means what actually runs is whatever's on `main` the moment the agent
installs it, not what a reviewer read; (2) right now that fetch would simply fail. **Verdict:
SKIP** — dead reference; if fal.ai vision access is wanted later, treat `npx skills add` catalogue
stubs as untrusted pointers and vet the resolved content each time, never pre-approve the pattern.

### qwencloud-vision (qwencloud/qwencloud-ai)
By contrast this is a genuinely well-built, vendor-maintained skill: real Python scripts
(`analyze.py`, `reason.py`, `ocr.py`, shared `vision_lib.py`), progressive disclosure into
`references/*.md`, explicit credential hygiene ("NEVER output any API key... non-plaintext status
checks only"), a documented model-selection policy that refuses to silently swap a user-specified
model, and clear file-size/upload-path rules. Two things to flag: it fetches its "current model
catalog" from an external CDN (`alioth-intl.alicdn.com`) at run time with a bundled local
fallback, and it runs an "update check" that nags the agent to auto-install a sibling skill via
`npx skills add ...` on stderr signals — a soft telemetry/growth-loop pattern, not malicious but
worth knowing before dropping this in front of end users. Apache-2.0, no license declared on the
skill itself (repo-level Apache-2.0 covers it). Requires `QWENCLOUD_API_KEY` /
`DASHSCOPE_API_KEY` (paid, Alibaba Cloud). **Verdict: SKIP for adoption, WATCH as an
engineering-quality reference** — this is the bar other API-vendor skills should be held to for
credential hygiene and model-pinning discipline, but WeftOS should not add an Alibaba Cloud vision
dependency when a license-clean local path exists (see §3).

## 2. OSINT / geolocation skills

### geolocate-from-pixels (useosint/osint-skills)
The strongest skill in the set. Pure knowledge artifact (no scripts, no code execution) that
teaches a ranked-clue methodology (plates, phone-number formats, road markings, utility-pole
style, signage typeface, vegetation, terrain — in that eliminating order), a worked example, and
— most importantly for WeftOS — an explicit **confidence-grading scheme**: Confirmed (≥3
independent non-transient features + reproducible bearing) / Probable / Region-only / Unconfirmed
/ Excluded, with "always report a radius with a coordinate. A bare six-decimal coordinate implies
sub-metre certainty you do not have." That is exactly the honest-geometry discipline
`docs/research/skill-3d/README.md` requires ("a geolocation guess is an estimate with
uncertainty, never ground truth"). It also carries a genuine ethics section (`ETHICS.md` at repo
root plus a per-skill "Legal and ethical notes" block): passive/lawful use only, no doxxing or
stalking, publish a region not a doorstep coordinate for private individuals, added caution in
conflict-imagery work. MIT. **Verdict: ADAPT** — port the clue taxonomy and confidence grading
into Urth's imagery-placement pipeline as a *pattern* (typed `EstimatedLocation{radius_m,
confidence, evidence[]}` distinct from metric BVH data), not a literal skill install, since Urth
needs this as Rust logic behind a typed API rather than a markdown-guided agent workflow. Keep the
ethics gate.

### ctf-osint (ljagiello/ctf-skills)
Solid, narrowly-scoped MIT skill for CTF competitions: DNS/WHOIS recon, EXIF/metadata,
Google-dorking, username enumeration, and a geolocation section (railroad signs, Street View
panorama matching, MGRS/Plus Codes) that overlaps with `geolocate-from-pixels` but is written for
flag-hunting rather than real-world investigation — no confidence grading, no ethics section in
the SKILL.md itself (the parent repo has `CODE_OF_CONDUCT.md`/`CONTRIBUTING.md` but not a
dedicated ethics gate). `user-invocable: false` in its frontmatter, meaning it's designed as a
sub-skill an orchestrator pivots into from `ctf-forensics`/`ctf-web`/`ctf-malware`, not a
standalone entry point. Good structural pattern (pivot table, tight scope) but the content is
CTF-specific (Shodan, Tor relay fingerprints, Snowflake IDs) and not directly reusable for Urth or
Episteme. **Verdict: PATTERN** — useful as a structural template for any future WeftOS
CTF/security-research skill; only worth shipping into the Agent Directory if/when there's an
actual CTF or pentest-recon workstream, gated to that team.

## 3. Scientific imaging skills (OMERO, PathML, pydicom) — dedup

`K-Dense-AI/claude-scientific-skills` and `K-Dense-AI/scientific-agent-skills` are **the same
repository** — the former is a GitHub rename/redirect the README itself documents ("Claude
Scientific Skills is now Scientific Agent Skills. Same skills, broader compatibility"). A byte-for-
byte diff of the cloned `claude-scientific-skills` tag against the locally-reviewed
`~/dev/scientific-agent-skills` clone (under review for Episteme, MIT, K-Dense Inc. 2025, 166
skills, arXiv 2609.00065) showed **zero differences** for omero-integration, pathml, or pydicom.
That local clone is the canonical, current copy.

`davila7/claude-code-templates` carries its own copies of `omero-integration` and `pathml` (no
`pydicom`) under `cli-tool/components/skills/scientific/`. Diffing them against canonical:

- **omero-integration**: davila7's SKILL.md is 245 lines vs canonical 239 — reformatted, not
  expanded — but it is **missing the entire `scripts/` directory** (canonical ships
  `omero_common.py`, `inventory.py`, `export_image_metadata.py`, `plan_transfer.py`,
  `validate_config.py`), missing `references/sources.md`, and missing the frontmatter that pins
  `omero-py==5.22.1`/IcePy, declares `license: MIT`, and enumerates the six `OMERO_*` env vars
  with a "never load .env" rule.
- **pathml**: davila7's copy is 160 lines vs canonical's 239, and — the material finding — **it
  drops the entire safety boundary**. Canonical pathml opens with "PathML is... not a validated
  medical device, diagnostic system, clinical decision support tool" plus a five-point PHI/consent
  checklist (de-identify pixels and metadata, pseudonymous IDs, encrypted storage, patient-level
  splits). None of that language appears in davila7's version, and it ships **no `scripts/`
  directory at all** (canonical has `slide_manifest.py`, `image_qc.py`, `plan_inference.py`,
  `validate_spatial_schema.py`, `plan_pipeline.py`).

Both repos are MIT at the top level; davila7's individual skill files declare no `license:` field
of their own (relying on the repo LICENSE). The templates copy is not malicious, just an older,
trimmed snapshot re-hosted in a components catalog without the guardrails K-Dense has since added.
**Verdict for davila7's copies: SKIP — use the canonical K-Dense-AI/scientific-agent-skills
skills directly; do not let the templates-catalog version reach Episteme, since it silently drops
the PHI/consent boundary.**

All three canonical skills (omero-integration, pathml, pydicom) are high quality: SKILL.md +
`references/*.md` + `scripts/*.py` progressive disclosure, pinned dependency versions with
verification dates, explicit safety/scope boundaries, least-privilege credential handling
(pydicom's de-identification key rotation/revocation section is the strongest example), and no
"call a cloud API" surface at all — everything is local, dependency-free CLI helpers over
open-source Python libraries (omero-py, PathML, pydicom). **Verdict: ADOPT as-is into Episteme.**

## 4. Fit against the four WeftOS workstreams

**(a) Skill-3D spatial reasoning / Urth planetary twin** — `geolocate-from-pixels` is the standout:
ADAPT its clue-ranking method and confidence-grading types into the Urth ingest path for
user-submitted or scraped imagery. It must never be wired to emit a bare lat/long; every output
needs `{region|point, radius_m, confidence, evidence[]}`, mirroring the honest-geometry doctrine
already enforced for DA3/Pi3/GroundingDINO outputs in `docs/research/skill-3d/README.md`. Treat a
geolocation guess exactly like a "not metric" expert output — no different from a Skill-3D expert
that outputs relative depth: a hypothesis with stated uncertainty, not ground truth. Also inherit
its privacy rule (region, not doorstep, for private individuals) as a hard constraint on any Urth
feature that ingests third-party photos. `ctf-osint`'s geolocation section adds nothing beyond
what `geolocate-from-pixels` already covers more rigorously — skip it for Urth.

**(b) MentraOS glasses egocentric image/video** — none of the ten skills targets egocentric/
head-mounted capture specifically. `qwencloud-vision`'s OCR and multi-image comparison scripts are
architecturally close to what a glasses assistant needs (image/video in, structured JSON out,
explicit large-file/upload handling) and are worth reading as an *engineering pattern* for the
MentraOS tool layer's request/response shape and credential hygiene, but the dependency itself
should not ship — see §3 below on the local alternative. No verdict change: **SKIP for direct
use, PATTERN for API-wrapper shape only.**

**(c) Episteme (STEM/medical/bio imaging pack)** — `omero-integration`, `pathml`, and `pydicom`
from `K-Dense-AI/scientific-agent-skills` are a direct **ADOPT**: they are already the exact
skills Episteme is reviewing that repo for, MIT-licensed, locally-scoped, and built with real PHI/
consent guardrails. Bring them in from `~/dev/scientific-agent-skills` (or upstream, they're
identical), not from davila7's stripped copies. No changes needed beyond WeftOS's normal
skill-import review.

**(d) Agent Directory's shared image-analysis skills for the base team** — of the ten, only
`geolocate-from-pixels` (after the ADAPT above) and the K-Dense scientific trio are worth exposing
broadly, and the scientific trio belongs in Episteme's namespace, not the general base team, since
most agents have no legitimate reason to touch OMERO/pathology data. `ctf-osint` is PATTERN-only
and should stay gated to a CTF/security team if one exists. The two dead/hollow entries
(`processing-computer-vision-tasks`, `fal-vision`) should not be listed in the directory at all —
recommend flagging them back if awesomeskill.ai's listing is treated as a sourcing feed, since one
is non-functional boilerplate and the other points at a repo path that no longer exists.

## 5. Vision-API skills vs. a local, license-clean path

Both `fal-vision` (dead upstream) and `qwencloud-vision` (live but Alibaba-Cloud-hosted, paid,
external-CDN-dependent) are cloud vision APIs behind a skill wrapper. Per
`docs/research/skill-3d/papers/experts-and-frontier-models.md`, WeftOS already has a documented,
license-clean local alternative for the same image-understanding surface:

- **Qwen3-VL-4B/8B** (Apache-2.0, open weights) for general image/video Q&A, OCR, and
  captioning — the same model family qwencloud-vision calls over the network, runnable locally or
  on WeftOS-controlled remote GPU instead.
- **Grounding DINO** (Apache-2.0) for open-vocabulary 2D detection — "the most Rust-ready expert
  in the set," with an ONNX path today and a `candle` native port plausible.
- **Depth Anything 3, metric-large checkpoint only** (Apache-2.0) for conditional metric depth —
  covers what neither fal-vision nor qwencloud-vision attempt at all (per-pixel distance).

This trio is the license-clean, no-per-call-cost, no-external-dependency substitute for both
vision-API skills' core capability (segment/detect/OCR/describe/VQA), and it is already the
adopted stack for Urth/glasses work. **Recommendation: do not add fal-vision or qwencloud-vision
as dependencies anywhere in WeftOS.** If a hosted-API vision path is ever wanted for burst
capacity, qwencloud-vision's credential-hygiene and model-pinning code is the reference to copy
the *pattern* from, not the dependency to install.

## Sources

- Repo clones (read-only, shallow, `~/dev/_skill-survey/`): claude-code-plugins-plus-skills,
  claude-code-templates, claude-scientific-skills, ctf-skills, open-design, qwencloud-ai,
  osint-skills, fal-ai-community-skills.
- Canonical scientific skills reviewed in place at `~/dev/scientific-agent-skills`.
- `docs/research/skill-3d/README.md`, `docs/research/skill-3d/papers/experts-and-frontier-models.md`.
- https://awesomeskill.ai/tag/image-analysis (listing snapshot, 2026-09-28).
