# Pointing

You point at the phrase in the ask. The lead already described the image. You load
Molmo and nothing else.

## Run

Pull the phrase out of the ask (the thing after "point at" or "where is the").

```bash
bin/eikon analyze <paths> --no-native --no-vlm --point "<phrase>"
```

That loads the pointing specialist model and skips the catalog VLM. If a quality or fast
server is resident on the VLM port, `bin/eikon stop` is not required for an in-process
pointing-model load of this size — but do not start a second full VLM alongside it.

## Note

`specialist` is `ground`. `summary` is where the phrase is, or that it is absent.
`facts` repeat the raw point string.
`details.phrase` is the phrase. `details.raw` is the model reply from the record's
`point` field.
