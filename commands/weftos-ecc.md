---
description: Query or calibrate the WeftOS ECC cognitive substrate
allowed-tools:
  - Bash
argument-hint: "<status|search|calibrate> [query]"
---

Operate the ECC (Error-Correcting Cognition) substrate via `weaver ecc`.

- `status` — `weaver ecc status`. Report the current substrate calibration, codebook stats, and any drift warnings.
- `search <query>` — `weaver ecc search "<query>"`. Render the ranked semantic hits with their source bindings (file + position if available).
- `calibrate` — `weaver ecc calibrate`. This may take a while — stream progress and warn the user it's a heavy operation. After completion, run `weaver ecc status` again to confirm calibration converged.

If `$ARGUMENTS` is empty, list the three subcommands and prompt for which to run.

**Pre-check:** confirm `weaver kernel status` shows the kernel running. ECC requires an active kernel.
