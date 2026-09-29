# Live Seed run: fall-detect through the Seed API adapter (2026-09-29)

Test: `live_seed_fall_detect` (crates/clawft-kernel/src/workload_runtime/tests_live.rs), run once by the operator with
`COGNITUM_SEED_LIVE=1 COGNITUM_SEED_LIVE_INSTALL=1`, over the tailnet `https://` base with the Seed's certificate pin
(`COGNITUM_SEED_CERT_SHA256`). The Seed is a Cognitum Seed, Pi Zero 2 W, firmware 0.24.2, paired, with fall-detect 1.0.0 pinned.

Result: **passed** (1 test, 17.7 s).

- Console cycle: exit 0, 7403 ms. The adapter stopped the running fall-detect instance before the console run
  (`stopped=["fall-detect"]`, because of UDP 5006 contention). Output:
  `{"status":"quiet","fall_detected":false,"confidence":0.0,"z_impact":0.0,"stillness_pct":0.0,"total_falls":0}`.
- Chain (isolated, in-memory): `workload.load, workload.start, workload.stop, workload.start, workload.start,
  workload.stop, workload.unload`.
- Afterwards, the Seed reported `fall-detect` and `baby-cry` both installed and not running.
- The operator chain (`~/.clawft/chain.rvf`) mtime didn't change.

Known gap: the same test over the plain-http USB base (`http://169.254.42.1`) fails in the adapter's client with
"error sending request", even though curl reaches the Seed. This is tracked as a follow-up.
