---
description: Inspect the WeftOS mesh — peers, listener state, recent ExoChain events
allowed-tools:
  - Bash
argument-hint: "[peers|listen|chain-tail]"
---

Inspect the mesh substrate. Default action (no `$ARGUMENTS`) is `peers`.

- `peers` — `weaver cluster nodes` to list every peer the kernel knows about, then `weaver cluster status` for overall topology health.
- `listen` — confirm the mesh listener is bound: `ss -tln | grep 9470` (or `netstat -an | findstr 9470` on Windows). If nothing is bound, walk the user through the activation checklist: (1) weaver built with `mesh` feature, (2) JSON config via `--config`, (3) `kernel.mesh.listen_addr` configured, (4) no WSL portproxy holding 9470, (5) Hyper-V firewall rule for WSL profile.
- `chain-tail` — `weaver chain tail` and render the most recent ExoChain events (timestamps + event type + payload summary).

If the kernel is not running, point at `/weftos-kernel boot` instead of failing.
