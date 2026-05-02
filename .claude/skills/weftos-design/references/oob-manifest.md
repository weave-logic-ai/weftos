# OOB stock-desktop manifest

Mirrors `docs/DESIGN.md` §9. The 12 surfaces below ship installed on
the OOB desktop. Each must render its empty/loading/offline state per
`docs/DESIGN.md` §5 with no substrate data and no installed apps.

| id | archetype | substrate roots | TOML fixture | status |
|---|---|---|---|---|
| `(built-in) Kernel` | chip-detail | `substrate/kernel/status` | `clawft-surface/fixtures/weftos-chip-kernel.toml` | ✅ shipped |
| `(built-in) Mesh` | chip-detail | `substrate/mesh/*` | `clawft-surface/fixtures/weftos-chip-mesh.toml` | ✅ shipped |
| `(built-in) ExoChain` | chip-detail | `substrate/exochain/*` | `clawft-surface/fixtures/weftos-chip-exochain.toml` | ✅ shipped |
| `(built-in) Explorer` | list-detail | `substrate/` | native (`explorer/`) | ✅ shipped (graduate to TOML in 0.9.x) |
| `app://weftos.files` | app-window | `substrate/fs/*`, `derived/*` | TBD | ❌ todo |
| `app://weftos.processes` | app-window | `substrate/kernel/processes` | TBD | ❌ todo (viewer exists) |
| `app://weftos.services` | app-window | `substrate/kernel/services` | TBD | ❌ todo (data path exists) |
| `app://weftos.network` | app-window | `substrate/{mesh,wifi,bluetooth}/*` | TBD | ❌ todo (chip TOMLs partial) |
| `app://weftos.settings` | app-window | `substrate/config/*` | TBD | ❌ todo |
| `app://weftos.scheduler` | app-window | `substrate/scheduler/*` | TBD | ❌ todo |
| `app://weftos.monitor` | tile-grid + plots | `substrate/kernel/*`, `substrate/sensors/*` | TBD | ❌ todo (kernel chip exists) |
| `app://weftos.logs` | stream | `derived/logs/*` | TBD | ❌ todo |
| `app://weftos.terminal` | stream (native) | n/a | native (`explorer/terminal.rs`) | ✅ shipped |
| `app://weftos.chat` | stream (native) | `derived/chat/*` | native (`explorer/chat.rs`) | ✅ shipped |
| `app://weftos.admin` | app-window | `substrate/kernel/*` | `clawft-app/fixtures/weftos-admin.toml` | ✅ shipped (reference app) |
| `app://weftos.launcher` | tile-grid | (browses installed apps) | TBD | ❌ todo |

## Status legend

- ✅ shipped — surface and code path live, audit passes.
- 🔶 partial — data path exists but no first-class app TOML.
- ❌ todo — needs scaffold + fill.
