# IT3-WavePixel v1.0 (Zenodo 23136398)

Board card: 49f20004-c927-4eac-a2ee-8c852622eecb. Reviewed 2026-10-07.

## Summary

Logvinovich and Perez (preprint, 2026-10-04, CC-BY-4.0) model an atom as three integer registers: Z (nuclear charge, "hardware address"), N (neutron count, "ECC padding") and M (a 172-bit electron occupation mask). Nuclear and chemical events become bit edits, for example Pb -> Au + 3p as `0x52 -> 0x4F` with popcount(XOR) = 4. The "audit script" in the paper is about 600 lines of Python assertions. It checks integer and bit identities, not physics. Almost every "exact" Standard Model value in it is a typed-in literal or an arithmetic coincidence chosen after the fact. The paper labels most of its own claims as interpretation, but the abstract and title read as if the physics were derived.

## Verdict: curiosity

The script verifies arithmetic the authors set up themselves, and it derives no physical constant. The only reusable content is generic bit-set bookkeeping that WeftOS already has better tools for.

## What the paper claims

- READ: 172 electron "layers" with capacities `[2,8,8,18,18,32,32,18,18,8,8,2]` (sum 172). Cumulative sums reproduce the noble-gas atomic numbers 2,10,18,36,54,86,118 (paper Sec. 3, script lines 95-115).
- READ: topological charge `Q_top(Z,M) = Z - popcount(M)`. Ionization, beta decay, electron capture, alpha decay and the Auger effect are bookkeeping on Z and popcount(M) (Sec. 5, script 199-236).
- READ: neutron "strain" proxy `Z^2/(Z+N)` falls as N rises, offered as a reason heavy nuclei need N > Z. The paper says this is not a derivation of the mass formula or magic numbers (Sec. 6).
- READ: Pb -> Au + 3p has "binary friction" popcount(82 xor 79) = 4. Stable Au-197 needs 208 - 3p - 8n (Sec. 9, script 139-176).
- READ: a two-strand lepton "chiral projector" (1-eps)/2 explains parity violation. A torus-holonomy picture of entanglement is called "no-signalling compatible".
- READ: particle-physics "register words": `A_FB = 3/13`, `1/alpha ~ 137.037` (integer part 137 = 0x89), `M_Z' = 1088 GeV = 0x0440`, `Xi_cc = 3619 MeV` against an experimental 3621. Section 19 lists "falsifiable bets", for example no periodic chemistry at Z >= 173.
- READ: the paper uses its own evidence levels: A (exact arithmetic), B (interpretation under the postulates), C (suggestive coincidence), O (open). Most physical claims are tagged B, C or O.

## Audit of the script

Method: the code is a `lstlisting` block in `Cifra_logos_Perez.tex` (lines 2479-3089). I extracted it to `audit.py` in the scratchpad and read it in full. It imports only `fractions.Fraction`. A grep for `open(`, `socket`, `urllib`, `requests`, `os.` and `subprocess` found nothing. I ran it in an empty temp directory (`.../scratchpad/wavepixel/run/`) and it wrote no files. MEASURED: the run completes and prints "Level A identities passed". Every check is an `assert` against a value typed into the same file, so a pass says only that the file is self-consistent.

Findings:

