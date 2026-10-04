Implemented the Linux container lifecycle in the isolated worktree. The supervisor now uses an immutable engine ID, distinct host and guest paths, an operator-pinned image, and read-only parent trust and bootstrap mounts. Registration binds the nonce, container ID, and parent-selected host socket to a signature. Readiness and adoption require an inspected container and signed child proof; stop and restart re-inspect the ID before destructive engine calls. Native launch remains the default. Seatbelt and nested registration were left to their assigned lane.

The main changes are in [container.rs](~/.codex/worktrees/project-container/weftos/crates/clawft-weave/src/project_supervisor/container.rs), [child.rs](~/.codex/worktrees/project-container/weftos/crates/clawft-weave/src/project_supervisor/child.rs), [boot.rs](~/.codex/worktrees/project-container/weftos/crates/clawft-weave/src/project_supervisor/boot.rs), and the [registration binding](~/.codex/worktrees/project-container/weftos/crates/clawft-rpc/src/mesh_local.rs:113). I added an [isolated smoke script](~/.codex/worktrees/project-container/weftos/scripts/p4-container-smoke.sh) and [operator guide](~/.codex/worktrees/project-container/weftos/docs/guides/project-linux-container.md).

**Evidence:** `scripts/build.sh check-tests` passed for the four affected crates; 9/9 filtered container tests passed, including fake-engine identity and stop cases; 16/16 scoped supervisor unit tests passed. `git diff --check` and `bash -n` passed. Logs are in [p4-final-gates.log](~/.codex/worktrees/project-container/weftos/target/p4-final-gates.log).

**Still unverified on this host:** the isolated supervisor integration suite could not start child processes in the sandbox (27/32 failed, including a socket `EPERM`). Docker socket access was denied, so the real container smoke script was not run. The exact integration command needing host approval is:

```bash
WEFTOS_RUNTIME_DIR="$PWD/target/p4-tests-runtime" cargo test -p clawft-weave --test project_supervisor -- --nocapture
```

The smoke script also needs Docker or Podman socket access and an operator-supplied digest in `WEFTOS_SMOKE_IMAGE`. No production container or live daemon was changed; no commit or push was made.