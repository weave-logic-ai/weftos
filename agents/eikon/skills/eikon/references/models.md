# Eikon models

Source of truth in code: `eikon/roster.py` under `llm_home`. Registry cards:
`docs/models/registry/vlm.yaml`, `docs/models/registry/image-embed.yaml`. This page is
the routing the agent follows, ported from the lab's own model reference. Treat any
specific RAM/machine figures below as the lab's measured numbers, not a portability
guarantee — re-check `bin/eikon roster` and the lab's own docs on a different machine.

## The VLM (pick one)

| Profile | RAM (lab-measured) | Use |
|---|---|---|
| `fast` (default) | ~12 GB | Describe, categorize, classify, label, tag, identify, and a second reading of any text. |
| `quality` | ~41 GB | Hard pages, dense documents, when the `fast` answer is thin. Not beside the daily coder. |
| `moe` | ~38 GB | Long batches. |

Server: `bin/eikon warm --profile <name>` starts an OpenAI-compatible VLM server on the
port declared in this package's `weftos-package.yaml` (`:8093` by lab convention).
In-process fallback runs the same model one image after another, released before the
next heavy stage.

The VLM prompt asks for one JSON object: `description`, `scene`, `categories`, `tags`,
`labels`, `objects`, `text`, `facts`. That is the whole delegation — there is no second
model for "tag" versus "describe".

## Always on, no weights

Apple Vision, built on first use:

| Request | Filed as |
|---|---|
| Accurate text recognition | `native.ocr`, and the leading `kb.text` |
| Image classification | `native.labels`. Confidence ≥ 0.08 joins `kb.categories` |
| Barcode detection | `kb.barcodes` and a fact per code |
| Face rectangle detection | `kb.face_count` |
| Feature-print request | archived print for `kb search --image` |

## Only when the flag asks

| Flag | Purpose | RAM (lab-measured) | Delegation |
|---|---|---|---|
| `--labels a,b,c` | SigLIP-2 label scoring | ~2 GB | Sigmoid score of each label against the image. Scores ≥ 0.15 join `kb.categories`. The same labels are also written into the VLM prompt. |
| `--embed` | SigLIP-2 embedding | ~2 GB | L2-normalized image vector stored on the record. |
| `--point "phrase"` | Molmo pointing | ~7 GB | Runs after the main VLM (and after SigLIP) so two VLMs are never resident together in-process. |

`bin/eikon pull --profile fast` and `bin/eikon pull --specialist siglip|molmo|sam`
download through the lab's own pull wrapper (Xet disabled on purpose — see
`USING_LOCAL_MODELS.md`).

## In the registry, not in this fan-out (held models)

| Model | Why it stays out |
|---|---|
| Grounding-DINO | Apache fallback if the SAM license blocks a job. No runner, and none gets built. |
| WD-tagger-EVA02 | Danbooru tags. Photographs and documents use the VLM tags instead. |
| YOLO11 | COCO boxes, AGPL-3.0 licensed. Not loaded. |
| ColQwen2.5 | Visual retrieval for document-page crops. Named as the upgrade to reach for when OCR plus `kb search` misses pages — not run by default. |
| Florence-2-large | Caption and detect. The VLM's JSON pass already covers both. |

## SAM 3.1 and depth (specialist-only — never called from the lead)

`bin/sam <image> --prompt "<thing>"` loads the SAM 3.1 weights once and prints JSON.
Several `--prompt` flags share one vision pass. Grounding DINO remains the Apache catalog
fallback and has no runner.

`bin/depth <image> --hfov-deg <deg>` (or `--focal-px <fx>`) runs a metric-depth model.
**Meters need `--focal-px` or `--hfov-deg`. Without one of those the map stays canonical,
not metric** — this honesty rule is load-bearing for anything downstream that treats a
depth map as a real-world measurement (see the `eikon-specialist` splat skill).

Both binaries are owned by the `eikon-specialist` agent's skill references
(`segment`, `splat`), not by this agent — the lead dispatches, it does not run them.

## Memory

`fast` can sit beside lighter concurrent work. `quality` and `moe` are the heavy brain
for that hour — check the project's monitor command, and never co-reside a heavy VLM
profile with the machine's own daily coder.
