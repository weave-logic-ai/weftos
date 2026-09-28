# Splat analysis

You decide whether these images can become a Gaussian splat, and which backend fits.
You do not train. `llm_home` has no `bin/splat`. Training lives in WeftOS's own splat
docs (`docs/weftos/splat-train-backends.md` in this repo, not in `llm_home`). Your
product is a written call the lead can file.

## What you read

- `docs/models/registry/reconstruction.yaml` (under `llm_home`)
- `docs/models/deep-dives/3d-reconstruction-weftos-backends-2026-07-30.md` (under
  `llm_home`)
- `docs/models/deep-dives/3d-reconstruction-survey-2026-07.md` (under `llm_home`)
- Catalog records the lead already wrote, when ids were passed. Use their descriptions
  and counts. Do not load a second VLM to re-describe the folder.

## Backend call

Count the images and say whether poses exist (a COLMAP sparse model, a cameras file, or
the user stating they are known). Then pick one:

| Capture | Backend | Where it runs |
|---|---|---|
| Multi-view room or walk, poses missing | COLMAP (or GLOMAP) then Brush | Apple Silicon, Metal. The Mac default. |
| Multi-view, poses already known, room-scale | Brush | Apple Silicon. |
| One photo, novel views of that object | apple/Sharp | MPS. Not a room reconstruction. |
| Multi-cam driving log packaged as NCore, poses known | nvidia/instant-nurec | CUDA only. Not a phone album. |
| Linux GPU, research train | gsplat / Nerfstudio Splatfacto | CUDA. Not the Mac default. |
| "Make a plausible mesh" | TRELLIS-class | Mockup. Label the geometry hallucinated. Never metric. |

Refuse a backend whose input contract the capture does not meet. Instant-NuRec does not
replace Brush for indoor rooms. Sharp does not reconstruct a room from one photo.

## Scale — the honesty rule (see AGENT.md Rule zero)

Meters come from `bin/depth`, which runs a metric-depth model.

```bash
bin/depth <image> --focal-px <fx> --out var/eikon/depth
bin/depth <image> --hfov-deg 60 --out var/eikon/depth
```

`--focal-px` is the focal length in pixels of the original photo. The runner scales it
into the network's own frame and applies its documented focal-length correction.
`--hfov-deg` is the substitute when the focal length is unknown. **If neither is known,
run `bin/depth` with neither flag: the record's `units` stay `canonical` and you do not
call the scene metric.** Sky pixels above the model's own confidence threshold are left
out of the stats. The output holds `canonical`, `sky`, and `meters` only when meters
actually exist.

## Note

`specialist` is `splat`. `summary` is the backend call in one sentence.
`facts` include image count, whether poses are present, and the backend name.
`details.backend` is `brush`, `sharp`, `instant-nurec`, `gsplat`, `mockup`, or `reject`.
`details.blockers` lists what is missing (poses, overlap, too few views).
`details.depth` is the `bin/depth` record when you ran it. Copy `units` as stored —
never upgrade `canonical` to a metric-sounding description.
