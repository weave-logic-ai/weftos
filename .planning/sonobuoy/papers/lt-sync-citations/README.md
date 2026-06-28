# LT-Sync Citation Family — Research Corpus Extension

**Created**: 2026-05-11 by sonobuoy symposium analyst.
**Parent paper**: Zhang & Wu 2025, LT-Sync (`papers/jmse-13-00528.pdf`, analysis at `papers/analysis/jmse-13-528-lt-sync.md`).
**Scope**: All 34 papers cited by LT-Sync, plus follow-on lineage where it leads. User direction 2026-05-11: *"grab nearly every cited document in this. Let me know if there are ones you cannot grab. You can create a whole new folder in papers/ for all of this new research so we keep it together."*

## Why this folder exists

LT-Sync sits at a citation crossroads: its 34 references span
**underwater time-sync** (TSHL, D-Sync, MU-Sync, DA-Sync, DE-Sync,
APE-Sync, Tri-Message, DC-Sync, SFDM, MM-Sync, TSMU), **DSSS /
spread-spectrum underwater comms** (CCOS, Gold codes, chaotic
DSSS, OFDM, M-ary cyclic), **FFT-based acquisition** (GNSS-derived
algorithms applied to UWA), **multistatic sonar** (target
localization with Doppler), and **BELLHOP propagation modeling**.
These are the canonical references for the **WeftAcousticTSF
implementation** in ADR-084 §1.

Keeping them in a dedicated subfolder maintains the corpus
organization principle: each major thread of research has its own
home, with `analysis/*.md` cards and `pdfs/*.pdf` storage
(gitignored per the existing convention).

## Folder layout

```
lt-sync-citations/
├── README.md                       (this file — citation index)
├── pdfs/                           (gitignored; PDFs go here when acquired)
└── analysis/                       (per-paper analysis cards)
```

## Access status legend

- ✅ **OPEN** — open access; can be fetched or already in corpus
- 🌐 **PREPRINT-LIKELY** — author/group homepage commonly hosts a free preprint
- 🔒 **PAYWALL** — behind publisher paywall (IEEE / ACM / Elsevier / Wiley / Springer)
- 🇨🇳 **CHINESE-JOURNAL** — published in *J. Electron. Inf. Technol.* or similar; English access difficult
- 📥 **USER-DROP-NEEDED** — request user to acquire and drop into `pdfs/`
- ✅✅ **ACQUIRED** — PDF present in `pdfs/`, analysis in `analysis/`

## Priority for clawft adoption

- **P0** — directly informs the LT-Sync schema or the immediate WeftAcousticTSF implementation
- **P1** — closely related; informs adjacent decisions (TSMU, MM-Sync, joint loc+sync)
- **P2** — background context (surveys, foundational primitives)

## Full citation index (LT-Sync's 34 references)

### Time-synchronization protocols (the core lineage)