1. **Tautological hex/decimal "identities" (MEASURED/READ).** `Z_PB == 0x52`, `Z_AU == 0x4F` (lines 145-146), `18 == 0x12` (376), `WALL13 == 0x0D` (385), `ZPRIME == 0x0440` (489), `XI_LIN == 0x0E23` (495). These confirm that Python parses hex literals. `Z_PB ^ Z_AU == 29` and `popcount == 4` (150-155) are true for any pair of integers that happen to be 82 and 79. Nothing links the count of 4 to a transmutation rate or cross-section, and the paper itself says it predicts no cross-section (Sec. 19.2).
2. **Shell capacities are the target, not an output (READ).** Lines 95-103 define `kappa` and `q(n)`, then `assert C == [2,8,8,18,...]`. The formula was built to reproduce the Madelung period lengths; the assert on line 103 is the check that the fit worked. Real chemistry has capacities 2,8,8,18,18,32,32, and the mirror half (18,18,8,8,2) up to 172 is the authors' extrapolation. The noble-gas list (line 341) and the valence table (353-370) are typed in and checked against `closure_distance`, a function of the same hard-coded `S`. The "valence proxy" gives C=4, N=3, O=2, F=1 because |Z - nearest closure| does, which is the octet rule restated.
3. **Particle-physics values are literals (READ).** `AFB = Fraction(3,13)` (479), `ZPRIME = 1088` (488), `XI_LIN = 3619`, `XI_EXP = 3621` (491-492). `SMACRO = 72` and `SMICRO = 24` (499-500) have no derivation in the script, and `CONF3 = 3` is then asserted equal to `v_q` (506), where `v_q = 3` is itself an input (394).
4. **A_FB = 3/13 is arithmetic from chosen inputs (READ, INFERRED).** Lines 393-436 set `v_l=2, v_q=3, k_CS=v_l`. They then compute `A = 4D/(4+D^2)` and multiply by 3/4. The formula and the choice `k_CS = v_l` are postulates, so the 3/13 follows by construction. INFERRED from standard electroweak values: 0.2308 is close to sin^2(theta_W) (~0.2312). It is not close to any measured forward-backward asymmetry (about 0.017 for leptons and 0.099 for b quarks at the Z pole). I did not look up current PDG values, so treat that comparison as unverified.
5. **137 is three unrelated integer decompositions (READ).** Lines 464-474: `KISS240 = 240`, `NTWIST = 103`, `CAP32 = 32`, `SIGMA136 = 136` are all inputs, then `137 == 240-103`, `13*13-32` and `136|1` are asserted. Three hand-picked ways to reach 137 are not independent evidence. The "geometric alpha" formula (paper Sec. 16.4) is not in the script at all. I coded it from the PDF text: with `N_twist = 103` it gives 137.073, not the paper's 137.037. N=102 gives 136.67 and N=104 gives 137.47 (MEASURED, but my reading of the typeset formula may differ from the authors' intent). Either way, 103 is the one free integer that makes the answer land on 137, and the result moves by about 0.4 per unit step.
6. **Neutron "strain" check is two-point (READ).** Line 258 asserts `strain(82,126) < strain(82,100) < strain(82,82)`, which is monotonicity of Z^2/(Z+N) in N, true for any positive Z. The N/Z "sample" (264-272) is four hand-picked nuclides sorted, not a test. Real nuclear stability needs the semi-empirical mass formula, which the paper disclaims.
7. **Physical Pb -> Au (INFERRED).** The paper's own epistemic guard calls it a "charge-level archetype". Gold from lead in accelerators is real, but it is a fragmentation process with a measured tiny cross-section (a known result from ultra-peripheral heavy-ion work). The register picture adds no predictive content to it.

Net: of the claimed exact observables, none is derived from first principles in the code. Counting and Hamming-weight identities (Level A) are true and trivially so. Everything labelled B/C/O is where the physics would have to be, and it is absent or hard-coded. The paper deserves credit for the level tags and for stating refutation conditions, though most of those conditions (Z >= 173, exotic N <= Z nuclei, FTL signalling) are either far outside current reach or already true of standard physics.

## Transferable ideas for WeftOS

Mostly none. Candidates considered:

- **Occupation mask plus popcount charge.** A fixed-width bitset with `charge = address - weight(mask)` is a conservation check. WeftOS already has stronger versions in ExoChain event hashing and governance gates, and a bitset invariant is a few lines in any language. INFERRED, no new capability.
- **XOR-diff as transition label.** XOR of two bitsets plus popcount is a standard change/distance measure. WeftOS RVF already has vector distances. Nothing here applies beyond general knowledge.
- **"Neutrons as ECC padding".** This is a metaphor with no code-level content. Real error correction (Hamming, Reed-Solomon, the checksums in RVF segments) is unrelated to the paper's N register. No transferable ECC framing.
- **Worth borrowing as practice, not physics.** The A/B/C/O evidence-level tagging, with an explicit refutation condition per claim, is a good habit for WeftOS research notes. It matches the MEASURED/READ/INFERRED tags already used here.

## Sources

- Zenodo record: https://zenodo.org/records/23136398 (API JSON at https://zenodo.org/api/records/23136398)
- Files downloaded: `Cifra_logos_Perez.pdf` (812 KB), `Cifra_logos_Perez.tex` (88 KB, 3681 lines), `IT3_WavePixel_FSM_v2.mp4` (2.4 MB, not viewed)
- Read: abstract, Secs. 2-9, 16, 19 of the PDF text (`pdftotext -layout`), and the full audit script (TeX lines 2479-3089)
- Working files (outside the repo): a temporary directory (not kept)
- Not done: no peer-review search, no PDG lookup, no review of the video, and no check of the cited references.
