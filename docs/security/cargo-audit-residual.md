# cargo audit residual ignores

Ignores live in `CARGO_AUDIT_IGNORES` in `scripts/build.sh` (gate check 12).

## wasmtime 45.0.3 (2026-09-30)

Patched releases are 36.0.16, 48.0.3 and 49.0.1+. The 48/49 lines need Rust 1.95/1.96,
while the repo pins Rust 1.93 (`rust-toolchain.toml`, `rust-version`). Moving off 45 means a
toolchain bump plus a wasmtime major bump, tracked separately.

| Advisory | Why not reachable | Expires |
|----------|-------------------|---------|
| RUSTSEC-2026-0314 (wasmtime-wasi, FS datetime overflow panic) | The only WASI context (`clawft-kernel` `wasm_runner`) has no FS preopens, so guests have no filesystem to call into | 2026-12-31 |
| RUSTSEC-2026-0316 (dynamic record lifting exceeds hostcall fuel limit) | Component-model only, low severity. wasmtime is built with `cranelift, async, wat, runtime` and no `component-model` feature; only core modules load | 2026-12-31 |
| RUSTSEC-2026-0222, RUSTSEC-2026-0269 | Already ignored before this change (0269: no FS preopens) | see `scripts/build.sh` |
