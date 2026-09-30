# Handoff — Urth spatial and copper sonobuoy research — 2026-09-26

> **Topic handoff, moved from `docs/handoff.md` on 2026-09-28.** Its files are now **committed** on
> `0.8-metaharness` (research in `52baf263`, docs-site pages in `c1a1247c`), so the "untracked / dirty"
> notes below are historical. The measurements, dead ends and open threads still stand.

WeftOS is a Rust agent OS (BVH + HNSW + chain) with a sparse planetary twin (**Urth**; rename candidate **uvRTH**). This session was **research + docs**, not hardware or crate work: 2026 spatial-intelligence survey, a `/urth-spatial` scrollyteller, rUv crosswalk, and a **copper-only** towed-array / 3-buoy plan (no Wi‑Fi). Nothing was committed; nothing was flashed.

## Current state

- Branch `0.8-metaharness` @ `a6ab662e`, **dirty** — large pre-existing 0.8-metaharness tree **plus** this session’s untracked research/docs. Do not treat `git status` as one ticket.
- **This session’s files (untracked unless noted):**
  - `docs/research/spatial-intelligence-2026/` (survey + scrolly storyboard)
  - `docs/research/copper-bus-timing-grounding/` (papers + range + braid)
  - `docs/research/sonobuoy-min-test-and-copper-tow.md`
  - `docs/research/pzsdr-p047-and-fiber-towline.md`
  - `docs/research/uvrth-name-candidate.md`, `docs/research/uvrth-tessellation-scales.md`
  - `docs/src/app/urth-spatial/`, `docs/src/public/urth-spatial/`, `docs/src/content/docs/weftos/research/`, `docs/src/content/docs/weftos/vision/urth.mdx`
  - **Tracked edit:** `docs/adr/adr-079-urth-digital-twin.md` (uvRTH callout + tessellation pointer only)
- Docs Next on `:4010` was used for scrolly verify; **dead** (runtime cap). Forge `:3000`/`:3333` — do not touch.
- `scripts/build.sh` **not** run for this work. No cargo/test claim.
- Never commit to `master`. Ruflo MCP is still project `[mcp_servers.ruflo]` (`ruflo__*`).

## What's working (verified)

| Thing | State | Verified how |
|---|---|---|
| `/urth-spatial` scrolly | Renders, cards expand, desktop + 390px | Playwright vs `npx next dev --port 4010` this session (server now down) |
| `/docs/weftos/vision/urth`, `/docs/weftos/research/spatial-intelligence` | HTTP 200 then | same |
| SVGs | well-formed | `xmllint --noout` on storyboard diagrams |
| Spatial Plane tickets | all Done (A–F BVH); **no open spatial items** | prior inventory + this session’s board read |
| Copper/sonar plan | **on paper only** | literature agents + Urick/P1 numbers; **no lake, no hose, no TWAI firmware** |

## Done this session

- **Spatial index story is closed as a decision:** BVH geometry, HNSW features, optional VectorRef, Graph Views F9 promote. Residuals (live BVH publish, `spatial_rpc`, Urth E1/E2) are docs, not Plane.
- **2026 survey** (Marble/Atlas, Genie/Cosmos/WBench, VGGT/SpatialLM, HOV-SG, CityGaussian/Octree-GS, V-JEPA) in `docs/research/spatial-intelligence-2026/`.
- **rUv two-way** in `ruv-parallels-and-gaps.md` (learn SuperSplat+provenance; offer BVH sibling + DiskANN bench).
- **Copper-only sonobuoy/tow (user constraint: no Wi‑Fi):** power + CAN/TWAI + RS-422 PPS on CAT5/7; USB-CAN gateway to WeftOS; ESP32 radio **off**.
- **Min test:** 3 Class A, lake **15–30 m**; copper-first 4 phones @ **0.40–0.45 m**.
- **Twin CAT5 in one Dyneema/Kevlar 4-plait** (parallel, not helix; loop at tail; not IBM Token Ring).
- **Range numbers written** (were missing until the user called it): see Measurements.
- **uvRTH** = Region Tessellated Holograph + rUv+Earth letter-play. Tessellation ranks nano→…→inf mapped to L0–L5. **ADR-079 still named Urth.**

## Measurements & calibration

*Paper budgets, not tank data. Do not re-derive; do measure the disc SL before quoting active range at sea.*

| Quantity | Value | Source |
|----------|-------|--------|
| TDoA clock budget | **~10 µs** ≈ 1.5 cm in water | architecture / RANGING |
| CAT5 delay | **~5 ns/m** (VF≈0.66); 30 m ≈ **159 ns** ≈ 0.24 mm sound | cal, then subtract |
| Software CAN time (Luckinger TII 2022) | **~50 µs** | **fails** 10 µs → PPS pair required |
| GPS SPS | **2–5 m** horiz | RANGING.md — **not** coherent BF geometry |
| 1.8 kHz λ | **0.82 m**; λ/10 = **8 cm** | c=1480 m/s |
| 4-el hose L=1.2 m HPBW | **~35°** at 1.8 kHz; array gain **+6 dB** | λ/L |
| Vocal school SL=120 dB, DI=0, SE=0 | **~1 km** | phase-economics §5.1 / Mode F |
| Same, 4-el DI≈6 dB | **~2 km** detect | scale 10^(DI/20) |
| 20-el / 100 m tow DI≈23 dB | **~14 km** detect | same notes |
| Triangle **(x,y)** | **~50–150 m** useful (few× 30 m baseline); 0.1–1 m in-frame if loud/direct | geometry, not clock |
| Cheap 35 mm **active** echo | **~50–200 m** if SL 130–150 dB | **not** JANUS 185–195 dB |
| Mesh BPF | **~300 Hz @ 1.8 kHz** | ranging hole, not PAM taxonomy |
| α(1.8 kHz) | **~0.06 dB/km** | spreading-limited until tens of km |
| Twin `d⊥` tight plait | **~1–1.5 cm** (~5° phase @ 1.8 kHz) | L/R weak until 8–12 cm spacers |

