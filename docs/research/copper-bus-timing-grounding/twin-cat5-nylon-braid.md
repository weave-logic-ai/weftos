# Twin CAT5 + nylon braid — geometry, loop, “token ring”

**Date:** 2026-09-25  
**Status:** Design note (not a hose spec)  
**Stack:** copper-only, no Wi‑Fi (`sonobuoy-min-test-and-copper-tow.md`)

Two Ethernets in a braid is not “more bandwidth.” It is a **twin-line aperture** (left/right) plus a **physical loop** (return path, delay cal, failover). SURTASS-class Twinline exists for the same acoustic reason.

**One tow, not two.** Left/right lives **inside the plait** (two CAT5s trapped parallel). A second tow with paravanes is Navy Twinline at metre-scale `d⊥` — not v1. Piezos are still **omni**; “facing” L/R is the **baseline**, not aiming the disc. At 1.8 kHz, λ ≈ 82 cm ≫ hose diameter, so a baffle on the jacket does almost nothing. Spatial `d⊥` is the whole trick.

---

## 1. What the second strand is for

A single line of omni phones is **cylindrically symmetric** — you cannot tell port from starboard without a turn or a second line (APL Twinline; constrained-BF papers).

Hold **two CAT5s parallel** at a **fixed** across-track gap `d⊥`:

| `d⊥` | Max phase at 1.8 kHz (λ ≈ 0.82 m) | Role |
|------|-------------------------------------|------|
| 5 cm | ~22° | Weak L/R, easy to braid |
| **8–12 cm** | ~35–52° | **Sweet spot** for a fat rope |
| 41 cm (λ/2) | 180° | Best L/R, awkward diameter |

Phones (or pods) on **both** strands, same station along-track. Sum/diff or a 2-D steering vector breaks the left/right flip. Along-track spacing stays **0.40–0.45 m** on each line.

Do **not** twist the two CAT5s around each other. A helix rotating the baseline kills L/R. They must stay **parallel**, like a zip-cord, with the braid as a **cage**, not a candy-cane.

---

## 2. Nylon, but not as the load

Nylon is a **spring**. Wet nylon: large elastic stretch, creep, ~15% strength loss. At break, double-braid nylon is ~40% elongation; aramid ~6%. Array geometry and pair length cannot ride that.

| Member | Job | Example |
|--------|-----|---------|
| **Load** | Tension, low stretch, torque-balanced | 2–4 × Dyneema/Kevlar, **contrahelical** (S + Z) |
| **Spacer** | Set `d⊥`, crush protection | **Nylon** (or PE) of **two diameters**: fat cores in the middle, thin fillers in the valleys |
| **Jacket / fairing** | Abrasion, strum | Nylon or polyester braid / fuzzy fairing |
| **VIM** | Stretch *on purpose* between USV and acoustic section | Nylon snubber **only here**, not in the acoustic section |

Historic hose arrays already used **intertwined nylon + Kevlar** inside PU (DTNSRDC thin-line tests): Kevlar takes load, nylon fills. Same split.

Different nylon sizes are the **mandrel**: two fat 6–8 mm nylons as rails, CAT5 laid in the grooves, thin 3 mm fillers, then an 8- or 12-plait cover. Target finished OD ~20–30 mm for an 8–12 cm `d⊥` you probably need a **ladder**: periodic rigid/semirigid spacers (3-D printed clips every 0.4 m) *plus* the braid, or you will not hold 8 cm under tow. A tight round braid alone wants to go circular and **collapse `d⊥`**.

v1 (lake, 10 m): two CAT5 + two Dyneema + zip-ties/spacers every 0.4 m, nylon serving over the top. Fancy 8-plait later.

### 2.1 Flat 4-strand: trap, wrap, tap

Yes — **trap two CAT5s parallel inside a Dyneema/Kevlar braid.** A **4-strand flat plait** is the right cage: it stays oval, so the two valleys do not collapse into a round rope and eat `d⊥`.

```
        Dyn (load)
     CAT5-A      CAT5-B     ← trapped, parallel, not twisted
        Dyn (load)
   cover: 4-plait Dyn/Kev  (S/Z pair so it does not roll)
```

**Wrap before braid.** Serve each CAT5 with PET tape or thin nylon (or a slit PET sleeve) so the plait cannot chew the jacket or change pair impedance. Then run the 4-plait over the served pair.

**Tap at stations (every 0.40–0.45 m):** do not cut the strength braid. Peel serving, punch-down or IDC onto brown (analog) or a CAN stub, heat-shrink, re-serve. The CAT5 is a **bus with drops**, not a new homerun per phone. Tail: loop A→B as in §3.

Tight 4-plait without clips → `d⊥` ≈ two jacket ODs (**~1–1.5 cm**). That is **weak L/R at 1.8 kHz** (~4–7° of phase) and **useful at 12–35 kHz**. For 1.8 kHz L/R, add **thin oval spacers** in the plait (or a ladder clip every station) to hold **8–12 cm**. Same braid, fatter former.

Do **not** use the four Dyneema legs as the Ethernet — CAT5 is passenger, Dyn is load. Contrahelical / flat plait so the oval **does not roll** (a roll swaps port/starboard).

---

## 3. The loop (“token ring”)

A **physical U-turn at the tail** is gold. IBM 802.5 Token Ring chips are not.

```
USV ── CAT5-A ────────────────────┐
     ── CAT5-B ────────────────────┘  (splice / loopback plug at drogue)
```

| Use | How |
|-----|-----|
| **Failover** | CAN (or 100BASE-TX) primary on A, shadow on B. Tail short = ring. Cut one side, still a bus from the other. |
| **Round-trip delay** | Pulse on A, hear it on B. `2L / v_prop` → **length of the living hose** (stretch, reel, bite). Recal `delay_ns`. |
| **Token** | Cheap **poll/token on CAN**: gateway holds the token, each pod speaks in turn. That is TDMA, not 802.5. |
| **Power** | +VIN out on A-blue, return on B-blue — or both carry +VIN and share braid as drain. Fuse both ends. |

Do **not** run a spanning-tree Ethernet ring on ESP32s for v1. Dual CAN or dual RS-485 is the ring.

Electrical length of the loop also gives a **1-D stretch gauge**: nylon VIM will change `τ`; acoustic-section Dyneema should not.

---

## 4. Pair budget (two CAT5 = 8 pairs)

| Pair | Line | Job |
|------|------|-----|
| A orange | out | CAN H/L |
| A green | out | RS-422 PPS |
| A blue | out | +12–24 V |
| A brown | out | analog phones **line 1** (or spare CAN) |
| B orange | return | CAN shadow / ring close |
| B green | return | PPS sense (loopback for delay cal) |
| B blue | return | power return / second rail |
| B brown | return | analog phones **line 2** (the twin) |

Pods at 0.4 m: either analog on brown pairs into a head ADC, or a tiny S3+TWAI every N metres tapping CAN+PPS+VIN.

---

## 5. What this does to accuracy

- Twin `d⊥` ~10 cm at 1.8 kHz: **L/R discrimination**, not a 2-D image.
- Loop delay: hose length to **cm** electrically (5 ns/m × 2).
- Load on Dyneema: `d⊥` and along-track stations stay put under tow; nylon-only braid would **grow** and walk the steering vector.
- Token/poll: no Wi-Fi, no collision, same USB-CAN gateway.

---

## 6. Do not

- Helix the two CAT5s.
- Hang tension on nylon in the **acoustic** section.
- IBM Token Ring PHYs.
- Skip spacers and hope a round braid holds 10 cm.
- Forget contrahelical strength (torque → the twin **rolls** and L/R swaps).
