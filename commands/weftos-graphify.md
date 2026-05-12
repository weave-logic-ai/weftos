---
description: Rebuild or query the WeftOS knowledge graph (graphify)
allowed-tools:
  - Bash
  - Read
argument-hint: "<rebuild|query|export> [query-string | --format <html|obsidian|json>]"
---

Operate on the project's knowledge graph via `weaver graphify`.

Dispatch on the first token of `$ARGUMENTS`:

- `rebuild` — run `weaver graphify rebuild`. Stream progress. If it panics on em-dash unicode (known issue at v0.6.2; check version with `weaver version`), warn the user and offer to run a `sed -i 's/—/--/g'` sweep over `.md` files before retrying.
- `query <text>` — run `weaver graphify query "$text"`. Render top hits with file paths and snippet excerpts.
- `export --format <fmt>` — run `weaver graphify export --format <fmt>`. Note that html/obsidian exporters had a "Edge missing source_file" schema bug in v0.6.2 — if export fails with that error, suggest `--format json` as a workaround.

If `$ARGUMENTS` is empty, run `weaver graphify --help` and show the user what's available.

**Pre-check:** confirm `weaver kernel status` shows the kernel running. Graphify queries require an active kernel.