| # | Citation | Year | Venue | Priority | Access | Status |
|---|---|---|---|---|---|---|
| [7] | Elson, Girod, Estrin. *Fine-grained network time synchronization using reference broadcasts (RBS)*. OSDI 02. | 2002 | USENIX OSDI | P2 | ✅ open (USENIX makes proceedings free) | ✅✅ ACQUIRED at `pdfs/[07]-elson-2002-rbs.pdf` |
| [8] | Ganeriwal, Kumar, Srivastava. *Timing-sync protocol for sensor networks (TPSN)*. SenSys 03. | 2003 | ACM SenSys | P2 | 🔒 ACM DL | 📥 user-drop |
| [9] | Maróti, Kusy, Simon, Lédeczi. *The flooding time synchronization protocol (FTSP)*. SenSys 04. | 2004 | ACM SenSys | P2 | 🔒 ACM DL | 📥 user-drop |
| **[10]** | **Syed, Heidemann. *Time Synchronization for High Latency acoustic networks (TSHL)*. INFOCOM 06.** | 2006 | IEEE INFOCOM | **P0** | 🌐 PREPRINT-LIKELY at USC/ISI | 📥 try-fetch |
| **[11]** | **Chirdchoo, Soh, Chua. *MU-Sync: A time synchronization protocol for underwater mobile networks*. WUWNet 08.** | 2008 | ACM WUWNet | **P1** | 🔒 ACM DL | 📥 user-drop |
| **[12]** | **Lu, Mirza, Schurgers. *D-sync: Doppler-based time synchronization for mobile underwater sensor networks*. WUWNet 10.** | 2010 | ACM WUWNet | **P0** | ✅ OPEN (ACM conference archive) | ✅✅ ACQUIRED at `pdfs/[12]-lu-2010-d-sync.pdf` |
| **[13]** | **Liu, Wang, Zuba, Peng, Cui, Zhou. *DA-Sync: A Doppler-Assisted Time-Synchronization Scheme*. IEEE T-MC.** | 2014 | IEEE TMC | **P0** | 🔒 IEEE | 📥 user-drop |
| **[14]** | **Zhou, Wang, Nie, Qiao. *DE-Sync: A Doppler-Enhanced Time Synchronization*. Sensors.** | 2018 | MDPI Sensors | **P0** | ✅ OPEN | ✅✅ ACQUIRED at `pdfs/[14]-zhou-2018-de-sync.pdf` (15 pp, via Europe PMC mirror PMID:29799468); analysis at `analysis/[14]-zhou-2018-de-sync.md` |
| **[15]** | **Zhou, Wang, Han, Qiao, Sun, Niaz. *APE-Sync: An Adaptive Power Efficient Time Synchronization*. IEEE Access.** | 2019 | IEEE Access | **P0** | ✅ OPEN | 📥 try-fetch |
| **[16]** | **Tian, Jiang, Liu, Wang, Liu, Wang. *Tri-Message: A Lightweight Time Synchronization Protocol*. ICC 09.** | 2009 | IEEE ICC | **P0** | ✅ OPEN (NJU author page) | ✅✅ ACQUIRED at `pdfs/[16]-tian-2009-tri-message.pdf` |
| [17] | Ouyang, Han, Wang, He. *Underwater Network Time Synchronization Method Based on Probabilistic Graphical Models*. J. Mar. Sci. Eng. | 2024 | MDPI JMSE | P1 | ✅ OPEN (MDPI) | 📥 try-fetch (note MDPI Akamai block; user may need to drop) |
| [18] | Sun, Ouyang, Han. *DC-Sync: A Doppler-Compensation Time-Synchronization Scheme*. IEEE Access. | 2024 | IEEE Access | P1 | ✅ OPEN | 📥 try-fetch |
| [19] | Wang, Su, Fan. *SFDM: A time-synchronization-free detection mechanism*. Ocean Eng. | 2024 | Elsevier Ocean Engineering | P1 | 🔒 Elsevier | 📥 user-drop |
| [33] | Lin, Feng, Wen, Wang, Lv, Zhao, Han. *MM-sync: A Mobility Built-in Model Based Time Synchronization Approach*. BDCloud 16. | 2016 | IEEE BDCloud | P1 | 🔒 IEEE | 📥 user-drop |
| [34] | Liu, Wang, Peng, Zuba, Cui, Zhou. *TSMU: A Time Synchronization Scheme for Mobile Underwater Sensor Networks*. GLOBECOM 11. | 2011 | IEEE GLOBECOM | P1 | 🔒 IEEE | 📥 user-drop |

### Time-sync + localization joint papers

