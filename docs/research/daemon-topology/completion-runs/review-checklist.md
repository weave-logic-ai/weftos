# Completion review checks

These are review predicates, not claims that implementation has passed.

## Phase 4

- A requested unsupported sandbox refuses launch; no automatic logical fallback.
- The sandbox permits only the project's intended filesystem, dedicated temporary directory, executable/runtime dependencies and parent socket.
- An isolated kernel cannot signal or inspect sibling daemons merely because it shares their uid.
- Process PID, readiness proof, certificate-bound registration, shutdown, restart and adoption remain correct through each driver.
- Linux containers have a real execution contract and tests, not only generated command strings.
- Nested parentage is authenticated and immutable for the launch; `weave.master` controls the intended relationship, not a boolean bypass.
- Independent nested runtime, keys, chain, ports and governance limits are exercised with isolated release/dev binaries.
- Distinguish running a WASM workload from running a WASM project kernel.

## Leaf

- Certificates and messages use strict signature verification and canonical key-derived node ids.
- Expected parent scope and certificate capabilities are enforced on actual receipt, not only encoded in signatures.
- Target parsing refuses traversal and cross-leaf/cross-scope paths.
- Acknowledgments bind the complete frame identity and cannot erase a different queued record with equal payload.
- Durable replay floors are updated before acknowledgment and survive parent restart. Exact retransmissions at a committed floor are deduplicated; a crash between delivery and floor persistence remains at-least-once unless the application side effect participates in the same transaction.
- Key provisioning never logs seeds or silently overwrites keys; public artifacts contain no private material.
- Discovery is authenticated against a configured trust root and does not silently accept a new parent.
- Offline storage is bounded, survives a torn write, and does not silently discard unacknowledged data.
- Firmware compiles with the shared protocol; host-only tests are not proof of real ESP32 behavior.

## Integration

- Review diffs independently after authors finish, including dependency/feature interactions.
- Run required local typecheck, lint, build, relevant container build and final workspace gate.
- Preserve the live daemon and unrelated working-tree changes.
- Record unexecuted hardware/platform checks explicitly; do not close tickets on an unverified assertion.
