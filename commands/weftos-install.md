---
description: One-shot bring-up — boot kernel, verify mesh, rebuild graph, calibrate ECC
allowed-tools:
  - Bash
---

Run the full WeftOS activation sequence and report each step's outcome:

1. `weftos init` — ensures the current project has a `.weftos/` runtime directory.
2. `weaver kernel boot` — start the daemon. Poll `weaver kernel status` for up to 8 seconds to confirm.
3. `weaver kernel services` — verify required services (mesh, chain, ecc, graphify) are running. Flag any in `failed` state.
4. `weaver graphify rebuild` — populate the knowledge graph from project files.
5. `weaver ecc status` — confirm the cognitive substrate is calibrated; if not, ask the user whether to run `weaver ecc calibrate` (heavy).

Stop on the first hard failure and surface the failing step's error verbatim. After all five succeed, print a one-line summary: kernel PID, mesh listener address, graph node count, ECC codebook size.
