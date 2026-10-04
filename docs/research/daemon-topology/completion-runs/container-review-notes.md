# Container lane: preliminary review probes

These are probes against the work in progress, not a final verdict.

- `parse_inspect` currently checks expected mounts exist, but does not reject extra mounts or compare isolation flags. Adoption must reject an identically labelled container with an additional host-root mount, privileged mode, host PID/network namespace, writable root, extra capabilities, or an unexpected image. Test these against fake inspect responses.
- Mount paths are encoded as comma-delimited `--mount` values. Reject or correctly encode paths containing delimiter characters; a single argv element is not sufficient to prevent Docker's mount-option parser interpreting them.
- The operator image configuration is reread on restart/adoption. Persist and compare the launched image digest and engine identity, so a changed operator file cannot cause adoption under a different contract.
- Verify stopped container cleanup and restart do not collide on the deterministic name. Engine `stop` success does not remove a container.
- Parent trust files must not be writable through a second overlapping project/runtime mount. A read-only guest mount does not protect a source file exposed elsewhere through a writable mount.
- Preserve native launch/registration tests while changing shared request and spawn types; test old JSON compatibility and a container ID/host-socket tamper against the signed registration.
- ADR-103 identifies a known parent-auth gap: same-uid child classification uses a supervised process group, which a descendant can escape with `setsid`/`setpgid`. A container with the parent's owner socket mounted must not then obtain literal `auth: "admin"`. Require an authenticated child-only socket or a proven stronger identity boundary, and test an escaped grandchild's literal-admin request.

Source read: uncommitted `project-container` worktree. Findings may already be changing as implementation continues.
