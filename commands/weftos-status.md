---
description: Show WeftOS kernel, mesh, chain, and ECC substrate status
allowed-tools:
  - Bash
argument-hint: "[--verbose]"
---

Report the current WeftOS status. Run the following in order, capturing stdout, and summarize for the user:

```bash
weftos status
weaver kernel status
weaver cluster status 2>/dev/null || echo "(cluster offline)"
weaver chain status 2>/dev/null || echo "(chain offline)"
weaver ecc status 2>/dev/null || echo "(ecc offline)"
```

Then state:

1. Whether the kernel daemon is running, its PID, and uptime.
2. Whether the mesh listener is bound (typically `0.0.0.0:9470`).
3. Whether ExoChain and ECC subsystems are healthy.
4. Any errors or "(offline)" markers — flag these as actionable.

If `$ARGUMENTS` contains `--verbose`, also run `weaver kernel services` and list every registered service with its state.
