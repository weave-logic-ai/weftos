# D10 completion contract

Source assessment: a nested project kernel is not a nested user daemon. The existing `project.nested.*` path launches `--profile project`. The remaining implementation must run `--profile user` without `--project`, which otherwise redirects into project supervision.

Required behavior:

- A master-authorized instance registry and signed boot contract bind the inner identity, private HOME/runtime/config, assigned ports, registration grant and tighten-only governance cap.
- Launch with a scrubbed environment. A runtime override alone does not isolate manifests, configuration or user keys. Keep the inner user key and chain private and persistent across restart.
- Validate the master pin, grant, paths and cap before mesh preparation or subsystem construction. Project overlay preparation cannot simply be reused without its project certificate contract; share policy machinery while preserving that validation.
- Isolated mode disables both machine-service discovery and collapsed mesh. `service=off` alone is insufficient.
- Positive outer registration may use explicitly granted collapsed mesh with the inner node key and master-assigned peers/port, subject to normal admission. Do not rebind the outer uid in the machine service to a second user key.
- Readiness and adoption verify instance identity, parent/depth and user key, not just a responsive socket. Revocation stops connectivity before reporting success.
- The inner user daemon must supervise its own project using its own manifest/certificate/token stores.

Tests must cover private state with poisoned outer environment, disabled-master and wrong/replayed grants, cap relaxation/rollback, no outer socket connection while isolated, explicit registration/revocation, required-port collision, and restart/adoption identity. Restarting to apply a cap change is acceptable; claiming unenforced live propagation is not.

Automatic port allocation, a second machine service, seamless live cap updates and richer registration taxonomies are not required for this minimal path. This document is an implementation contract, not evidence of completion.
