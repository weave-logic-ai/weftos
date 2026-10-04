# Linux project containers (ADR-103 Phase 4)

Set `sandbox = "linux-container"` in the manifest's `[serve]` section for a
`child-kernel` project. The default remains `logical` (the legacy `native` spelling is accepted). The user daemon reads
`~/.weftos/project-container.json`, which is outside the project root:

```json
{"engine":"docker","image":"registry.example/weftos@sha256:<64 lowercase hex characters>"}
```

`podman` is also accepted. The image must already be local and contain an
real `clawft-weave` executable `/usr/local/bin/weaver` that accepts `kernel start --foreground --profile project
--project <id>`. The launcher uses `--pull=never`; it does not build or update
the image. `/weftos` is a directory, never the executable: its subdirectories
remain the mount targets below. Do not substitute the `weft`/weftosCLI binary.
The launcher rejects mount targets that descend from or hide the executable;
inspection also checks the exact entrypoint before adoption or mutation.
It selects no network, drops capabilities, prevents privilege
escalation, limits processes, and runs under the host user's UID/GID. Podman
uses `keep-id` mapping. Unsupported engine flags fail the launch.

The guest sees its project at `/weftos/project`, its writable runtime at
`/weftos/run/<id>`, parent-controlled trust files at `/weftos/trust`, and the
dedicated child endpoint at `/weftos/parent/child.sock`. The host child socket is
`<run>/<id>/guest/kernel.sock`. The trust and parent link mounts are read only;
the child cannot rewrite `spawn.json`, `user.pub`, `parent-policy.json`, or the
`revoked` marker. The parent endpoint is served separately from the owner
`kernel.sock` and admits only child RPC methods. The mounted `child-ipc`
directory contains only `child.sock`, so a surviving container can reconnect
after a user-daemon restart.

The engine's 64-hex container ID is persisted before inspection and start.
If a crash leaves only the deterministic name, recovery inspects its immutable
ID, image, mounts and isolation settings before any cleanup. Registration signs
that ID and the parent-selected host socket with a fresh challenge; the parent
checks the outstanding spawn nonce and inspects labels and mounts. Readiness
and adoption also require a fresh project-key-signed handshake. Engine
inspection failures leave the container as an unverifiable leftover and block
duplicate launch. Stop asks the child to shut down, then reinspects the ID
before engine stop or kill. The exited ID remains inspectable until a verified
restart removes it.

To smoke-test an operator image without connecting to a daemon, use:

```bash
WEFTOS_SMOKE_ENGINE=docker \
WEFTOS_SMOKE_IMAGE='registry.example/weftos@sha256:<digest>' \
scripts/p4-container-smoke.sh
```

The script creates an isolated container and directories under `target/`,
checks the immutable ID and mount permissions, runs `/usr/local/bin/weaver kernel start --help`, and
removes its container. It never uses a production project or daemon.
It is an image/CLI preflight only, not driver lifecycle acceptance. The
[fixed driver acceptance plan](../research/daemon-topology/completion-runs/container-lifecycle-plan.md#actual-linux-container-driver-acceptance-fixed-gate)
requires the real parent with `sandbox=linux-container`, immutable engine IDs,
signed registration, crash restart, adoption, revocation and cleanup on a
dedicated disposable Linux engine. Neither this smoke nor the logical-pair
harness can satisfy that gate.
