# ADR-054: claude-flow integration — user-installed, not first-party

- **Status**: Accepted (2026-04-28)
- **Closes**: WEFT-488
- **Related**: ADR-052 (toolregistry split), `docs/guides/mcp.md` (`weft mcp add`)

## Context

`claude-flow` (`@claude-flow/cli`) is a TypeScript orchestration layer
distributed via npm. It exposes an MCP server that surfaces a large
set of swarm-coordination, memory, hook, and routing tools. Several
agents (and the project `CLAUDE.md`) reference it via the MCP install
path:

```bash
claude mcp add claude-flow -- npx -y @claude-flow/cli@latest
```

A live question through M6 has been: should WeftOS *ship* claude-flow
as part of its own release artefact (vendoring the npm dist, exposing
the same tools first-party through `clawft-services::mcp`), or treat
it as a third-party MCP server the user installs themselves?

## Decision

**`claude-flow` stays user-installed.** WeftOS does not vendor or
ship `@claude-flow/cli`. It is a standard third-party MCP server,
installed by the user via `claude mcp add` (Claude Desktop) or
`weft mcp add` (WeftOS daemon — see `docs/guides/mcp.md`). The
WeftOS release artefact does not bundle Node, npm, or the claude-flow
package.

## Rationale

1. **Release-surface containment.** Pulling claude-flow into the
   WeftOS distribution would add a Node.js / npm dependency to a
   binary tree that today is pure Rust + WASM. The release matrix
   (cargo-dist, GitHub Releases, crates.io, Homebrew) would have to
   grow Node provisioning, npm version pinning, and a parallel
   security audit. The WeftOS-side cost of that is permanent; the
   user-side cost of `npx -y @claude-flow/cli@latest` is one line.

2. **Versioning independence.** claude-flow ships continuously on its
   own cadence (multiple versions per week during active development).
   A first-party shipment would either lag the upstream (frustrating
   users) or force WeftOS releases on every claude-flow bump
   (frustrating us).

3. **MCP is the integration contract.** `clawft-services::mcp`
   already speaks MCP as a client (`McpSession::connect`) and as a
   server (`weft mcp-server`). Treating claude-flow as just another
   MCP endpoint reuses that path. The protocol-version handshake
   (WEFT-489) and the `tools/list` allowlist (WEFT-189 / WEFT-480)
   apply uniformly, regardless of whether the peer MCP server is
   first-party or user-installed.

4. **Trust model.** A user who installs claude-flow has consented to
   that supply chain (npm registry, the `@claude-flow/cli` package,
   its transitive deps). Vendoring it would shift that consent onto
   the WeftOS release, which we don't want to silently take on for
   users who never asked for it.

5. **`CLAUDE.md` already documents it.** The project's
   `CLAUDE.md` shows the canonical install (`claude mcp add
   claude-flow ...`) plus the supported subcommands. That stays the
   primary integration doc; `docs/guides/mcp.md` adds the WeftOS-
   daemon equivalent (`weft mcp add`) for users running on the
   daemon side.

## Implications

- No new code in `crates/` for this decision. The `weft mcp add`
  surface for installing the MCP server entry into `weave.toml` is
  documented in `docs/guides/mcp.md` (WEFT-492).
- The release pipeline (`.github/workflows/release*.yml`,
  `cargo-dist.toml`) does not gain a Node.js / npm step.
- Documentation: `CLAUDE.md` keeps the user-install snippet;
  `docs/guides/mcp.md` documents the `weft mcp add` install path
  with a concrete `weave.toml` excerpt.
- If a future release wants to first-party a *subset* of the
  claude-flow surface (e.g. its hooks or memory backend), that is a
  new ADR — this one only covers the integration model for the
  current shape.

## Followups

None — this is a closure ADR. WEFT-492 covers the user-facing
documentation; WEFT-489 covers the protocol-version handshake that
applies to claude-flow as one specific MCP peer.
