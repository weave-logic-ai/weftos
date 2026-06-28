# MMA / Moving-Magnet Underwater Projectors — Research Stub

**Created**: 2026-05-11.
**Status**: 📋 **Potentials collection only — not a commitment.** Filed as a research option for the VLF beacon role; LT-Sync round-trip exchange between Class-C/A buoys remains the baseline plan.

## Why this folder exists

User direction 2026-05-11: *"We can still use round trips. This can be a beacon. It's an option anyhow, we are not making decisions yet, just collecting potentials."*

A moving-magnet actuator (MMA) underwater acoustic projector is a non-piezo, linear-electromagnetic driver that produces VLF acoustic output (~1 Hz to tens of kHz) from a single transducer. If kept as an *option* for the Class-S anchor role, it could:

- Broadcast a continuously-modulated VLF carrier carrying time + position for the mesh to listen to passively at multi-km range.
- Coexist with — not replace — the LT-Sync round-trip protocol between Class-C/Class-A buoys at higher frequency (38 kHz LFM chirp band).
- Solve the **range/bandwidth ambiguity**: low frequencies propagate far but carry little data; high frequencies carry data but die out fast. An MMA covering both bands in one driver sidesteps the dual-transducer problem.

This folder collects the primary literature so the decision can be revisited later when the architecture review of the Class-S role is on the agenda.

## Acquired

