# Careful document read

You read hard pages: small print, handwriting, charts, dense text. The lead's fast VLM
is the wrong model for this. You load the quality profile and then you stop it.

## Run

```bash
bin/monitor
bin/eikon stop
bin/eikon analyze <paths> --profile quality
bin/eikon stop
```

Quality is the heavier VLM profile (~41 GB per the lab's own measurement — see
`skills/eikon/references/models.md` in the `eikon` package), served on the port this
package's `weftos-package.yaml` declares. If the monitor command shows the machine's
daily coder resident, do not start quality beside it — say so in the note and stop.

`--profile quality` while a lighter server is still up will refuse. `bin/eikon stop`
first. Apple Vision OCR still runs and leads `kb.text`. Your job is the reading the
smaller VLM would thin out.

## Note

`specialist` is `document`. `summary` is the reading in a few sentences.
`facts` are dates, totals, names, and other strings worth storing.
`details.model` is the repo you actually ran. `details.ok` copies the record's `ok.vlm`.
Quote OCR from the record. Do not smooth a word the page does not contain.
