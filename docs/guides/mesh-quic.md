# Mesh QUIC Transport (WEFT-118 / ADR-026)

WeftOS mesh supports **QUIC** via [`quinn`](https://docs.rs/quinn) as the
primary Cloud/Edge transport (ADR-026), alongside TCP and WebSocket.
Application encryption and node identity remain **Noise** (`snow`,
ADR-024) on top of the bidirectional stream — quinn's TLS is only used
to complete the QUIC handshake (ephemeral self-signed cert +
skip-verify on the client).

| Transport | Feature | Address scheme | Typical use |
|-----------|---------|----------------|-------------|
| **QUIC** | `quic` (default with kernel) | `quic://host:port` | Cloud / Edge, multiplexed, UDP |
| TCP | `mesh` | `host:port` or `tcp://…` | Dev, corporate UDP-blocked nets |
| WebSocket | `mesh` | `ws://…` / `wss://…` | Browser / WASI / HTTP proxies |

## Config

```toml
[kernel.mesh]
enabled = true
transport = "quic"                 # "tcp" | "ws" | "quic"
listen_addr = "0.0.0.0:9489"       # UDP bind for QUIC
noise = true                       # Noise XX (recommended production)
# noise_key_path = "/etc/weftos/mesh.key"
seed_peers = ["quic://10.0.0.2:9489"]
discovery = false
```

JSON equivalent (`~/.clawft/config.json`):

```json
{
  "kernel": {
    "mesh": {
      "enabled": true,
      "transport": "quic",
      "listen_addr": "0.0.0.0:9489",
      "noise": true,
      "seed_peers": ["quic://10.0.0.2:9489"]
    }
  }
}
```

### Fields

| Field | Type | Default | Notes |
|-------|------|---------|-------|
| `enabled` | bool | `false` | Start mesh listener at boot (phase 5d). |
| `transport` | string | `"tcp"` | `"tcp"`, `"ws"` / `"websocket"`, or `"quic"`. |
| `listen_addr` | string | `127.0.0.1:9489` | Bind address. For QUIC this is a **UDP** port. |
| `noise` | bool | `false` | Wrap every peer stream in Noise XX. |
| `noise_key_path` | string? | — | 32-byte Ed25519/X25519 private key file. |
| `seed_peers` | string[] | `[]` | Peer URLs to dial after listen. Prefer `quic://` when `transport = "quic"`. |
| `discovery` | bool | `false` | Kademlia DHT peer discovery. |

## Build

Default `clawft-kernel` builds enable `mesh` + `quic`:

```bash
scripts/build.sh native-debug
# or
scripts/build.sh check
```

Opt out of the quinn dependency tree:

```bash
cargo check -p clawft-kernel --no-default-features --features native,mesh
```

If `transport = "quic"` is set but the binary was built without the
`quic` feature, boot logs an error and falls back to TCP.

## Runtime behaviour

1. Boot phase 5d reads `[kernel.mesh]`.
2. `transport = "quic"` selects `QuicTransport` (`mesh_quic.rs`).
3. Listener binds UDP, mints a throwaway self-signed TLS cert for SNI
   `localhost`, ALPN `weftos-mesh`.
4. Client dials with the same ALPN and **does not** verify the server
   certificate (Noise authenticates the peer).
5. Each accepted/connected session opens one bidirectional QUIC stream;
   frames are **4-byte big-endian length-prefixed** (same as TCP/WS).
6. Optional Noise XX wraps that stream (`mesh_noise::NoiseChannel`).

Higher layers (handshake, IPC, chain sync, assessment) are
transport-agnostic via `MeshTransport` / `MeshStream`.

## Firewall / ops

- Open **UDP** on the mesh port (default `9489/udp`).
- Some corporate networks block UDP; fall back with
  `transport = "tcp"` (or WebSocket for browser edges).
- QUIC multipath / connection migration benefits Edge nodes that change
  networks without re-running Noise when the path is stable enough to
  keep the connection; full re-handshake still applies on hard loss.

## Tests

```bash
# Unit + two-node handshake + Noise-over-QUIC
cargo test -p clawft-kernel --features quic mesh_quic -- --nocapture

# Or via the project build wrapper
scripts/build.sh test -p clawft-kernel
```

Key tests in `crates/clawft-kernel/src/mesh_quic.rs`:

- `quic_transport_connect_send_recv` — raw framed bytes
- `two_node_cluster_forms_over_quic` — `WeftHandshake` exchange
- `two_node_quic_with_noise` — quinn + snow Noise XX
- `quic_recv_survives_select_cancellation` — WEFT-18 cancel-safety

## See also

- [ADR-026: QUIC primary transport](../adr/adr-026-quic-primary-transport.md)
- [ADR-024: Noise Protocol](../adr/adr-024-noise-protocol.md) (if present)
- [Kernel guide — Mesh](./kernel.md#mesh-k6)
- [Configuration guide](./configuration.md)
- Source: `crates/clawft-kernel/src/mesh_quic.rs`, `mesh_tcp.rs`, `mesh_ws.rs`, `mesh_noise.rs`
