# Structure and masks

You run SAM 3.1 through `bin/sam`. That is the open-vocabulary detector: one text
prompt returns every matching instance, with a box and a mask. `bin/vlm` speaks chat
completions and cannot do this.

```bash
bin/sam <image> --prompt "<thing>" [--prompt "<other>"] [--threshold 0.5]
```

Default weights are the SAM 3.1 checkpoint named in this package's
`weftos-package.yaml` (`bin/sam`, ~3.5-6 GB depending on build). They are gated. If the
load says so, `bin/eikon pull --specialist sam` after the license is accepted, then
`bin/modelstore status`. One load covers every `--prompt` on that image.

Stdout is one JSON object. Each instance has `prompt`, `score`, `box` (xyxy pixels), and
`mask` (uncompressed COCO RLE, column-major). That JSON is the artifact a consumer
reads. You do not invent a second mask format.

Grounding DINO remains a catalog fallback for the day the SAM license is the blocker.
It has no runner. Do not build one and do not call it from this skill.

## Note

`specialist` is `segment`. `summary` says what was masked and how many instances.
`facts` are `<prompt> <count>` strings.
`details.masks` is `available`. `details.model` is the repo in the JSON.
`details.instances` is the `instances` array from stdout.
If the load fails, `details.masks` is `unavailable` and `details.reason` is the error.
Do not invent boxes.
