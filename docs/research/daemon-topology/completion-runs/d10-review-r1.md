# D10 combined source review

Source-only independent review; runtime evidence pending. Three blockers routed to the implementation lane:

1. Recovered nested entries have no owned Child handle. Stop/grant/revoke must wait for their runtime lock to become free or refuse, not unlink an unowned live endpoint and report completion. Never signal an unverified PID.
2. Outbound seed connections assign the configured node ID and pump with AllowAll without authenticating the responder identity. The inbound peer ceiling does not protect this path. Require reciprocal authenticated admission before routing.
3. Repeated start writes a new signed policy while leaving the existing child running. The child later rejects that changed signature. Preserve unchanged effective policy bytes or restart before replacement. Rule hash includes limits; this is an availability defect, not an established cap bypass.

Signed boot, generation floor, private identity, and inbound ceiling showed no source-level replay or cap-relaxation bypass in the reviewed paths. This is not runtime validation.