| Status | Citation |
|---|---|
| 📥 user-drop (or browser-session fetch) | Wallin, B. *Implementation of Moving Magnet Actuation in Very Low Frequency Acoustic Transduction*. MS Thesis, Ocean Engineering, University of Rhode Island, 2017. Advisor: James Miller. doi:[10.23860/thesis-wallin-brenton-2017](https://doi.org/10.23860/thesis-wallin-brenton-2017). PDF at https://digitalcommons.uri.edu/cgi/viewcontent.cgi?article=2029&context=theses (CC-BY, bepress refuses curl with HTTP 405; works in browser). Landing page: https://digitalcommons.uri.edu/theses/1017/ |
| 📥 user-drop | Wallin, Crocker, Szelag. *Implementation of moving magnet actuation in very low frequency underwater acoustic transduction* (JASA companion). J. Acoust. Soc. Am. **139**(4_Supplement):2198–2199, April 2016. doi:[10.1121/1.4950552](https://doi.org/10.1121/1.4950552) (AIP, conference-abstract; full thesis is the more substantive source). |

## Key spec numbers from the thesis abstract

- **Source level ~125 dB re 1 µPa @ 1 m at 1 Hz**
- **Source level >180 dB re 1 µPa @ 1 m at 30 Hz**
- Driver method validated by three independent approaches: analytical Matlab model, FEA in Abaqus, bench test with off-the-shelf Bose actuator.
- Sponsor / motivation: NUWC USRD Leesburg facility line-array calibration at 1–100 Hz.

## What this would *potentially* enable for clawft (NOT a decision)

If a Class-S anchor buoy carried an MMA projector instead of (or alongside) a piezo transducer:

1. **VLF continuous beacon** — modulate UTC time and lat/lon onto a 1–10 kHz carrier, transmitted continuously. Class-C/A buoys passively decode it as a coarse-sync reference, complementing the round-trip LT-Sync precision sync. Same role GPS L1 plays for terrestrial nodes.
2. **Multi-km range** — 1–10 kHz suffers far less absorption than 38 kHz LFM (~0.01 dB/km vs ~5 dB/km), so a single anchor could cover an order-of-magnitude larger mesh.
3. **Wider-band transducer** — if the same driver also reaches the chirp band, the Class-S buoy might only need one acoustic aperture, simplifying the pressure-vessel design.

## Tensions vs. project constraints (cost / size / power)

User update 2026-05-11 (later in same session): *"The driver method
is very good honestly, may be cheap enough to have on lots of nodes.
It can be used to encode across very large frequency range and it is
very low power. It's also very loud, 150-170 dB!"* — i.e., this is no
longer scoped as an anchor-only Class-S option; the working hypothesis
shifts to **every node may carry one**.

If that hypothesis holds, the topology question changes:

- Symmetric mesh: any node can broadcast time + ID continuously on
  a per-node VLF carrier slot. No special anchor role.
- Two acoustic apertures per buoy — high-band piezo (chirp matched
  filter, TDOA bearing) + low-band MMA (continuous low-rate beacon /
  time / mesh ID). The two bands don't compete for channel.
- Discovery and coarse-sync done passively at the VLF band; precision
  sync still uses LT-Sync round-trips at chirp band where needed.

**Cost / size / power unknowns to answer (research, not decide):**

- *Cost*: Wallin used an off-the-shelf Bose linear actuator. The
  actuator itself is $50–200-ish; the underwater housing is the
  cost driver. Same packaging problem we already solve for
  hydrophones (oil-fill, urethane bellows, pressure compensation).
- *Size*: Wallin's design is bench-scale. What's the smallest moving-
  magnet actuator that still hits 150 dB at, say, 10 Hz? Open
  question; needs a survey of the off-the-shelf actuator catalog
  (Bose, H2W Technologies, BEI Kimco, Moog) and a re-derivation of
  the SPL ∝ piston-area × stroke × ω² relationship at hobby scale.
- *Power*: User claim "low power"; thesis doesn't quote a wall-power
  number. The actuator's continuous rating is the relevant figure
  (usually a few-watt continuous, tens-of-watt peak for the small
  ones). At a few watts continuous on a 5 W solar-trickle Class A
  buoy, this is feasible — *if* the duty cycle is low or the carrier
  is unmodulated CW (which a time beacon basically is).
- *SPL claim 150-170 dB*: matches Wallin's measured curve (125 dB
  @ 1 Hz climbing to 180 dB @ 30 Hz, so 150-170 dB lives around
  5-20 Hz). At that SL, against ~50-70 dB ambient in the 5-100 Hz
  band and ~0.01 dB/km absorption coefficient, the link budget
  closes at tens of km, not just "several." Order-of-magnitude
  better than the chirp band.

**Open architectural questions if every-node-has-MMA holds:**

1. Per-node VLF carrier slot allocation: how many nodes can the
   1-100 Hz band realistically host? If 1 Hz channels with 1 Hz
   guard band, 50 nodes per cluster is the rough upper bound.
2. How does the MMA's modulated carrier coexist with the chirp band
   (mostly via frequency separation — they're an order of magnitude
   apart, so they don't interfere physically; the question is whether
   the *receiver* analog front-end can handle both bands without
   intermodulation in its AGC stage).
3. What's the simplest time encoding? Bi-phase shift keying of UTC +
   per-node ID on a fixed carrier seems lowest-effort. ~100 bps is
   enough for a 1-pulse-per-second time + 8-byte node ID + position.

## Related potentials (sibling research threads, also not decisions)

A coherent tier-by-distance sync hierarchy is emerging in conversation
(user direction 2026-05-11, brainstorming only):

| Tier | Distance scale | Candidate medium | Sync latency class |
|---|---|---|---|
| Intra-mooring vertical stack | meters | **Near-field magnetic induction** | sub-µs ("near instant") |
| Horizontal buoy mesh | 100 m – few km | Acoustic LFM chirp + **LT-Sync** round-trip | ms-class |
| Optional long-range anchor | ~10 km | **MMA VLF beacon** (this folder) | ms-class, continuous |

The MI option for the vertical stack should get its own research
stub eventually. Anchor literature to look at when the time comes:
Akyildiz-group MI underwater comms surveys, Sun & Akyildiz IEEE
Trans. Antennas Propag. 2010 "Magnetic Induction Communications for
Wireless Underground Sensor Networks," NPS WET (Wireless Electronic
Tether) work. Filed here as a forward-pointer; **no acquisition
attempt this session.**

## Cross-references

- `papers/analysis/jmse-13-528-lt-sync.md` — LT-Sync protocol that round-trip exchange would use.
- `papers/lt-sync-citations/analysis/[14]-zhou-2018-de-sync.md` — DE-Sync calibration loop that LT-Sync inherits.
- `build/build-hydrophone-*.md` — current piezo-based hydrophone implementation path (the receive side; MMA is transmit side).
- `RANGING.md` — inter-buoy ranging design; a VLF beacon broadcasting position would feed directly into this.

## Action items (none urgent)

- [ ] User-drop the Wallin 2017 thesis PDF into `pdfs/` (bepress refuses headless curl).
- [ ] When time permits, search for follow-on MMA projector literature: Sutton 2010s NUWC work, Slamani Toulon papers, Sound Innovations / WirelessSeismic commercial products.
- [ ] If/when the Class-S architecture is revisited, write a full per-paper analysis card following the convention at `papers/analysis/*.md`.
