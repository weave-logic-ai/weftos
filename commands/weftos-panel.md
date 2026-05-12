---
description: Open the WeftOS web dashboard in Claude Code's Preview pane
allowed-tools:
  - Bash
  - mcp__Claude_Preview__preview_start
  - mcp__Claude_Preview__preview_navigate
  - mcp__weftos__weft_open_panel
argument-hint: "[--port <port>]"
---

Render the WeftOS web dashboard inside Claude Code's Preview pane. The full sequence:

1. **Ensure the dashboard process is running.** Check whether `weft ui` is already serving:
   - Run `ss -tln 2>/dev/null | grep 18789` (or the override port from `$ARGUMENTS`).
   - If nothing is bound, launch in the background: `weft ui --no-open &` (or `weft ui --no-open --port <port> &` if a port was supplied). Wait up to 3 seconds for the listener to come up.

2. **Resolve the URL.** Call the `mcp__weftos__weft_open_panel` MCP tool — it returns `{ url, preview_hint, ui_command }` based on the kernel's configured `gateway.host` + `gateway.api_port`.

3. **Drive the Preview pane.** Use `mcp__Claude_Preview__preview_start` (or `preview_navigate` if a preview is already open) with the `url` from step 2.

4. **Report.** Tell the user the dashboard is up, give them the URL as a clickable fallback, and remind them they can close the preview at any time.

If the user passed `--port <port>` in `$ARGUMENTS`, pass it through to `weft ui` in step 1 — but note the MCP tool still reads the *configured* port. If the override doesn't match the config, the URL from the tool will be wrong. In that case, build the URL manually as `http://127.0.0.1:<port>/` and use it directly.
