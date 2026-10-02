# WeftOS Deployment Standard Operating Procedures

Version: 1.0.0
Effective: 2026-04-02
Applies to: WeftOS v0.3.1+
Author: WeaveLogic Architecture Team

---

## Overview

This document defines the standard operating procedures for deploying WeftOS as
a cross-project coordination layer. It was developed against two real
WeaveLogic properties -- weavelogic.ai (B2B consulting site) and
weftos.weavelogic.ai (open-source docs site) -- and is intended as a reusable
template for every client engagement.

Each SOP is self-contained with prerequisites, step-by-step procedures, expected
outputs, quality gates, and known limitations.

---

## Table of Contents

1. [SOP 1: Adding WeftOS to an Existing Project](#sop-1-adding-weftos-to-an-existing-project)
2. [SOP 2: Building the Knowledge Graph](#sop-2-building-the-knowledge-graph)
3. [SOP 3: Cross-Project Coordination](#sop-3-cross-project-coordination)
4. [SOP 4: Continuous Assessment](#sop-4-continuous-assessment)
5. [SOP 5: Iterative SOP Improvement](#sop-5-iterative-sop-improvement)

---

## SOP 1: Adding WeftOS to an Existing Project

### Purpose and Scope

Establish a WeftOS runtime instance within an existing software project so that
the kernel can boot, agents can execute, and the ECC knowledge graph can begin
population. This SOP covers initial installation through first successful kernel
boot.

### Prerequisites

| Requirement | Detail |
|---|---|
| WeftOS CLI (`weft`) | v0.3.1+ installed and on PATH (via cargo install, Homebrew, or binary release) |
| Rust toolchain | 1.93+ (only if building from source) |
| Project VCS | Git repository with at least one commit |
| Disk space | 100 MB minimum for `.weftos/` runtime state (models, sessions, chain) |
| Network | Outbound HTTPS for LLM providers (optional for offline/local inference) |
| Permissions | Write access to project root directory |

### Procedure

#### Step 1: Run `weft init` from the project root

```bash
cd /path/to/project
weft init
```

This invokes `weftos::init::init_project()` which performs three operations:

1. **Creates the `.weftos/` directory structure:**

```
.weftos/
  chain/          # ExoChain append-only event ledger (SHAKE-256 linked)
  tree/           # Exo-resource-tree serialized snapshots
  logs/           # Kernel and agent runtime logs
  artifacts/      # Build artifacts, generated reports, exported graphs
  models/         # ONNX embedding models (all-MiniLM-L6-v2, ~86 MB)
  sessions/
    current/      # Active session JSONL files (one per running session)
    history/      # Archived sessions organized by project name
    subagents/    # Session state for spawned sub-agents
```

2. **Generates `weave.toml`** at the project root with auto-detected settings:
   - `domain.language` -- detected from Cargo.toml (rust), package.json
     (javascript), pyproject.toml (python), or falls back to generic
   - `sources.git` -- enabled if `.git/` exists
   - `sources.files.patterns` -- language-appropriate glob patterns
   - `kernel.max_processes` -- defaults to 64
   - `embedding.provider` -- defaults to `mock-sha256` (no GPU required)
   - `mesh.enabled` -- defaults to false (single-project mode)

3. **Appends `.weftos/` to `.gitignore`** if a gitignore exists and does not
   already contain the entry.

#### Step 2: Configure `weave.toml` for the specific project

Edit the generated `weave.toml` to set project-specific values. The critical
sections for initial deployment are:

```toml
[domain]
name = "weavelogic-ai"          # Human-readable project name
language = "javascript"          # Primary language
description = "B2B consulting website for WeaveLogic"

[kernel]
max_processes = 32               # Tune to project complexity
health_check_interval_secs = 60  # Longer interval for low-churn projects

[embedding]
provider = "onnx"                # Use ONNX for real embeddings
model_path = ".weftos/models/all-MiniLM-L6-v2.onnx"
dimensions = 384
batch_size = 16

[sources.git]
path = "."
branch = "main"                  # or "master"

[sources.files]
root = "."
patterns = ["**/*.ts", "**/*.tsx", "**/*.js", "**/*.json", "**/*.md"]
exclude = ["node_modules/**", ".next/**", "dist/**", ".weftos/**"]

[governance]
default_environment = "development"
risk_threshold = 0.9
```

#### Step 3: Download the embedding model

If using ONNX embeddings (recommended for real knowledge graph construction):

```bash
weft kernel boot --download-models
```

This places `all-MiniLM-L6-v2.onnx` (86 MB) into `.weftos/models/`. The model
produces 384-dimensional sentence embeddings used by HNSW for semantic search.

For lightweight or CI environments, keep `embedding.provider = "mock-sha256"`
which uses SHA-256 content hashing instead of semantic embeddings. This is
deterministic and costs nothing, but loses semantic similarity search.

#### Step 4: Boot the kernel

```bash
weft kernel boot --foreground
```

The boot sequence follows this state machine:

```
Booting -> [init services] -> [init chain] -> [init ECC] -> Running
```

During boot, the kernel:

1. Initializes the `ServiceRegistry` and registers system services
2. Creates the `ProcessTable` for agent lifecycle management
3. Initializes the `ChainSubsystem` (ExoChain with genesis event)
4. Initializes the `EccSubsystem` if the `ecc` feature is enabled:
   - `HnswService` (vector index with ef_search=100, ef_construction=200)
   - `CausalGraph` (concurrent DAG on DashMap)
   - `ImpulseQueue` (inter-structure event bus)
   - `CrossRefStore` (forward/reverse index across structures)
   - `DemocritusLoop` configuration
5. Starts the `HealthSystem` for periodic liveness checks
6. Opens the IPC socket for CLI communication

#### Step 5: Verify the installation

```bash
weft kernel status
```

Expected output shows kernel state as `running`, with service count >= 1 and
process count of 0 (no agents spawned yet).

```bash
weft kernel services
```

Expected output lists at minimum: ExoChain, HNSW (if ECC enabled), Health.


#### Optional: machine mesh service

When the machine mesh service runs (`weaver mesh serve`, or the installed unit),
check it and pin its key once, after comparing the fingerprint `weaver mesh status`
shows with the one an administrator reads on the service host:

```bash
weaver mesh status            # identity, policy, registrations, journal
weaver mesh trust             # shows the machine key fingerprint, then pins it
```

`weaver mesh trust` writes `~/.weftos/mesh/machine.pub`. There is no pin by
default: until it exists, verbs check the key only against the service record
beside the socket (`service.json`), so the first contact is trust-on-first-use.
After pinning, a different key is a hard `machine_key_changed` error; replace
the pin only with `weaver mesh trust --replace` after verifying the new key out
of band. The service listens on `127.0.0.1:9489` by default; exposing it on the
LAN (`listen = "0.0.0.0:9489"` in `mesh.toml`) is an explicit choice.

#### Installing and removing the machine mesh service (administrator)

`weaver mesh install-service` and `weaver mesh uninstall-service` only print a
shell script. They run nothing, write nothing and refuse an `--apply` form. Read
the script, then run it as an administrator from your own account
(`sudo sh install.sh`); the service itself never runs as root.

```bash
weaver mesh install-service > install.sh          # --kind launchd|systemd (default: this host)
weaver mesh install-service --adopt-node-key ~/.weftos/run/node.key > install.sh
less install.sh && sudo sh install.sh             # does NOT start the service
```

What the script does:

- creates the service account and group (`_weftos` on macOS via `dscl`, `weftos`
  on Linux via `systemd-sysusers`), with no login shell;
- **adds the invoking user to that group.** `/var/run/weftos` (the mesh socket and
  `service.json`) is owned by the service account's group with mode 0750, so only
  group members can reach the socket. The membership applies at the next login: log
  out and in, or start a new session, before `weaver mesh status` works. Other users
  on the machine need `dseditgroup -o edit -a USER -t user _weftos` (macOS) or
  `usermod -aG weftos USER` (Linux) to use the service;
- creates `/etc/weftos`, `/var/lib/weftos/mesh` (0700, service account),
  `/var/run/weftos` (0750, service account and group), `/var/log/weftos` (macOS);
- copies the binary to `/usr/local/libexec/weftos/weaver`, root-owned 0755. The
  service never runs from a user-writable path;
- writes `/etc/weftos/mesh.toml` when absent: `listen = "127.0.0.1:9489"` unless you
  pass `--listen`, plus `--admin-uid` ids if given;
- installs the unit (`/Library/LaunchDaemons/ai.weftos.mesh.plist` and a small
  `ai.weftos.mesh-rundir` helper that recreates `/var/run/weftos` at boot, because
  macOS clears `/var/run`; or `/etc/systemd/system/weftos-mesh.service`);
- with `--adopt-node-key PATH`, copies that key to `/var/lib/weftos/mesh/node.key`
  (0600, service account) so the node id does not change. It refuses to overwrite an
  existing different key; an identical key is a no-op, so the script can be re-run.
  The key then exists in two places until you remove the old copy;
- the script is re-runnable. An existing `mesh.toml` is kept and the script says that
  `--listen` / `--admin-uid` were not applied. `--listen` accepts `IP:PORT` or `localhost:PORT` only (no other hostnames); a
  non-loopback address or a port below 1024 is flagged in the script header and on
  stderr (the service has no capabilities and cannot bind a privileged port). Paths
  and ports come from `mesh.toml` only; the units set no environment overrides;
- macOS: the log is `/var/log/weftos/mesh.log` and is NOT rotated yet. `newsyslog`
  cannot rotate a file launchd holds open (no copytruncate; output would go to a
  deleted file), so none is installed. Follow-up: log through os_log/syslog or
  self-rotate in the daemon. The service can start before the rundir helper
  has created `/var/run/weftos`; launchd retries every 10 s until it exists.

The last line is the enable command, printed and commented, not run:
`sudo launchctl bootstrap system /Library/LaunchDaemons/ai.weftos.mesh.plist` or
`sudo systemctl enable --now weftos-mesh`. Stop a collapsed user daemon first
(`weaver kernel stop`); it holds port 9489. Migration from the collapsed daemon is
the owner procedure below.

#### Moving to the machine mesh service (owner migration)

Nothing here is automatic, and no step touches `~/.weftos/run/node.key`,
`~/.weftos/chain/*`, `~/.clawft/*`, any project runtime directory or the Pi's
`cluster_peers.json`; they are only read (the adopt copy). The service writes only
under `/var/lib/weftos/mesh`, the user daemon only `~/.weftos/user.key` and
`~/.weftos/mesh/`.

1. **Install the service, adopting the node key.** Build and install the packaged
   binary, then `weaver mesh install-service --adopt-node-key ~/.weftos/run/node.key
   > install.sh`, read it, and run it as an administrator. It creates the account and
   directories, copies the binary and the key, and writes `mesh.toml`. The key is now
   in two places; the node id, and the id the Pi pinned, are unchanged. Log out and in
   so your account's new group membership applies.
2. **Give the user daemon its user key.** `weaver migrate user-key --dry-run`, then
   without `--dry-run`. `~/.weftos/user.key` gets the `chain.key` seed: same public
   key, same user id. `weaver doctor runtime` WARNs (`user_key_split`) if the two
   ever differ.
3. **Swap the listener.** Stop the collapsed user daemon (`weaver kernel stop
   --profile user`; it holds 9489), start the service (`sudo launchctl bootstrap
   system /Library/LaunchDaemons/ai.weftos.mesh.plist` or `sudo systemctl enable --now
   weftos-mesh`), then `weaver mesh status` and `weaver mesh trust` after comparing
   the fingerprint with the one the install script printed.
4. **Require the service.** Set `service = "required"` under `[kernel.mesh]` in
   `~/.weftos/weave.toml` and start the user daemon again. It registers; the first bind
   is journalled (`how: "tofu"`, or pending until `weaver mesh bind approve <uid>`
   under the `approve` policy). `weaver kernel status --profile user` prints
   `Profile:    user (roles: user)` and `Mesh:       service (connected)`, and the
   `Node:` line keeps the node id from before.
5. **Check from the Pi** that the machine still appears under the same id. Only then
   delete `~/.weftos/run/node.key` (the doctor's `mesh.node_key_dup` WARNs until you
   do; keep a backup until peers reconnect).

Rollback, before step 5's removal: stop the service, set `service = "off"`, restart
the user daemon. It binds 9489 with its own `node.key`; no chain or id has changed.
After the removal, an administrator first copies `/var/lib/weftos/mesh/node.key`
back. Choosing a fresh box key instead of adopting means the Pi must re-pin this
machine.

What the suite cannot check and you verify on the real install: the service
running as `_weftos`/`weftos` on 9489, the adopted node id in `weaver mesh status`,
a second real account unable to take your address (`scripts/dev/mesh-two-uid.sh`,
needs passwordless `sudo -u`), launchd/systemd restarts, the Pi seeing the same id,
and `weaver update` printing (not running) the service restart. The rest of the
Phase 3 exit list runs as your own user in `scripts/build.sh test-mesh-service`.

Removing it: `weaver mesh uninstall-service > uninstall.sh`, read it, run it as root.
It stops the unit and removes the unit files, the root-owned binary and the runtime
directory. It keeps `/var/lib/weftos/mesh` including `node.key`, the journal, logs,
`mesh.toml` and the account. `--purge-key` additionally deletes `node.key` (peers
that pinned this machine will no longer recognise it). Account removal is listed
commented out; before deleting the account run `weaver mesh bind revoke <uid>` for
every bound uid, because a later account that reuses the uid would inherit its bind.

Updating: `weaver update` replaces the user binary and restarts the user daemon. For
the service it only prints the `sudo install ... /usr/local/libexec/weftos/weaver`
and the restart line (`sudo launchctl kickstart -k system/ai.weftos.mesh` or
`sudo systemctl restart weftos-mesh`) when the packaged build differs from the one
`service.json` reports; it never calls `sudo`. `weaver doctor` reports the `mesh.*`
checks (reachability, proto window, pin, journal, box key mode, a leftover
`~/.weftos/run/node.key`, two listeners on 9489, force-revoked users) and the service
tier skew.

### Expected Outputs

| Output | Location | Description |
|---|---|---|
| `.weftos/` directory | Project root | Runtime state directory with all subdirectories |
| `weave.toml` | Project root | Project configuration file |
| `.gitignore` update | Project root | `.weftos/` entry appended |
| Genesis chain event | `.weftos/chain/` | First ExoChain event (tamper-evident root) |
| Kernel boot log | `.weftos/logs/` | Boot sequence with timing data |

### Quality Gates

- [ ] `weft kernel status` returns `running` state
- [ ] `weft kernel services` lists ExoChain service
- [ ] `.weftos/` directory contains chain/, tree/, logs/, artifacts/, sessions/
- [ ] `weave.toml` exists and is valid TOML
- [ ] `.gitignore` contains `.weftos/` entry
- [ ] No secrets or credentials in `weave.toml`

### Known Limitations and Future Improvements

| Limitation | Impact | Future Fix |
|---|---|---|
| `mock-sha256` default embedding | No semantic search until ONNX configured | Auto-download model on first boot (Sprint 15) |
| No `weave.toml` schema validation | Typos in config silently ignored | JSON Schema + `weft doctor` check (Sprint 14) |
| Single kernel instance per project | Cannot run multiple kernels in same `.weftos/` | PID file locking (Sprint 14) |
| No daemon mode | Must keep terminal open or use systemd/pm2 | `weft kernel boot --daemon` (Sprint 15) |

---

## SOP 2: Building the Knowledge Graph

### Purpose and Scope

Populate the ECC (ExoChain + CausalGraph + CrossRef + HNSW) knowledge graph
with comprehensive information about the project's codebase, infrastructure,
dependencies, and operational behavior. This SOP covers initial graph population
and the ongoing DEMOCRITUS refinement loop.

### Prerequisites

| Requirement | Detail |
|---|---|
| SOP 1 completed | Kernel booted and healthy |
| ONNX embeddings enabled | `embedding.provider = "onnx"` in weave.toml |
| Source files accessible | File patterns in `sources.files.patterns` are correct |
| Git history available | `.git/` directory with commit history |

### Procedure

#### Phase 1: Static Code Analysis (Tree-sitter)

The `clawft-plugin-treesitter` crate provides AST parsing, symbol extraction,
and complexity metrics. Spawn a code analysis agent:

```bash
weft agent spawn --type researcher --name code-analyzer \
  --tool treesitter_parse \
  --tool treesitter_symbols \
  --tool treesitter_complexity
```

The agent uses three operations from `analysis.rs`:

1. **`parse_source()`** -- Parses each source file into a tree-sitter AST.
   Supported languages are auto-detected from file extensions.

2. **`extract_symbols()`** -- Extracts named symbols (functions, structs,
   classes, methods, interfaces) with line ranges and visibility.

3. **`compute_complexity()`** -- Calculates cyclomatic complexity, nesting
   depth, and function-level metrics.

For each symbol discovered, the agent emits an impulse to the ECC:

```
ImpulseType::BeliefUpdate
  source_structure: ResourceTree (0x02)
  target_structure: HnswIndex (0x04)
  payload: { symbol_name, file_path, kind, line_range, complexity }
```

**What this produces in the graph:**

- ResourceTree nodes for every file, module, function, type, and trait
- HNSW vectors for every symbol (embedded via the symbol name + docstring +
  context window)
- CausalGraph edges with `CausalEdgeType::Enables` between dependency
  relationships (function A calls function B)

**Current tool status:** Tree-sitter plugin exists with parse, symbols, and
complexity analysis implemented. Languages supported: Rust, TypeScript,
JavaScript, Python, Go, C, C++.

#### Phase 2: Git History Mining

The `clawft-plugin-git` crate wraps `git2` and provides tools for repository
analysis:

```bash
weft agent spawn --type researcher --name git-miner \
  --tool git_log \
  --tool git_diff \
  --tool git_blame \
  --tool git_status
```

The agent performs:

1. **Commit graph traversal** -- Walk all commits, extract messages, authors,
   timestamps, and changed file sets.

2. **Change frequency analysis** -- Identify hot files (frequently changed) and
   change coupling (files that change together).

3. **Blame-based ownership** -- Determine per-file and per-function ownership
   from `git blame`.

4. **Branch topology** -- Map branch structure, merge patterns, and release
   tags.

**What this produces in the graph:**

- CausalGraph edges with `CausalEdgeType::Follows` for temporal commit
  ordering
- CausalGraph edges with `CausalEdgeType::Correlates` for files that
  co-change (statistical coupling)
- ExoChain events for each significant commit (tagged, merged, or affecting
  many files)
- CrossRef entries linking commits to the files/symbols they touch

**Current tool status:** Git plugin fully implemented with clone, commit,
branch, diff, blame, log, and status operations via `git2`.

#### Phase 3: Dependency Analysis

For each project type, a specialized scan extracts the dependency graph:

| Project Type | Source | Tool |
|---|---|---|
| Rust | `Cargo.toml` + `Cargo.lock` | `clawft-plugin-cargo` (crate metadata, dep tree) |
| Node.js | `package.json` + `package-lock.json` | Custom agent (parse JSON, resolve versions) |
| Python | `pyproject.toml` / `requirements.txt` | Custom agent (parse TOML/text) |
| Docker | `Dockerfile` + `docker-compose.yml` | `clawft-plugin-containers` |
| Infrastructure | Terraform, Vercel config | Custom agent (parse HCL/JSON) |

```bash
weft agent spawn --type researcher --name dep-analyzer \
  --tool cargo_metadata \
  --tool file_read
```

**What this produces in the graph:**

- ResourceTree nodes for each dependency (name, version, features)
- CausalGraph edges with `CausalEdgeType::Enables` between dependency and
  dependent
- CausalGraph edges with `CausalEdgeType::Inhibits` for version conflicts
  or known CVEs

**Current tool status:** The Cargo plugin exists. Node.js and Python dependency
parsing requires custom agent prompts using `file_read` tool -- no dedicated
plugin yet.

#### Phase 4: Infrastructure and Deployment Scanning

Gather deployment topology from configuration files:

```bash
weft agent spawn --type researcher --name infra-scanner \
  --tool file_read \
  --tool file_search
```

The agent scans for and parses:

- `vercel.json` / `.vercel/` -- Deployment configuration, environment bindings
- `Dockerfile` / `docker-compose.yml` -- Container definitions, port mappings
- `.github/workflows/*.yml` -- CI/CD pipeline definitions, test/deploy stages
- `.env.example` / `.env.production` -- Environment variable inventory (names
  only, never values)
- DNS/domain configuration -- Extracted from Vercel project settings or
  infrastructure-as-code

**What this produces in the graph:**

- ResourceTree nodes for each deployment target, environment, and service
- CausalGraph edges with `CausalEdgeType::Causes` linking deploy configs to
  the services they produce
- CausalGraph edges with `CausalEdgeType::Enables` linking environment
  variables to the features they gate

**Current tool status:** Containers plugin exists. Vercel/GitHub Actions
parsing requires custom agents. This is a priority gap for Sprint 14.

#### Phase 5: DEMOCRITUS Loop Activation

Once the initial scan phases complete, the DEMOCRITUS cognitive loop takes over
for continuous refinement. The loop runs on every kernel tick (default 50ms):

```
SENSE -> EMBED -> SEARCH -> UPDATE -> COMMIT
```

1. **SENSE** -- Drains the `ImpulseQueue` for up to `max_impulses_per_tick`
   (default 64) new events from the scan agents.

2. **EMBED** -- Produces vector embeddings for each impulse payload using
   the configured `EmbeddingProvider` (ONNX all-MiniLM-L6-v2).

3. **SEARCH** -- Queries HNSW for `search_k` (default 5) nearest neighbors
   to each new embedding. This finds semantically similar existing nodes.

4. **UPDATE** -- For each pair that exceeds `correlation_threshold` (default
   0.7 cosine similarity), creates a `CausalEdgeType::Correlates` edge in
   the CausalGraph and registers a `CrossRef` in the CrossRefStore.

5. **COMMIT** -- Logs the tick result to ExoChain for auditability.

The loop operates within a `tick_budget_us` (default 15ms) and will stop early
if the budget is exceeded, ensuring the kernel remains responsive.

**Tick result metrics:**

```rust
DemocritusTickResult {
    impulses_sensed: usize,
    embeddings_produced: usize,
    searches_performed: usize,
    edges_added: usize,
    crossrefs_added: usize,
    budget_exceeded: bool,
    duration_us: u64,
}
```

#### Phase 6: Cross-Reference Consolidation

After initial population, run a consolidation pass:

```bash
weft agent spawn --type researcher --name crossref-builder \
  --tool graph_query \
  --tool graph_update
```

This agent walks the CausalGraph and CrossRefStore to:

1. Resolve transitive dependencies (A enables B, B enables C => A transitively
   enables C)
2. Identify contradiction clusters (conflicting edges that need review)
3. Mark high-centrality nodes (files/symbols that many things depend on)
4. Calculate graph coherence metrics

Each cross-reference uses `UniversalNodeId` (BLAKE3 hash of structure_tag +
context_id + hlc_timestamp + content_hash + parent_id) to uniquely identify
nodes across all four ECC structures (ExoChain, ResourceTree, CausalGraph,
HnswIndex).

### Expected Outputs

| Output | Description |
|---|---|
| Populated ResourceTree | Hierarchical model of files, modules, symbols, dependencies, infra |
| CausalGraph | DAG with typed/weighted edges (Causes, Enables, Correlates, etc.) |
| HNSW index | Vector embeddings for semantic search across all entities |
| CrossRefStore | Bidirectional index linking nodes across all ECC structures |
| ExoChain audit trail | Tamper-evident log of every graph mutation |
| Graph statistics | Node count, edge count, coherence score, coverage percentage |

### Quality Gates

- [ ] ResourceTree node count > 0 for each scanned source directory
- [ ] CausalGraph contains at least `Enables` edges for dependency relationships
- [ ] HNSW index has entries for all extracted symbols
- [ ] CrossRef count > 0 (structures are actually linked)
- [ ] DEMOCRITUS tick results show `edges_added > 0` in at least one cycle
- [ ] ExoChain has genesis + at least one scan event
- [ ] No ExoChain hash verification failures

### Known Limitations and Future Improvements

| Limitation | Impact | Future Fix |
|---|---|---|
| No dedicated Node.js dependency plugin | Must use generic file_read + custom prompts | `clawft-plugin-npm` (Sprint 15) |
| No Vercel/GitHub Actions parser | Infra scanning is shallow | `clawft-plugin-ci` (Sprint 15) |
| Tree-sitter grammars bundled at compile time | Cannot add languages without rebuild | Dynamic grammar loading (Sprint 16) |
| ONNX model is 86 MB | Large for CI or ephemeral environments | Quantized model option (Sprint 15) |
| No incremental re-scan | Full rescan on every boot | File-watcher-based delta scan (Sprint 14) |
| DEMOCRITUS correlation_threshold is global | Same threshold for all entity types | Per-entity-type thresholds (Sprint 15) |

---

## SOP 3: Cross-Project Coordination

### Purpose and Scope

Enable two or more WeftOS instances running on different projects to share
knowledge, coordinate changes, and maintain a federated view of the full system.
This SOP covers the mesh networking setup, trust model, and coordination
patterns.

### Prerequisites

| Requirement | Detail |
|---|---|
| SOP 1 + SOP 2 completed | On each participating project |
| Network connectivity | Projects must be able to reach each other (same host, LAN, or WAN) |
| Shared org identity | Projects belong to the same governance domain |
| Mesh feature enabled | `weft` binary compiled with `mesh` feature |

### Procedure

#### Step 1: Designate Project Roles

For the initial WeaveLogic deployment:

| Project | Role | Mesh Address |
|---|---|---|
| clawft (weftos.weavelogic.ai docs) | **Coordinator** | `0.0.0.0:9489` |
| weavelogic.ai | **Member** | `0.0.0.0:9471` |

The coordinator is typically the project with the richer knowledge graph or the
one that owns shared infrastructure. In this case, clawft contains the WeftOS
source code which is the authoritative reference.

#### Step 2: Configure Mesh Networking

**On the coordinator (clawft):**

```toml
# weave.toml
[mesh]
enabled = true
bind_address = "0.0.0.0:9489"
seed_peers = []
node_id = "clawft-coordinator"

[mesh.tls]
cert_path = ".weftos/certs/node.crt"
key_path = ".weftos/certs/node.key"
ca_path = ".weftos/certs/ca.crt"
```

**On the member (weavelogic.ai):**

```toml
# weave.toml
[mesh]
enabled = true
bind_address = "0.0.0.0:9471"
seed_peers = ["127.0.0.1:9489"]   # Or the coordinator's reachable address
node_id = "weavelogic-ai-member"

[mesh.tls]
cert_path = ".weftos/certs/node.crt"
key_path = ".weftos/certs/node.key"
ca_path = ".weftos/certs/ca.crt"
```

#### Step 3: Establish the Trust Model

The mesh transport (defined in `mesh.rs`) uses the Noise protocol for encrypted
peer-to-peer communication. Trust is established through:

1. **Genesis hash verification** -- Both nodes must share the same governance
   genesis hash. If they differ, the connection is rejected with
   `MeshError::GenesisMismatch`. This ensures both projects belong to the
   same organizational governance domain.

2. **Peer discovery** -- Three mechanisms are available:
   - `SeedPeer` -- Static addresses in `mesh.seed_peers` (always available)
   - `Mdns` -- Automatic LAN discovery (behind `mesh-discovery` feature)
   - `Kademlia` -- WAN discovery via DHT (behind `mesh-discovery` feature)
   - `PeerExchange` -- Learned from connected peers' peer lists

3. **Capability-based access** -- The `AgentCapabilities` system controls what
   operations remote peers can perform. Capabilities are checked by the
   `CapabilityChecker` before any cross-project operation executes.

**For same-host deployments** (like WeaveLogic's server):

Both projects run on the same machine. Use `127.0.0.1` addresses and skip TLS
(development mode). The governance genesis is shared by deriving it from a
common org secret.

**For cross-host deployments** (client engagements):

Use TLS with mutual certificate authentication. Each project generates a
keypair and the CA certificate is distributed to all participating projects.

#### Step 4: Configure the Federated Knowledge Graph

The ECC graph spans projects through a federated model, not a single shared
database. Each project maintains its own:

- ExoChain (local append-only ledger)
- ResourceTree (local resource hierarchy)
- CausalGraph (local causal DAG)
- HNSW index (local vector store)

Cross-project links are established through `CrossRef` entries with
`StructureTag::Custom(0x10)` designating "remote project" references:

```
UniversalNodeId (local) <--CrossRef--> UniversalNodeId (remote)
```

The mesh layer synchronizes these cross-references using three protocols:

1. **Gossip** -- Lightweight metadata exchange (node counts, edge counts,
   last-updated timestamps) on a 30-second interval.

2. **Query forwarding** -- When a local HNSW search returns results above
   threshold, the query is optionally forwarded to connected peers for
   broader search. Results are merged and ranked.

3. **Event bridging** -- Significant ExoChain events (deploys, test failures,
   dependency updates) are broadcast to connected peers as impulses.

#### Step 5: Define Coordination Patterns

For the WeaveLogic deployment, three coordination patterns are relevant:

**Pattern A: Shared Dependency Tracking**

Both projects use Next.js. When weavelogic.ai updates its Next.js version,
the coordinator detects the dependency change and:

1. Emits an impulse to the clawft docs project
2. The docs project's DEMOCRITUS loop receives the impulse
3. A CausalGraph edge `CausalEdgeType::Correlates` is created linking the
   two dependency nodes
4. If the version differs, a `CausalEdgeType::Contradicts` edge flags the
   mismatch for review

**Pattern B: Brand and Design Consistency**

Both projects share brand assets (colors, logos, typography). The cross-ref
system tracks:

- Shared CSS variables / Tailwind config values
- Common component patterns (headers, footers, CTAs)
- Design token files

When a brand asset changes in one project, the mesh broadcasts the change and
the receiving project's knowledge graph flags all dependent components.

**Pattern C: Deploy Ordering**

When both projects deploy to Vercel, the coordination layer can enforce ordering
rules:

- Docs site deploys should follow (not precede) source code changes
- If clawft publishes a new API doc, the docs deploy should include it
- If weavelogic.ai CTA links to docs, both deploys should be coordinated

This is tracked via CausalGraph edges with `CausalEdgeType::Follows` between
deploy events in each project's ExoChain.

### Expected Outputs

| Output | Description |
|---|---|
| Mesh connection established | Two kernels connected and exchanging gossip |
| Cross-project CrossRefs | References linking entities across project boundaries |
| Dependency correlation edges | CausalGraph edges for shared dependencies |
| Event bridge active | ExoChain events flowing between projects |
| Federated search working | HNSW queries returning results from both projects |

### Quality Gates

- [ ] `weft kernel services` on each project shows Mesh service as healthy
- [ ] Peer discovery confirms both nodes see each other
- [ ] At least one cross-project CrossRef exists after initial sync
- [ ] Gossip interval producing regular metadata exchange
- [ ] No `GenesisMismatch` errors in kernel logs

### Known Limitations and Future Improvements

| Limitation | Impact | Future Fix |
|---|---|---|
| Mesh networking is transport-layer only (K6.1) | No application-level protocol for graph sync | Graph sync protocol (Sprint 16) |
| No conflict resolution for competing edges | Concurrent edits to same cross-ref can diverge | CRDT-based edge merging (Sprint 17) |
| Gossip is push-only | New peers must wait for next gossip round | Pull-on-connect (Sprint 15) |
| No multi-tenant isolation within mesh | All connected projects see all events | Namespace isolation (Sprint 14, deferred) |
| Discovery backends (mDNS, Kademlia) are feature-gated | Must compile with `mesh-discovery` | Enable by default in v0.4 |
| No cross-project agent migration | Agents cannot move between projects | Agent serialization + transfer (Sprint 17) |

---

## SOP 4: Continuous Assessment

### Purpose and Scope

Maintain the knowledge graph as a living, continuously updated representation
of the system. This SOP covers the triggers, monitors, and reporting mechanisms
that keep the graph current and surface findings to stakeholders.

### Prerequisites

| Requirement | Detail |
|---|---|
| SOP 1 + SOP 2 completed | Knowledge graph populated |
| File watcher available | `notify` crate (workspace dependency, already included) |
| Git hooks writable | Ability to install post-commit hooks |
| CI/CD access | Ability to add pipeline steps (for CI-triggered assessment) |

### Procedure

#### Step 1: Configure Assessment Triggers

Four trigger types drive re-assessment:

**Trigger A: File System Watch (Real-time)**

The `notify` crate (v7, already a workspace dependency) watches the project
directory for file changes:

```toml
# weave.toml
[assessment.triggers.filesystem]
enabled = true
debounce_ms = 2000         # Wait 2s after last change before scanning
patterns = ["**/*.ts", "**/*.rs", "**/*.json"]
exclude = ["node_modules/**", "target/**", ".weftos/**"]
```

When a file change is detected:
1. The modified file is re-parsed by tree-sitter
2. Changed symbols are re-embedded and re-indexed in HNSW
3. ResourceTree nodes are updated
4. DEMOCRITUS processes the resulting impulses on the next tick

**Trigger B: Git Hook (On Commit)**

Install a post-commit hook:

```bash
weft hooks install --type post-commit
```

This creates `.git/hooks/post-commit` that calls `weft assess --scope commit`,
which:
1. Reads the commit diff to identify changed files
2. Runs targeted tree-sitter analysis on changed files only
3. Updates git-derived CausalGraph edges (co-change correlations)
4. Logs the assessment to ExoChain

**Trigger C: CI Pipeline (On Push/PR)**

Add a step to the GitHub Actions workflow:

```yaml
- name: WeftOS Assessment
  run: |
    weft assess --scope ci --format github-annotations
```

This produces GitHub-compatible annotations for:
- Complexity regressions (function complexity increased)
- Dependency changes (new, removed, or upgraded dependencies)
- Coupling anomalies (unexpected file co-changes)
- Coverage of the knowledge graph (percentage of code indexed)

**Trigger D: Scheduled (Cron)**

The kernel's built-in cron subsystem (implemented in `cron.rs`) runs periodic
full assessments:

```toml
# weave.toml
[assessment.triggers.scheduled]
enabled = true
cron = "0 2 * * *"         # Daily at 2 AM
scope = "full"              # Full rescan, not incremental
```

#### Step 2: Configure the Assessment Pipeline

Each assessment run follows this pipeline:

```
TRIGGER -> SCOPE -> SCAN -> ANALYZE -> REPORT -> COMMIT
```

1. **SCOPE** -- Determine what to scan based on the trigger type:
   - `commit` -- Only files changed in the last commit
   - `ci` -- All files changed in the PR/push
   - `full` -- Everything in `sources.files.patterns`
   - `dependency` -- Only dependency manifests (Cargo.toml, package.json)

2. **SCAN** -- Run the appropriate Phase 1-4 agents from SOP 2 on the
   scoped file set.

3. **ANALYZE** -- Compare new scan results against the existing graph:
   - New nodes = additions to the codebase
   - Missing nodes = deletions
   - Changed embeddings = modified semantics
   - New edges = new relationships discovered
   - Broken edges = dependencies that no longer hold

4. **REPORT** -- Generate findings in the configured format.

5. **COMMIT** -- Log the assessment result to ExoChain.

#### Step 3: Configure Reporting

Reports can be surfaced through multiple channels:

**Terminal Output (default):**

```bash
weft assess --scope full --format table
```

Produces a summary table with:
- Total nodes / edges / cross-refs
- Nodes added / removed / modified since last assessment
- Top 5 highest-centrality nodes (most depended-upon)
- Top 5 highest-churn files (most frequently changed)
- Coherence score (ratio of edges with evidence vs. total edges)

**JSON Export:**

```bash
weft assess --scope full --format json > .weftos/artifacts/assessment-latest.json
```

Produces a machine-readable report suitable for dashboard ingestion or
comparison between assessments.

**GitHub PR Comments (future):**

```bash
weft assess --scope ci --format github-pr --pr-number 42
```

Posts a comment on the PR with assessment findings. This requires the
`gh` CLI and appropriate permissions. Not yet implemented.

**Dashboard (future):**

The GUI block engine (Sprint 13) provides a `BudgetBlock` for per-agent cost
tracking. Additional assessment blocks are planned for Sprint 15:
- Graph health block (node/edge counts over time)
- Churn heatmap block (file change frequency visualization)
- Dependency graph block (interactive dependency visualization)

#### Step 4: Connect to the WeaveLogic Consulting Product

This SOP is the operational backbone of WeaveLogic's consulting offering. The
connection points are:

1. **Initial Assessment Report** -- Run SOP 1 + SOP 2 on a client's codebase.
   The resulting knowledge graph and assessment report form the basis of the
   initial consulting engagement.

2. **Ongoing Monitoring** -- Set up continuous assessment (this SOP) to
   provide ongoing value. Monthly reports showing graph evolution, complexity
   trends, and coupling patterns.

3. **AI Assessor Integration** (Sprint 14 backlog) -- An LLM-powered agent
   that interprets the knowledge graph and produces natural-language
   recommendations. This agent reads the CausalGraph, identifies risk
   patterns, and generates actionable findings.

4. **Comparison Reports** -- Compare a client's graph metrics against
   baselines (industry averages or the client's own historical data) to
   show improvement over time.

### Expected Outputs

| Output | Description |
|---|---|
| File watcher active | Real-time re-indexing of changed files |
| Git hook installed | Post-commit assessment trigger |
| CI step configured | PR/push assessment with annotations |
| Cron assessment | Nightly full rescan |
| Assessment reports | JSON + table output for each assessment run |
| ExoChain audit trail | Every assessment logged with results |

### Quality Gates

- [ ] File watcher detects and re-indexes a test file change within 5 seconds
- [ ] Post-commit hook runs without error on a test commit
- [ ] Assessment report contains non-zero node and edge counts
- [ ] Coherence score is reported and > 0
- [ ] ExoChain contains assessment events with valid hash chain
- [ ] CI assessment completes within 60 seconds for incremental scope

### Known Limitations and Future Improvements

| Limitation | Impact | Future Fix |
|---|---|---|
| No incremental tree-sitter re-parse | Full file re-parse on every change | Incremental parsing with tree-sitter edit API (Sprint 15) |
| No GitHub PR comment integration | Findings only in CI logs | `weft assess --format github-pr` (Sprint 15) |
| No baseline comparison | Cannot show improvement over time | Historical assessment storage + diff (Sprint 16) |
| AI Assessor not implemented | No natural-language recommendations | AI Assessor agent type (Sprint 14) |
| No alerting/notification | Must check reports manually | Webhook + email alerts on threshold breach (Sprint 16) |

---

## SOP 5: Iterative SOP Improvement

### Purpose and Scope

Use the WeftOS knowledge graph and operational telemetry to continuously improve
these SOPs themselves. This SOP defines the feedback loop between execution
data and procedure refinement.

### Prerequisites

| Requirement | Detail |
|---|---|
| SOPs 1-4 executed at least once | Baseline data from at least one full deployment |
| ExoChain with assessment history | Multiple assessment events for trend analysis |
| Session history | At least 5 sessions in `.weftos/sessions/history/` |

### Procedure

#### Step 1: Define SOP Effectiveness Metrics

Each SOP tracks metrics that indicate whether it is working correctly:

**SOP 1 (Adding WeftOS) Metrics:**

| Metric | Target | Source |
|---|---|---|
| Time to first kernel boot | < 10 minutes | ExoChain genesis timestamp - init timestamp |
| Config errors on first boot | 0 | Kernel boot log error count |
| Manual config edits needed | < 5 | Session history (count of weave.toml edits) |

**SOP 2 (Knowledge Graph) Metrics:**

| Metric | Target | Source |
|---|---|---|
| Code coverage (% files indexed) | > 90% | ResourceTree node count / total file count |
| Symbol coverage | > 80% | Extracted symbols / estimated total symbols |
| Graph density | > 0.01 | Edge count / (node count * (node count - 1)) |
| DEMOCRITUS tick efficiency | < 15ms avg | DemocritusTickResult.duration_us |
| Correlation discovery rate | > 0 per day | DemocritusTickResult.edges_added cumulative |

**SOP 3 (Cross-Project) Metrics:**

| Metric | Target | Source |
|---|---|---|
| Mesh uptime | > 99% | Gossip interval regularity |
| Cross-ref count | > 10 | CrossRefStore with remote StructureTag |
| Event bridge latency | < 1s | Timestamp diff between emit and receive |
| Federated search recall | > 70% | Manual spot-check of cross-project queries |

**SOP 4 (Continuous Assessment) Metrics:**

| Metric | Target | Source |
|---|---|---|
| Assessment latency (incremental) | < 30s | ExoChain event timestamps |
| Assessment latency (full) | < 5m | ExoChain event timestamps |
| False positive rate | < 10% | Manual review of flagged findings |
| Graph staleness | < 24h | Max time since last ResourceTree update |

#### Step 2: Collect Execution Telemetry

Every SOP execution is instrumented through the ExoChain. Each significant
step produces a chain event with:

- `kind`: The SOP step identifier (e.g., "sop1.kernel_boot", "sop2.treesitter_scan")
- `source`: The agent or operator that executed the step
- `payload`: Timing data, error counts, output statistics
- `prev_hash`: Link to the previous event (tamper-evident chain)
- `payload_hash`: Content commitment for the payload

This data is queryable through:

```bash
weft chain query --kind "sop*" --since "7d" --format json
```

#### Step 3: Analyze SOP Performance

Run a periodic (monthly) SOP review by spawning an analysis agent:

```bash
weft agent spawn --type researcher --name sop-analyst \
  --tool chain_query \
  --tool graph_query
```

The agent:

1. Queries ExoChain for all SOP-related events in the review period
2. Calculates the metrics from Step 1 against their targets
3. Identifies steps that consistently exceed time targets
4. Identifies steps that produce errors or require manual intervention
5. Generates a SOP Performance Report

#### Step 4: Apply Improvements

Based on the analysis, improvements fall into three categories:

**Category A: Procedure Updates**

If a step consistently fails or requires manual intervention, update the SOP
procedure text. Track the change in the ExoChain:

```bash
weft chain append --kind "sop.update" --source "operator" \
  --payload '{"sop": 2, "step": "phase1", "change": "added exclude pattern for generated files"}'
```

**Category B: Tooling Improvements**

If a step is slow or error-prone due to missing tooling, create a backlog item:

```bash
weft task create --title "Build npm dependency parser plugin" \
  --priority high \
  --labels "sop-improvement,tooling" \
  --body "SOP 2 Phase 3 requires manual parsing of package.json. Build clawft-plugin-npm."
```

**Category C: Threshold Tuning**

If metrics consistently miss targets, evaluate whether the target is wrong
(adjust the SOP) or the system needs improvement (file a bug):

- `correlation_threshold` too high? Lower it and measure false positive impact.
- `tick_budget_us` too low? Increase it and measure kernel responsiveness impact.
- `max_impulses_per_tick` bottleneck? Increase and watch memory usage.

#### Step 5: Version the SOPs

SOPs are versioned documents. Each update increments the version:

- **Patch** (1.0.x): Typo fixes, clarification, no behavioral change
- **Minor** (1.x.0): New steps, changed thresholds, improved quality gates
- **Major** (x.0.0): Structural reorganization, new prerequisites, breaking
  changes to expected outputs

The version history is tracked in the ExoChain and in this document's header.

### Expected Outputs

| Output | Description |
|---|---|
| SOP Performance Report | Monthly metrics vs. targets for each SOP |
| Improvement backlog items | Tasks for tooling and procedure improvements |
| Updated SOP document | New version with applied improvements |
| ExoChain update events | Audit trail of all SOP changes |

### Quality Gates

- [ ] All SOP metrics have defined targets and data sources
- [ ] Monthly review completed within 1 business day
- [ ] Every SOP change logged in ExoChain
- [ ] No metric below target for 3 consecutive months without action
- [ ] SOP version incremented on every substantive change

### Known Limitations and Future Improvements

| Limitation | Impact | Future Fix |
|---|---|---|
| Manual metric collection | Monthly review is labor-intensive | Automated metric dashboard (Sprint 16) |
| No A/B testing of SOP changes | Cannot empirically compare procedure variants | SOP variant tracking in ExoChain (Sprint 17) |
| Session history is JSONL, not indexed | Slow to query across many sessions | Session indexing with HNSW (exists in session_indexer.rs) |
| No cross-client SOP benchmarking | Cannot compare deployment efficiency across engagements | Anonymized metric aggregation (v1.0) |

---

## Appendix A: Concrete Deployment Plan for WeaveLogic Properties

This appendix applies the SOPs above to the two WeaveLogic properties.

### Project 1: clawft (weftos.weavelogic.ai)

| Attribute | Value |
|---|---|
| Path | `/claw/root/weavelogic/projects/clawft/` |
| Type | Rust workspace (22 crates) + Fumadocs site (Next.js 16) |
| Current .weftos/ | Exists, populated (sessions, ONNX model, handoff doc) |
| Status | SOP 1 partially complete (directory exists, no weave.toml yet) |

**Remaining SOP 1 steps:**
1. Generate `weave.toml` via `weft init` (will detect Rust + Node.js)
2. Configure dual source patterns: `["**/*.rs", "**/*.ts", "**/*.tsx"]`
3. Exclude: `["target/**", "node_modules/**", ".weftos/**", "docs/src/.next/**"]`
4. Boot kernel with ONNX embeddings (model already present)

**SOP 2 priority data sources:**
1. Cargo workspace dependency graph (22 crates, rich internal dependency data)
2. Tree-sitter on all `.rs` files (kernel, core, plugins -- the heart of WeftOS)
3. Git history (rich, 13+ sprints of development)
4. Vercel deployment config (docs/src/)
5. GitHub Actions workflows (.github/workflows/)

### Project 2: weavelogic.ai

| Attribute | Value |
|---|---|
| Path | `/claw/root/weavelogic/projects/weavelogic.ai/` |
| Type | Next.js monorepo (frontend + API) with Prisma ORM |
| Current .weftos/ | Does not exist |
| Status | SOP 1 not started |

**SOP 1 steps:**
1. Run `weft init` from project root
2. `weave.toml` will detect Node.js (package.json present)
3. Configure patterns: `["**/*.ts", "**/*.tsx", "**/*.js", "**/*.prisma"]`
4. Exclude: `["node_modules/**", ".next/**", ".weftos/**"]`
5. Copy ONNX model from clawft (avoid re-downloading)
6. Boot kernel

**SOP 2 priority data sources:**
1. Package.json workspace structure (services/frontend, services/api, packages/*)
2. Prisma schema (database model -- extremely high value for knowledge graph)
3. Next.js page/route structure (app/ directory)
4. API endpoint definitions
5. Git history

### Cross-Project Coordination (SOP 3)

**Shared dependencies to track:**
- Next.js version (both projects)
- TypeScript version
- Tailwind CSS (if used in both)
- Vercel deployment platform

**Coordination patterns to implement:**
1. Dependency version sync alerts
2. Brand asset change propagation
3. Deploy ordering (clawft source changes -> docs deploy -> weavelogic.ai CTA verification)

**Mesh configuration:**
Both projects on the same host, use localhost addresses, development TLS mode.

---

## Appendix B: Tool Inventory and Gap Analysis

### Existing Tools (v0.3.1)

| Tool | Crate | Capability |
|---|---|---|
| tree-sitter parse/symbols/complexity | `clawft-plugin-treesitter` | AST analysis for 7 languages |
| git status/log/diff/blame/commit/branch/clone | `clawft-plugin-git` | Full git2 integration |
| cargo metadata | `clawft-plugin-cargo` | Rust dependency analysis |
| container operations | `clawft-plugin-containers` | Docker/Compose parsing |
| browser automation | `clawft-plugin-browser` | Headless browser for web testing |
| OAuth2 flows | `clawft-plugin-oauth2` | Authentication for API access |
| calendar integration | `clawft-plugin-calendar` | Scheduling (useful for cron assessment) |
| file read/write/search | `clawft-tools` | Basic file operations |
| HNSW vector search | `clawft-kernel` (HnswService) | Semantic similarity search |
| CausalGraph CRUD | `clawft-kernel` (causal.rs) | DAG with typed edges |
| ExoChain append/query | `clawft-kernel` (chain.rs) | Tamper-evident audit log |
| CrossRef store | `clawft-kernel` (crossref.rs) | Universal node linking |
| DEMOCRITUS loop | `clawft-kernel` (democritus.rs) | Continuous cognitive refinement |
| Impulse queue | `clawft-kernel` (impulse.rs) | Inter-structure event bus |
| Mesh transport | `clawft-kernel` (mesh*.rs) | Noise-encrypted peer networking |
| Embedding (ONNX) | `clawft-kernel` (embedding_onnx.rs) | 384-dim sentence embeddings |

### Tools Needed (Gap)

| Tool | Priority | Target Sprint | Purpose |
|---|---|---|---|
| `clawft-plugin-npm` | High | Sprint 15 | Node.js dependency graph parsing |
| `clawft-plugin-ci` | High | Sprint 15 | GitHub Actions / Vercel config parsing |
| `clawft-plugin-prisma` | Medium | Sprint 15 | Prisma schema -> ResourceTree |
| `weft assess` CLI command | High | Sprint 14 | Unified assessment entry point |
| `weft hooks install` CLI | Medium | Sprint 14 | Git hook management |
| `weft chain query` CLI | Medium | Sprint 14 | ExoChain querying from command line |
| Graph sync protocol | Low | Sprint 16 | Application-level mesh graph sync |
| AI Assessor agent | High | Sprint 14 | LLM-powered finding generation |
| Dashboard blocks | Medium | Sprint 15 | Visual assessment output |
| PR comment formatter | Low | Sprint 15 | GitHub PR integration |

---

## Appendix C: Security Considerations

### Data Classification

| Data | Classification | Handling |
|---|---|---|
| Source code (indexed) | Confidential | Stored only in .weftos/ (gitignored), never transmitted unencrypted |
| Embeddings | Internal | HNSW vectors are not reversible to source but contain semantic signal |
| ExoChain events | Internal | Append-only, signed with Ed25519 + optional ML-DSA-65 |
| Environment variable names | Confidential | Names indexed, values NEVER stored or transmitted |
| Environment variable values | Secret | NEVER indexed, NEVER stored in knowledge graph |
| Git commit messages | Internal | Indexed for semantic search |
| API keys/credentials | Secret | NEVER processed by any WeftOS agent or tool |
| Cross-project gossip | Internal | Encrypted via Noise protocol in mesh transport |

### Access Control

- Agents operate under the `AgentCapabilities` system
- Each agent has a defined capability set (filesystem read, filesystem write,
  network, etc.)
- The `CapabilityChecker` validates every tool invocation against the agent's
  capabilities
- Cross-project mesh connections require governance genesis hash match
- The governance gate (`GateBackend`) can block operations that exceed the
  configured `risk_threshold`

### Audit Trail

Every significant operation is logged to the ExoChain with:
- SHAKE-256 hash linking (tamper-evident chain)
- Ed25519 signatures (authenticity)
- Optional ML-DSA-65 dual signatures (post-quantum readiness)
- RVF (RuVector Format) segment encoding with witness chains

---

## Appendix D: Quick Reference Card

```
# Install WeftOS on a project
cd /path/to/project && weft init

# Configure (edit generated file)
$EDITOR weave.toml

# Boot the kernel
weft kernel boot --foreground

# Check kernel health
weft kernel status
weft kernel services

# Spawn analysis agents
weft agent spawn --type researcher --name code-analyzer --tool treesitter_parse
weft agent spawn --type researcher --name git-miner --tool git_log

# Run assessment
weft assess --scope full --format table

# Connect two projects (mesh)
# Edit weave.toml [mesh] section on both projects, then boot both kernels

# Query the knowledge graph
weft chain query --kind "sop*" --since "7d"

# Check cross-project links
weft kernel services  # Look for Mesh service status
```
