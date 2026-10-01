# cargo audit residual ignores

Ignores live in `CARGO_AUDIT_IGNORES` in `scripts/build.sh` (gate check 12).

## wasmtime 45.0.3 (2026-09-30)

Patched releases are 36.0.16, 48.0.3 and 49.0.1+. The 48/49 lines need Rust 1.95/1.96,
while the repo pins Rust 1.93 (`rust-toolchain.toml`, `rust-version`). Moving off 45 means a
toolchain bump plus a wasmtime major bump, tracked separately.

| Advisory | Why not reachable | Expires |
|----------|-------------------|---------|
| RUSTSEC-2026-0314 (wasmtime-wasi, FS datetime overflow panic) | The only WASI context (`clawft-kernel` `wasm_runner`) has no FS preopens, so guests have no filesystem to call into | 2026-12-31 |
| RUSTSEC-2026-0316 (dynamic record lifting exceeds hostcall fuel limit, low severity) | The `component-model` feature IS enabled, through feature unification via wasmtime-wasi p2 (clawft-kernel `wasm-sandbox`). The ignore holds because no code instantiates a component: only core `Module::new` (kernel `wasm_runner/runner.rs`, `clawft-wasm-host/src/engine.rs`) and `p1::add_to_linker_async` are used. Re-check if anyone adds `Component::`, `bindgen!` or a p2/component host | 2026-12-31 |
| RUSTSEC-2026-0269 (FS sandbox escape via trailing slashes) | No FS preopens in the only WASI context | 2026-12-31 |
| RUSTSEC-2026-0222 (type indices mixed between engines) | Needs two engines sharing a store; the kernel uses one engine per runner. Not fully audited | 2026-12-31 |

All four are tracked under ticket `toolchain-wasmtime-bump`.

## Enforcement

`scripts/build.sh audit` fails when (a) any date in `CARGO_AUDIT_EXPIRIES` has passed, or
(b) the source under `crates/` contains `wasmtime::component`, `Component::new`, `bindgen!`,
`preopened_dir` or `.preopen(` (a grep guard, `audit_guard_wasmtime_usage`).