| # | Citation | Year | Venue | Priority | Access | Status |
|---|---|---|---|---|---|---|
| [1] | Liu, Wang, Cui, Zhou, Yang. *A Joint Time Synchronization and Localization Design for Mobile Underwater Sensor Networks*. IEEE T-MC. | 2016 | IEEE TMC | P1 | 🔒 IEEE | 📥 user-drop |
| [3] | Vermeij, Munafò. *A Robust, Opportunistic Clock Synchronization Algorithm for Ad Hoc Underwater Acoustic Networks*. IEEE J. Ocean. Eng. | 2015 | IEEE JOE | P1 | 🔒 IEEE | 📥 user-drop |
| [5] | Gong, Li, Jiang. *AUV-Aided Joint Localization and Time Synchronization for UWASNs*. IEEE Signal Process. Lett. | 2018 | IEEE SPL | P1 | 🔒 IEEE | 📥 user-drop |

### CSAC / atomic-clock context (foundational for Phase 4+ ranging)

| # | Citation | Year | Venue | Priority | Access | Status |
|---|---|---|---|---|---|---|
| [6] | Gardner, Collins. *A Second Look at Chip Scale Atomic Clocks for Long Term Precision Timing Four Years in the Field*. MTS/IEEE Oceans 16. | 2016 | IEEE Oceans | **P0** | 🌐 PREPRINT-LIKELY (WHOI gray literature common) | 📥 try-fetch |

### Surveys (context)

| # | Citation | Year | Venue | Priority | Access | Status |
|---|---|---|---|---|---|---|
| [2] | Luo, Wu, Ruby, Hong, Guo, Ni. *Simulation and Experimentation Platforms for Underwater Acoustic Sensor Networks*. ACM Comput. Surv. | 2017 | ACM CSUR | P2 | 🔒 ACM DL | 📥 user-drop |
| [4] | Tuna, Gungor. *A survey on deployment techniques, localization algorithms, and research challenges for UWASNs*. Int. J. Commun. Syst. | 2017 | Wiley IJCS | P2 | 🔒 Wiley | 📥 user-drop |
| [25] | Lasassmeh, Conrad. *Time Synchronization in Wireless Sensor Networks: A Survey*. SoutheastCon 10. | 2010 | IEEE SoutheastCon | P2 | 🔒 IEEE | 📥 user-drop |

### Spread-spectrum underwater acoustic comms

| # | Citation | Year | Venue | Priority | Access | Status |
|---|---|---|---|---|---|---|
| [20] | Zhou, Zhang, Zhang, Nie, Wang, Liu. *Underwater Acoustic Spread Spectrum Communications Based on Space-Time Cluster Processing*. J. Electron. Inf. Technol. | 2022 | Chinese journal | P2 | 🇨🇳 CHINESE-JOURNAL | 📥 user-drop (English version may not exist) |
| [21] | Li, Liu, Jia, Huang, Xiao, Guo. *Mapping Sequences Spread Spectrum UWA Communications Using Gold Codes*. WUWNet 21. | 2021 | ACM WUWNet | P1 | 🔒 ACM DL | 📥 user-drop |
| [22] | Hu, Han, Liu, Zhang, Zhang. *Combination Differential DSSS Algorithm for Mobile UWA Communication*. J. Electron. Inf. Technol. | 2022 | Chinese journal | P2 | 🇨🇳 CHINESE-JOURNAL | 📥 user-drop |
| [23] | Shu, Wang, Wang, Yang. *Chaotic direct sequence spread spectrum for secure UWA communication*. Appl. Acoust. | 2016 | Elsevier App. Acoustics | P1 | 🔒 Elsevier | 📥 user-drop |
| [24] | Du, Xiong, Wang. *Research on mobile spread spectrum underwater acoustic communication*. ICSPCC 19. | 2019 | IEEE ICSPCC | P1 | 🔒 IEEE | 📥 user-drop |
| [28] | Ra, Youn, Kim. *High-Reliability UWA Communication Using an M-ary Cyclic Spread Spectrum*. Electronics. | 2022 | MDPI Electronics | P2 | ✅ OPEN | 📥 try-fetch |

### Chaotic-sequence / CCOS foundations

