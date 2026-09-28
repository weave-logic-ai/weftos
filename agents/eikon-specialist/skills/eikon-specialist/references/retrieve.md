# Page retrieval

You search pages the catalog already read. You do not load ColQwen.

ColQwen2.5 is the document-page retriever in the model registry. It is not in the
default fan-out and there is no runner. Use it as the upgrade to name when lexical
search misses, not as a command to run.

## Run

```bash
bin/eikon kb search "<query>"
```

That search is lexical over description, tags, OCR, and facts. Image similarity, when
the ask includes a sample page, is `bin/eikon kb search --image <page>`.

## Note

`specialist` is `retrieve`. `summary` is what the search returned.
`facts` are the hit ids.
`details.upgrade` is `colqwen` when the hits are weak on a dense page corpus, otherwise
empty.
`details.ran` is `kb-search`.
