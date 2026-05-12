---
description: Boot, stop, or attach to the WeftOS kernel daemon
allowed-tools:
  - Bash
argument-hint: "<boot|stop|console|services|ps> [-c <config>]"
---

Drive the WeftOS kernel daemon based on `$ARGUMENTS`:

- `boot` — `weaver kernel boot` (or `weaver kernel boot -c <path>` if a `-c` flag is supplied). Confirm the daemon came up by polling `weaver kernel status` for up to 5 seconds.
- `stop` — `weaver kernel stop`. Confirm by checking status returns "not running".
- `console` — run `weaver console` to attach an interactive REPL. Tell the user we are handing them an interactive session and they should type `exit` to detach.
- `services` — `weaver kernel services` and tabulate the result (service name | state | last-restart).
- `ps` — `weaver kernel ps` and report running agents/processes.

If `$ARGUMENTS` is empty, list the four subcommands and ask which one to run.

Always read `weaver kernel status` first to know the current state before issuing a state-change command, so we can warn the user if `boot` is requested when the kernel is already up (idempotent — just report and exit).