| # | Citation | Year | Venue | Priority | Access | Status |
|---|---|---|---|---|---|---|
| **[27]** | **Xiao, Xuan, Wu. *Research on an Improved Chaotic Spread Spectrum Sequence*. ICCCBDA 18.** | 2018 | IEEE ICCCBDA | **P0** | 🔒 IEEE | 📥 user-drop |
| [29] | Sun, Zhou, Mou. *Design and Performance Analysis of Multi-user Chaotic Sequence Spread-Spectrum Communication System*. J. Electron. Inf. Technol. | 2007 | Chinese journal | P2 | 🇨🇳 CHINESE-JOURNAL | 📥 user-drop |

### FFT-based acquisition foundations

| # | Citation | Year | Venue | Priority | Access | Status |
|---|---|---|---|---|---|---|
| **[30]** | **Kim, Kong. *Design of FFT-Based TDCC for GNSS Acquisition*. IEEE T-WC.** | 2014 | IEEE TWC | **P0** | 🔒 IEEE | 📥 user-drop |

### Multistatic sonar / target localization

| # | Citation | Year | Venue | Priority | Access | Status |
|---|---|---|---|---|---|---|
| [26] | Yang, Yang, Ho. *Moving Target Localization in Multistatic Sonar by Differential Delays and Doppler Shifts*. IEEE Signal Process. Lett. | 2016 | IEEE SPL | P1 | 🔒 IEEE | 📥 user-drop |
| [31] | Narykov, Wright, García-Fernández, Maskell, Ralph. *Poisson multi-Bernoulli mixture filtering with an active sonar using BELLHOP simulation*. FUSION 22. | 2022 | IEEE FUSION | P2 | 🔒 IEEE | 📥 user-drop |

### Underwater OFDM / routing context

| # | Citation | Year | Venue | Priority | Access | Status |
|---|---|---|---|---|---|---|
| [32] | Ghafoor, Noh, Koo. *OFDM-based spectrum-aware routing in underwater cognitive acoustic networks*. IET Commun. | 2017 | IET Communications | P2 | 🔒 Wiley/IET | 📥 user-drop |

## Already in clawft corpus (no re-fetch needed)

- **TSHL** [10]: already at `papers/analysis/tshl-clock-sync.md` (round-2 analysis).
- **D-Sync** [12]: covered in `papers/analysis/cooperative-buoy-positioning.md` (round-2).
- **BELLHOP** [31]'s solver: covered in `papers/analysis/bellhop-ray-tracing.md` (round-2).

These three are already analyzed; no need to re-acquire PDFs.

## Recommended order of acquisition

If acquiring all 34 is too much in one pass, **prioritize these 8 (all P0)**:

1. [10] TSHL — Syed & Heidemann 2006 INFOCOM (✅ already in corpus)
2. [12] D-Sync — Lu, Mirza, Schurgers 2010 WUWNet (✅ already covered)
3. [13] DA-Sync — Liu et al. 2014 IEEE TMC
4. [14] DE-Sync — Zhou et al. 2018 Sensors (OPEN access — try-fetch likely succeeds)
5. [15] APE-Sync — Zhou et al. 2019 IEEE Access (OPEN access — try-fetch likely succeeds)
6. [16] Tri-Message — Tian et al. 2009 IEEE ICC
7. [27] Xiao et al. 2018 improved chaotic spread spectrum (the CCOS foundation)
8. [30] Kim & Kong 2014 IEEE TWC FFT-based TDCC (the acquisition algorithm)

Plus [6] Gardner & Collins 2016 CSAC paper for the Phase 4+ ranging context.

## Acquisition workflow (when PDFs arrive)

For each PDF dropped into `pdfs/`:

1. **Read the PDF** with the Read tool's `pages` parameter for long papers (>10 pages).
2. **Produce a per-paper analysis card** at `analysis/<paper-slug>.md` matching the existing pattern (citation header + verification + summary + key equations + clawft-relevance + cross-references). Aim for 200-400 lines per paper.
3. **Update this README** to mark the paper ✅✅ ACQUIRED.
4. **Update `ADR-084 §1`** if the paper changes the WeftAcousticTSF schema or its parameters.

