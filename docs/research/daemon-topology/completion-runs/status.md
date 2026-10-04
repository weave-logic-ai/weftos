# Completion status

## Verified

- Baseline e6662b818: gate 22/22, 12,300 tests passed (27 individual skips).
- Combined container, Seatbelt, nested and leaf native source: scoped check passed after import, combined-check-r2.log.
- Earlier Seatbelt host boundary tests: 4/4 and real lifecycle RPC test 1/1. These predate the final combination.
- Reviewed container scoped tests: 12/12, prior to source integration.
- ESP-IDF release firmware snapshot r4 compiled; all 50 snapshot files matched author source. No hardware was flashed.

## In progress

- Combined native run finished: 5,356 passed, 10 failed, 7 skipped (combined-tests-r1.log). Confirmed fixes and fixture corrections are integrated; focused low-concurrency rerun is active (focused-tests-r2.log).
- D10 review corrections integrated: recovered-process shutdown checks, reciprocal outbound admission, repeated-start policy handling, and bounded graceful shutdown through the owned liveness pipe. Signed-leaf and reciprocal-authentication merge conflicts resolved preserving both paths; independent review underway.
- Wasmtime persistent guest/runner: source implementation, review corrections and launcher integration; compilation and actual guest lifecycle not yet verified.
- Actual container lifecycle harness; existing engine version smoke does not establish lifecycle acceptance.

## Still required before completion

Run final combined runtime tests, rebuild and exercise real child-only daemon endpoint, validate nested and container lifecycle on their real boundaries, validate Wasmtime guest and runner, then final quality gate and phase review. Leaf hardware/power-loss behavior and legacy bare-metal migration are not established by the IDF build. No completion commit or push has been made for this integration.
