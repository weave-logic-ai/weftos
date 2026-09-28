# Bioacoustics on the copper tow — mahi baseline, whales, time, Urth / RuVector

**Date:** 2026-09-26  
**Status:** Planning estimate  
**Hardware:** twin-CAT5 4-el (or 8-el) 1.2 m hose, 1.8 kHz mesh ~300 Hz BPF, wired PPS; 3-buoy taut triangle optional.  
**Companions:** `tow-detect-classify-resolution.md`, `gps-imu-fusion-accuracy.md`, ADR-079 (Urth).

> **Naming (not a rename):** consider **uvRTH** as a product mark for the twin, with **RTH** TBD. Working expansions below. Canonical name in ADRs remains **Urth** until a decision.

---

## 1. The mahi-school baseline (what that physics actually is)

Open-ocean **mahi-mahi (*Coryphaena hippurus*)** are not a loud vocal species. The useful acoustic handle on a **school** is **swimbladder / aggregation resonance and backscatter**, not a whistle:

- Holliday 1972: schooled fish echoes show structure **200 Hz–5 kHz** (anchovy / mackerel / rockfish sampled). That band **includes our 1.8 kHz mesh**.
- Tuna/FAD work is mostly **38 / 120 / 200 kHz echosounder**, not 1.8 kHz passive.
- Purse-seine sonars find schools at **20–30 kHz**, 1.5–2 km, ~5–7° beams — a different instrument class than a 1.2 m JFET hose.

So the “mahi school baseline” for **this** stack is: **can a 1.8 kHz chirp (or a loud swimbladder chorus) light up a school as a blob in a ~35° sector**, then **track that blob over time** in the twin. It is **not** “ID a dorado at 2 km, 1°.”

A **wide** second RX (or P79-class 35–200 kHz) is what starts to look like fisheries sonar. The mesh hose is the **cueing** layer.

---

## 2. Loud animals vs our bands

| Source | Typical band | 1.8 kHz mesh (300 Hz) | Wide RX 0.5–8 kHz | Twin hose L/R | Notes |
|--------|----------------|------------------------|-------------------|---------------|--------|
| **Humpback / most mysticete song** | 20 Hz–8 kHz | **Yes** (in-band energy) | **Yes** | coarse | Best PAM target for v1 |
| **Fin / blue** | 10–100 Hz | edge / no | if ADC DC-ish | n/a | Need a low-f path |
| **Sperm click** | ~2–25 kHz, ~10 kHz peak | **partial** (LF tail) | **yes** | yes | Clicks want ≥20 kS/s |
| **Dolphin whistle** | ~2–20 kHz | **no** (BPF) | **yes** | yes | |
| **Dolphin / porpoise click** | 20–150 kHz | **no** | **no** | — | Needs P79 / 200 kHz path |
| **Reef fish chorus / grunts** | 100–1000 Hz peak | **yes** if 1.8 kHz tail | **yes** | weak | |
| **Mahi / tuna *school*** | resonance ~0.3–3 kHz; TS at 38 kHz+ | **cue** (active chirp or dense school) | better | blob | Classify school ≠ classify species |
| **Boat / outboard** | broadband + lines | **yes** | **yes** | yes | Easiest non-bio class |

**Detect (v1 hose + chirp):** mysticete song, nearby sperm LF, boats, **maybe** a dense swimbladder school as extra reverberation in the 1.8 kHz hole. **Not:** odontocete clicks, a lone mahi, a quiet swimmer.

---

## 3. Resolving *over time* (fusion, RuVector, the twin)

A single ping is a fat sector. The stack we already have is built to **integrate**:

| Layer | What accumulates | Over minutes–hours |
|-------|------------------|--------------------|
| **Wired PPS + tow** | coherent 1.8 kHz snapshots, L/R bit | SAS-lite: as the USV moves, the 35° beam **sweeps**; a school becomes a **track**, not one blob |
| **3-buoy TDoA** | 0.1–1 m in-frame fixes when SNR is high | filter → **smoothed track** (√N if independent looks) |
| **SPS GPS** | 2–5 m georef of the figure | enough to drop a **region** on the twin, not a fish |
| **RuVector / HNSW** (`VectorRef` on Event leaves, ADR-088) | embedding of each snippet (band, flux, duration, L/R, optional spectrogram) | “heard **this call family here** before”; nearest-neighbour to Watkins/Perch-style exemplars **once the wide RX exists** |
| **Graph Views** | bind spatial (BVH) + temporal (chain) + feature (HNSW) | track = a **path of Event leaves**, promote F9 when stable |
| **Twin (Urth / uvRTH)** | sparse L4 quilt: unobserved stays empty | school = **densifying region** + confidence, never a fake school in empty ocean |

**What “resolved over time” means in practice**

- **Song whale, hours:** you get a **bearing history** (35° bins, L/R if twin) georeferenced to 2–5 m for the *array*, animal position only if the 3-buoy net or a long tow baseline is in play. Over a night, RuVector clusters the call type; the twin shows a **corridor**, not a GPS tag on the animal.
- **Dolphin group, minutes:** with **wide RX**, click trains / whistles become a moving energy centroid in the beam. Still not 1° tracks on the 1.2 m hose.
- **Mahi school, one pass:** active 1.8 kHz chirp → extra return in a sector; over a **lawnmower** USV path the twin paints a **patch** whose size is beamwidth × range (at 200 m, 35° is ~120 m across — a school-sized *cell*, not a biomass estimate).
- **Repeat visit, days:** HNSW + chain: same patch, similar embedding → “this aggregation persists.” That is the Urth sparse-first story.

Do not claim ICES-style school biomass. That needs calibrated 38 kHz TS and a real echosounder beam.

---

## 4. Naming note — **uvRTH** (candidate, not decided)

ADR-079 still names the twin **Urth**. Optional product mark: **uvRTH**.

**uv** — keep lowercase: weft/UV (two-band sight: metric BVH + appearance), or “micro-verse.” Not a second planet.

**RTH** — pick one and freeze later:

| Expansion | Why it fits |
|-----------|-------------|
| **Region Tessellated Holograph** | **Leading.** Region = E1 / sharded BVH / per-LOD views. Tessellated = sparse quilt. Holograph = metric + appearance reconstructable from observations. |
| **Realtime Topological Holograph** | Topology = BVH + Graph Views; “realtime” is true of Views, less of the persistent twin. |
| **Residual Topological Hologram** | Honest: we store residuals, not a complete Earth. |
| **Region-Temporal Holograph** | Regions + Event time; weaker on the quilt. |

Copy trial: **uvRTH** — Region Tessellated Holograph. Spoken “you-vee-arth.” Do **not** search-replace ADR-079 until product agrees.

---

## 5. One line

v1 copper tow **cues** loud, in-band life (songs, boats, maybe a resonant school) as **tracks and patches** in RuVector + the twin. It does **not** classify mahi or resolve a dolphin click. Wide RX and/or 38 kHz-class active are the next instruments; fusion is how a 35° beam becomes a corridor over time.