## Acquisition status — automated fetch results (2026-05-11, updated)

### First pass (failed) — direct publisher access

WebFetch, direct curl, Semantic Scholar API, PubMed Central,
EuropePMC, arXiv. **0 of 34 papers acquired**: every publisher
blocked by Akamai/Cloudflare.

### Second pass (succeeded) — WebSearch + alternative hosts

WebSearch surfaced non-bot-protected hosts: author homepages,
university repositories, conference-archive direct PDFs.
**Combined with curl, 2 LT-Sync references and 5 closely-related
extras acquired**:

**LT-Sync direct references acquired**:

- ✅✅ **[12] D-Sync** (Lu, Mirza, Schurgers 2010 WUWNet) at
  `pdfs/[12]-lu-2010-d-sync.pdf` (166 KB, 8 pages). Source:
  http://wuwnet.acm.org/2010/papers/006.pdf (ACM WUWNet
  conference-archive direct PDF).
- ✅✅ **[16] Tri-Message** (Tian, Jiang et al. 2009 IEEE ICC) at
  `pdfs/[16]-tian-2009-tri-message.pdf` (227 KB, 5 pages).
  Source: cs.nju.edu.cn (Nanjing University author homepage,
  not bot-protected).

**Closely-related extras** (not in LT-Sync's reference list but
highly relevant to WeftAcousticTSF / Doppler-aware time-sync /
multicarrier UWA comms):

- ✅✅ **Mason, Berger, Zhou, Willett 2008** "Detection,
  Synchronization, and Doppler Scale Estimation with Multicarrier
  Waveforms in Underwater Acoustic Communication" — UConn group,
  6 pages, IEEE Oceans 2008. Source: CMU author page.
- ✅✅ **Jiang, Wang, Yu 2019 (SJTU)** "Doppler Scale Estimation
  for Underwater Acoustic Communications Using Zadoff-Chu
  Sequences" — Shanghai Jiao Tong U., 6 pages, WCSP 2019.
- ✅✅ **EMU-Sync analysis** (Phak, et al. 2014?) — 4 pages,
  Nakhon Pathom Rajabhat University. Lighter-weight analysis of
  the EMU-Sync protocol with comparisons to D-Sync.
- ✅✅ **Liu, Zhu, Li, Fang, Wu 2021 Sensors** "Energy-Efficient
  Time Synchronization Based on Nonlinear Clock Skew Tracking
  for Underwater Acoustic Networks" — 13 pages. Builds on the
  DE-Sync / APE-Sync lineage; arguably **closer to clawft's
  Class C use case than LT-Sync** (low-mobility, energy-
  efficient).
- ✅✅ **Ghalkhani, Zhang et al. 2024 ASILOMAR** "UWGS:
  Geometry-Based Enhanced Time Synchronization in Underwater
  Acoustic Mobile Networks" — U. Padova SIGNET group, 7 pages.
  2024 paper, recent. Geometric-vs-Doppler-vs-PGM lineage; novel
  approach that doesn't appear in LT-Sync's references but is
  state-of-the-art parallel work.

### Third pass (2026-05-11 cont., paper-search MCP suite)

After paper-search / unpaywall MCP tools came online, retried the
acquisition with `download_with_fallback` (Crossref/OpenAlex/EuropePMC/
Semantic-Scholar routes), `download_semantic` (DOI: prefix), and
`unpaywall_get_fulltext_links`. Results:

**Acquired (2 new + 1 from second pass)**:

- ✅✅ **[7] RBS** (Elson, Girod, Estrin 2002 USENIX OSDI) at
  `pdfs/[07]-elson-2002-rbs.pdf` (210 KB, 16 pages). Source:
  https://www.usenix.org/legacy/event/osdi02/tech/full_papers/elson/elson.pdf
  — USENIX proceedings always open, no bot protection.