## Dead ends — do not retry

- **Wi‑Fi as backhaul or time base** — user: copper **replaces** Wi‑Fi. Radio off (also jittered RX ADC).
- **CAN SOF / CanTSyn software as PPS** — AUTOSAR exists because arbitration wrecks “now”; ~50 µs software vs 10 µs budget.
- **LoRa on a CAT5 pair as the bus** — CSS-as-PLC is seconds-scale, not PPS, not PCM. Optional later radio only.
- **IBM Token Ring PHYs** — tail loop = failover + 2L/v_prop length; token = CAN poll.
- **Helix two CAT5s / nylon as acoustic-section load** — helix kills L/R; nylon stretch walks geometry. Dyn/Kev load, nylon spacer/VIM only.
- **Two tows / paravanes for v1 L/R** — L/R is **inside one 4-plait**. Twinline is later.
- **Aiming piezos L/R at 1.8 kHz** — omni; λ ≫ jacket. Spatial `d⊥` is the trick.
- **SPS GPS as coherent array geometry** — 2–5 m ≫ 8 cm. GPS georeferences; hose/taut/RTK is `D_ij`.
- **Passive 2 km mahi** — 120 dB is **vocal chorus** (Ramcharitar), not mahi. Mahi = active/resonance cue.
- **Dolphin clicks on mesh BPF** — 20–150 kHz; detection range **0** until HF RX.
- **35 kHz imaging on 0.4 m spacing** — ~10 λ, grating lobes.
- **35 mm disc = 185 dB JANUS projector** — different transducer. Measure SL.
- **P047 RFSoC as buoy ADC or hydrophone sampler** — 1 MHz floor, $8.7k, dry-end/USV only. Fiber later.
- **I²C over the hose** — Class B 1–3 m only.
- **Merge WorldGraph into BVH / CSI occupancy as geometry SoT** — complementary stacks.
- **Mass-rename Urth → uvRTH** — candidate only until product says go.
- **Re-bake spatial index (R-tree, kd-tree)** — ADR-056 stands.
- **Docs `:4010` still up** — it died. Restart if you need the scrolly.
- **Assume Plane has spatial tickets** — empty. Residuals unfiled.

## Open threads

1. **First physical:** 3× Class A, pool ~8 m then lake **15–30 m** (events over **USB-CAN**, radio off). Done = `AcousticEvent` on shore with PPS capture. **Not filed on Plane.**
2. **4-phone copper hose** 8–15 m, 0.4 m taps, optional twin CAT5 4-plait. Done = dry ping then bucket.
3. **Measure 35 mm TX SL** before any open-ocean active range claim.
4. **Optional Plane:** live BVH publish, `spatial_rpc` e2e, VGGT/SpatialLM adapters, DiskANN WEFT-660/661 upstream.
5. **uvRTH:** freeze name or leave Urth; tessellation ranks not on the wire yet.
6. **Wide RX (0.5–8 kHz)** before species/PAM taxonomy.
7. **Vercel deploy** of `/urth-spatial` not done.

## Resume here

```bash
cd /Users/mathewbeane/weftos
git branch --show-current   # 0.8-metaharness
# doctrine, newest first
less docs/research/copper-bus-timing-grounding/README.md
less docs/research/copper-bus-timing-grounding/sonar-range.md
less docs/research/sonobuoy-min-test-and-copper-tow.md
less docs/research/uvrth-name-candidate.md
# scrolly (optional)
cd docs/src && npx next dev --port 4010 --hostname 127.0.0.1
# open http://127.0.0.1:4010/urth-spatial
```

Do **not** start with W5500, LoRa-on-copper, P047, or fiber. Next build is **CAN + PPS + 12 V on CAT5**, radio off, USB-CAN into WeftOS.

## Key paths

- `docs/handoff.md` — this file
- `docs/research/copper-bus-timing-grounding/` — **start here** for sonar/copper
- `docs/research/sonobuoy-min-test-and-copper-tow.md` — 3 buoys, CAT5 pair map, CAN/PPS
- `docs/research/spatial-intelligence-2026/` — 2026 papers + STORYBOARD
- `docs/src/app/urth-spatial/` — scrollyteller
- `docs/adr/adr-079-urth-digital-twin.md` — Urth; uvRTH callout
- `.planning/sonobuoy/` — original build/RANGING (Wi‑Fi/TWTT in architecture.md is what copper **replaces**)
- Ruflo patterns: `pattern-copper-bus-no-wifi-pps-can`, `pattern-spatial-intelligence-2026-urth-survey`, `pattern-ruv-urth-parallels-gaps-2026-09-21`

## Gotchas

- **architecture.md still says Wi‑Fi gossip + TWTT.** User overrode: copper is the network and the clock. Don’t “fix” the plan back to Wi‑Fi.
- Call MCP `ruflo__*`, never `claude-flow__*`. Lead still `team_on_stop` until matched idle is proven (prior handoff).
- Don’t take down Forge `:3000`/`:3333`.
- Dirty tree is mixed 0.8 work + this research — commit only what the user names.
- `ruvnet-brain` search failed once this session (`tokenizer.json` missing under `local_files_only`). Grounding for rUv crosswalk was earlier in the session when it worked.
