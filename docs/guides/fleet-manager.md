# Fleet manager: snapshot, locations and the gateway path

The daemon can describe the whole fleet it knows in one read-only document, and
an operator can say where each node physically is. The console reads the
document through the gateway (ADR-102), never straight from a device and never
through the cog host. Design brief: `docs/research/fleet-manager/network-tab-brief.md`.

## From the command line

```
weaver fleet status
weaver fleet status --json
weaver fleet location set c6-01 --site "Plant 2" --room "Room 14"
```

`status` prints one row per node: trust tier and its source, address, last
heartbeat, mesh connection class, placed cogs and location. `--json` prints the
raw snapshot with the provenance of every field. `location set` is an Admin
verb (the local socket owner); it saves the label in the daemon's runtime
directory (`fleet-locations.json`) and appends a `fleet.location.set` event
(source `fleet`) to the chain. A node id need not be a mesh peer: an edge node
that only checks in to a cog host can be labelled by the id it checks in with.

## `fleet.snapshot` (Read)

The snapshot is composed from what the daemon already holds. It contacts no
peer and no device (which is why it does not call `workload.status`, a Write
verb because it does). Every node section is `{"value": ..., "provenance": ...}`:

| Provenance | Meaning |
|---|---|
| `signed_fact` | Signed by the node it describes and verified by this daemon (`cluster.facts`: capabilities, trust tier, signed envelope). The licence binding is operator-signed. |
| `daemon_observed` | Held or observed by this daemon: cluster state, last seen, mesh connection class and heartbeat, revocations, the placement controller's instances and lifecycle, `infer.status`. |
| `operator_claimed` | Set by an operator: location labels, and the tier of each placement target. |
| `peer_claimed` | Announced by the peer itself over the mesh and not authenticated: the node `name`, and `announced.platform` / `announced.address`. An unverified peer chooses these. |
| `self_reported` | Said by an unauthenticated edge node about itself (the cog-host roster). Display only. |

Top level: `schema`, `source` (`daemon`), `fetched_at`, `ttl_secs`,
`local_node_id`, `degraded[]` (a source that could not be read, with the
reason), `nodes[]`, and daemon-wide `placement`, `infer`, `licence` and
`revocations`. A node carries `node_id`, `name`, `local`, and any of `cluster`,
`facts`, `mesh`, `revoked`, `instances`, `location`.

`cluster` holds the fields this daemon observes (state, first and last seen). A
peer's last-seen time moves only on verified paths, so an unverified announce
cannot make a node look fresh. `licence` is an observed section; only its
`binding` (state, mesh id, sequence, grant fingerprint) comes from an
operator-signed record, and the record itself is not returned. Placement
instances carry the controller's lifecycle (`state`, `restarts`, `reschedules`,
`has_error`) but no error text.

Read is machine-wide: a caller with a machine-level token sees every placed
instance, while a token scoped to a project sees only that project's. A label
for an id nothing else knows (an edge node that only checks in to a cog host)
is flagged `unknown_node: true`.

Labels are free text typed by an operator. The daemon refuses control
characters and invisible Unicode formatting characters (bidi overrides,
zero-width characters), but a console must still escape labels, names and every
other string it shows from a snapshot.

`mesh.rtt_ms` is `null` with a note: nothing in this build measures round-trip
time, and a zero would read as a perfect link. `cluster.last_seen` is the
peer's last heartbeat (RFC 3339), also in `cluster.nodes`.

## How the console gets a token

The console talks to the gateway, which validates bearer tokens through the
daemon (ADR-102 D3). There is no new credential type.

1. Start the daemon and the gateway (`weft gateway`, or `weft ui`).
2. Issue a token on the daemon's machine: `weft token issue --label console`
   (15 minutes by default, 24 hours at most). The secret is printed once.
3. The console sends `Authorization: Bearer <token>` to
   `GET http://<gateway>:18789/api/fleet/snapshot`.

```
curl -H "Authorization: Bearer $WEFT_TOKEN" http://localhost:18789/api/fleet/snapshot
```

What the token can and cannot do:

- The gateway asks the daemon with an explicit `read` scope for this route
  (`DaemonKernelFacade`), so the call can only reach Read-class verbs, and the
  route table maps only `fleet.snapshot`. `fleet.location.set` is Admin on the
  daemon and has no gateway route (a POST to `/api/fleet/location` is 404).
- The token itself is not read-only. A gateway token is owner-equivalent for
  the gateway's REST and MCP surface (ADR-102 D4), so treat it like a shell and
  revoke it when done (`weft token revoke <id>`). Scoped gateway tokens are a
  separate decision.
- Without a token the route answers 401. With no daemon it answers 503
  (`weaver kernel start`).

## Edge heartbeat v2

`POST /fleet/heartbeat` on a cog host accepts the v1 body unchanged and these
optional fields: `uptime_s`, `load` (fraction of one core), `free_heap`,
`reset_reason`, `chip`, `mac`, `channel`, `sample_hz`. Out-of-range numbers are
ignored, strings are capped, and unknown fields are skipped. Every roster row in
`/network` carries `"provenance": "self_reported"`: heartbeats stay
unauthenticated and display-only until leaf keys are provisioned, so nothing may
use them for placement or policy, and the console must label them.
