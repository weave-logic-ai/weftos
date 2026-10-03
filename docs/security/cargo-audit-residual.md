# cargo audit residual ignores

Ignores live in `CARGO_AUDIT_IGNORES` in `scripts/build.sh` (gate check 12).

## wasmtime: no residual ignores

The wasmtime advisories RUSTSEC-2026-0222, -0269, -0314 and -0316 were previously ignored
with reachability arguments while the repo was on wasmtime 45.0.3 and Rust 1.93. They are
cleared: `rust-toolchain.toml` pins Rust 1.95 and `Cargo.lock` resolves wasmtime and
wasmtime-wasi to 48.0.5 (patched line 48.0.3+). The four ignores, their expiry dates
(`CARGO_AUDIT_EXPIRIES`) and the grep guard that backed them (`audit_guard_wasmtime_usage`)
were removed, so `scripts/build.sh audit` now fails if any of these advisories reappears.

The remaining ignores (paste, bincode, quick-xml, failure, event-listener, lexical-core) are
unmaintained or transitive items documented inline in `scripts/build.sh`.