- ✅✅ **[14] DE-Sync** (Zhou, Wang, Nie, Qiao 2018 MDPI Sensors) at
  `pdfs/[14]-zhou-2018-de-sync.pdf` (2.4 MB, 15 pages). Source:
  Europe PMC mirror via paper-search `download_with_fallback` with
  source=crossref, which traversed the DOI to PMID:29799468 and pulled
  the full author-version PDF that PMC mirrors. **This is the most
  important acquisition of the session** — DE-Sync is the direct
  predecessor to LT-Sync and the calibration loop that all the rest
  of the family inherits.

**Definitive failures (require user-drop)** — confirmed via the new
MCP suite that the following are categorically blocked from automated
fetch on this network/IP:

- **IEEE Xplore PDFs** (HTTP 418 from Cloudflare bot detection): [1],
  [3], [5], [6], [13], [15], [18], [24], [25], [26], [30], [31], [33], [34].
  Confirmed via `download_semantic` returning `418 Client Error: Unknown
  Code for url: https://ieeexplore.ieee.org/document/...` for [13]
  (6412670), [15] (8694779). Unpaywall has correct DOI mappings but
  the IEEE PDF URLs are unfetchable.
- **MDPI Akamai 403** (still blocked despite Europe PMC mirror trick
  working for [14] DE-Sync): [17] Ouyang 2024 JMSE, [28] Ra 2022
  Electronics. JMSE and Electronics aren't reliably mirrored on PMC,
  so the same route that worked for [14] Sensors fails here.
  - For [17] Ouyang specifically: `download_with_fallback` returned
    `europepmc_PMID_41181524.pdf` but **PDF verification showed it is
    a different paper** (Yap et al. "offshore mooring inspection
    drones") — Europe PMC's DOI-to-PMID mapping for non-medical MDPI
    journals is unreliable. **Do NOT trust automated fetch for non-PMC
    MDPI journals without first-page verification.**
- **ACM Digital Library** still paywalled with login: [2], [8], [9], [11], [21].
- **Wiley / Elsevier / IET** subscription required: [4], [19], [23], [32].
- **Chinese-language journals**: [20], [22], [29].

**Newly confirmed wrong DOIs in source citations**:

- **[15] APE-Sync**: papers-needed.csv had DOI `10.1109/ACCESS.2019.2911288`
  which Unpaywall maps to a DIFFERENT paper. The **correct DOI is
  `10.1109/ACCESS.2019.2912229`** (verified via OpenAlex W2941329658,
  abstract matches). Update papers-needed.csv accordingly.
- **[27] Xiao 2018 ICCCBDA**: papers-needed.csv DOI `10.1109/ICCCBDA.2018.8386556`
  resolves to "Channel resource allocation based on graph theory and
  coloring principle in cellular networks" — not the chaotic SS paper.
  Either the DOI is wrong or the conference proceedings entry was
  miscataloged. **Real DOI for the Xiao paper unknown; needs user
  investigation.**
- **[6] Gardner & Collins CSAC**: DOI `10.1109/OCEANS.2016.7761340`
  resolves to "On collapse failure analysis of subsea corroded sandwich
  pipelines under external pressure" — not the CSAC paper. **Real
  DOI also wrong; user-drop required, possibly via WHOI gray
  literature instead of IEEE.**

### What's still needed (28 LT-Sync references + others)

For full coverage of LT-Sync's citation graph, 32 papers remain
in 📥 USER-DROP-NEEDED state. Most-impactful P0 priority:

- [14] DE-Sync (Zhou et al. 2018, Sensors) — partial PMC mirror
  attempt failed; user-drop needed
- [15] APE-Sync (Zhou et al. 2019, IEEE Access) — IEEE Xplore;
  user-drop needed
- [27] Xiao 2018 chaotic spread-spectrum — IEEE ICCCBDA; user-drop
- [30] Kim & Kong 2014 FFT-TDCC GNSS acquisition — IEEE TWC;
  user-drop

The combination of [12] D-Sync + [16] Tri-Message + [14] DE-Sync
(acquired this session) + LT-Sync itself + the extras already gives
us enough depth to draft the WeftAcousticTSF firmware implementation
in ADR-084 §1 with high confidence. **With DE-Sync now read and
analyzed (see `analysis/[14]-zhou-2018-de-sync.md`), the core
calibration loop and skew–Doppler coupling math (eq. 5, 12, 13) are
fully captured.** The other 28 papers refine / extend / contextualize
but aren't strictly blocking.

### Original "0 of 34" report

The first pass result was **0 of 34** before WebSearch +
alternative-host discovery. Second-pass total was **2 of 34 LT-Sync
references** plus **5 closely-related papers**. After the third
pass (this session, paper-search MCP suite), updated total: **4 of
34 LT-Sync references** (the two from pass-2 plus [7] RBS and
[14] DE-Sync) plus the same 5 extras.

The acquisition blockers were:

- **MDPI** (Sensors, Electronics, JMSE): Akamai edge returns
  HTTP 403 on every tool/curl attempt, including the
  `openAccessPdf` URL that Semantic Scholar surfaces. Worked
  around for the LT-Sync paper itself because the user manually
  dropped the PDF; same workaround needed for [14] DE-Sync,
  [17] Ouyang 2024, and [28] Ra et al.
- **IEEE Xplore / IEEE Access**: Cloudflare bot-challenge page
  returned in place of PDF. Affects [1], [3], [5], [6], [13],
  [15], [16], [18], [24], [25], [26], [30], [31], [33], [34].
- **ACM Digital Library**: paywalled with login required;
  bot-detected. Affects [2], [8], [9], [11], [12], [21].
- **Wiley / Elsevier / IET**: subscription required. Affects
  [4], [19], [23], [32].
- **Chinese-language journals** (*J. Electron. Inf. Technol.*):
  no English versions; access required Chinese-language
  publisher portal navigation. Affects [20], [22], [29].
- **arXiv**: no preprints found for any of these papers (they
  are journal/conference proceedings in an applied field that
  rarely uses preprint servers).
- **PubMed Central / EuropePMC mirrors**: even for the papers
  indexed there (DE-Sync has PMC 6021945), the mirror redirect
  returns HTML splash, not direct PDF. The mirror itself works
  for some papers but not consistently.

## User-drop request — 34 papers needed

**All 34 papers are in the same state: user-drop needed.** Please
acquire and drop PDFs into `pdfs/` with filenames matching this
convention (so the analysis cards can be auto-linked):

```
pdfs/
├── [01]-liu-2016-joint-sync-loc.pdf
├── [02]-luo-2017-uwasn-platforms.pdf
├── [03]-vermeij-2015-opportunistic.pdf
├── [04]-tuna-2017-survey.pdf
├── [05]-gong-2018-auv-aided.pdf
├── [06]-gardner-2016-csac.pdf
├── [07]-elson-2002-rbs.pdf
├── [08]-ganeriwal-2003-tpsn.pdf
├── [09]-maroti-2004-ftsp.pdf
├── [10]-syed-2006-tshl.pdf            (✅ already in main corpus at papers/analysis/tshl-clock-sync.md)
├── [11]-chirdchoo-2008-mu-sync.pdf
├── [12]-lu-2010-d-sync.pdf            (✅ already covered in cooperative-buoy-positioning.md)
├── [13]-liu-2014-da-sync.pdf
├── [14]-zhou-2018-de-sync.pdf         (P0 — request priority)
├── [15]-zhou-2019-ape-sync.pdf        (P0 — request priority)
├── [16]-tian-2009-tri-message.pdf     (P0 — request priority)
├── [17]-ouyang-2024-pgm-sync.pdf
├── [18]-sun-2024-dc-sync.pdf
├── [19]-wang-2024-sfdm.pdf
├── [20]-zhou-2022-st-cluster-dsss.pdf
├── [21]-li-2021-gold-codes-uwa.pdf
├── [22]-hu-2022-combined-dsss.pdf
├── [23]-shu-2016-chaotic-dsss.pdf
├── [24]-du-2019-mobile-dsss.pdf
├── [25]-lasassmeh-2010-sync-survey.pdf
├── [26]-yang-2016-multistatic-target.pdf
├── [27]-xiao-2018-improved-chaotic.pdf (P0 — CCOS foundation)
├── [28]-ra-2022-mary-cyclic.pdf
├── [29]-sun-2007-multiuser-chaotic.pdf (Chinese journal — English may not exist)
├── [30]-kim-2014-fft-tdcc-gnss.pdf    (P0 — FFT acquisition foundation)
├── [31]-narykov-2022-pmb-bellhop.pdf
├── [32]-ghafoor-2017-ofdm-routing.pdf
├── [33]-lin-2016-mm-sync.pdf
└── [34]-liu-2011-tsmu.pdf
```

**P0 priority requests (the 5 that most affect WeftAcousticTSF
implementation):**

1. **[14] DE-Sync** (Zhou et al. 2018, Sensors) — the closest
   Doppler-Enhanced predecessor to LT-Sync; understanding it
   sharpens our adaptation choices.
2. **[15] APE-Sync** (Zhou et al. 2019, IEEE Access) — adaptive-
   power variant; informs Class C battery-budget firmware.
3. **[16] Tri-Message** (Tian et al. 2009, IEEE ICC) — the
   lightweight 3-message predecessor that LT-Sync directly
   extends.
4. **[27] Xiao et al. 2018, ICCCBDA** — the CCOS spread-spectrum
   sequence foundation. Required reading for the modulation
   choice in ADR-084 §1 adaptation 3.
5. **[30] Kim & Kong 2014, IEEE T-WC** — the FFT-based TDCC
   acquisition algorithm LT-Sync borrows from GNSS. Required
   reading for the ESP-DSP implementation on the S3.

If only some subset is acquirable, **acquire these 5 first**. The
LT-Sync paper already gives us enough to draft the WeftAcousticTSF
implementation; these 5 give us the depth to make defensible
detail decisions during the P3 firmware panel.

## Acquisition pathways the user can use

In order of likely ease:

1. **University library** — most institutions have IEEE, ACM,
   Elsevier, Wiley subscriptions. If you have university
   affiliation, the IP-authenticated portals usually work.
2. **Google Scholar** — search the paper title; the "All N
   versions" link sometimes surfaces author-hosted PDFs (Schurgers
   group at UCSD, Heidemann group at USC/ISI commonly host
   preprints).
3. **ResearchGate** — many authors upload accepted manuscripts.
4. **Author email** — direct request to corresponding author
   for the PDF; nearly always granted within a few days.
5. **MDPI on a non-bot-protected device** — open access papers
   (DE-Sync, Ra et al., Ouyang) can be downloaded from a normal
   browser session without issue; only the bot-detected sessions
   are blocked.
6. **Sci-Hub** — out of scope for this corpus (legal / ethical
   concerns); not a recommended path.

## Workflow once PDFs arrive

For each PDF dropped into `pdfs/`:

1. I'll read with the Read tool (`pages: 1-N` for >10 page papers).
2. Produce a per-paper analysis card at
   `analysis/<paper-slug>.md` matching the existing 65-paper
   corpus convention (citation header + verification + summary
   + key equations + clawft-relevance + cross-references; aim
   for 200-400 lines).
3. Mark the row in this README as ✅✅ ACQUIRED.
4. If the paper changes a WeftAcousticTSF parameter, propagate
   the change to ADR-084 §1 + the LT-Sync analysis card.
