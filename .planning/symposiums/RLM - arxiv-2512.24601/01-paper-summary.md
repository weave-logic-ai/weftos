# Paper Summary: Recursive Language Models (RLMs)

**arXiv:** 2512.24601v2 [cs.AI]

## 1. Title / Authors / Submission Date

- **Title:** Recursive Language Models
- **Authors:** Alex L. Zhang, Tim Kraska, Omar Khattab (all MIT CSAIL)
- **Correspondence:** altzhang@mit.edu, okhattab@mit.edu
- **Preprint date:** January 29, 2026 (arXiv v2 timestamp: 28 Jan 2026)
- **Code:** https://github.com/alexzhang13/rlm
- **PDF metadata title / author fields confirm:** "Recursive Language Models" / "Alex L. Zhang; Tim Kraska; Omar Khattab"

## 2. Core Claim

An LLM can process prompts that are one-to-two orders of magnitude larger than its native context window — and can out-score strong baselines on information-dense tasks within the window too — if the prompt is kept outside the model's context and instead exposed to the model as a variable inside a REPL that the model drives with code and recursive self-calls (§1, §2, Figure 1).

## 3. Method

**Setup.** Given a base LLM `M` with maximum context size `K` and an arbitrary-length prompt string `P ∈ Σ*`, a Recursive Language Model (RLM) is an inference-time scaffold that returns a response `Y ∈ Σ*`. The RLM exposes the same string-in / string-out interface as an ordinary LLM, so callers cannot tell the difference (§2). The authors want effectively unbounded `|P|`, unbounded output length, and unbounded semantic horizon (the ability to do Ω(|P|) or Ω(|P|²) semantic work over the prompt).

**Algorithm 1 (the RLM loop, §2, p. 3):**

```
state   <- InitREPL(prompt=P)              # P lives as a variable, not in M's context
state   <- AddFunction(state, sub_RLM_M)   # register a callable llm_query / sub-RLM
hist    <- [Metadata(state)]               # only metadata (e.g. len(P), short prefix) goes to M
loop:
    code                <- LLM_M(hist)              # M emits Python
    (state, stdout)     <- REPL(state, code)        # execute, persist variables
    hist                <- hist || code || Metadata(stdout)   # only truncated stdout metadata re-enters hist
    if state[Final] is set: return state[Final]
```

Three design choices distinguish RLMs from the "obvious" agent scaffold the authors call Algorithm 2 (§2, p. 3):

1. **Prompt lives in E, not in hist.** `P` is never copied into `M`'s context window; `M` only ever sees constant-size metadata about `P` (length, short prefix, chunk sizes) and whatever `print()` output it chooses to surface from the REPL. This is what lets `|P| >> K` work.
2. **Output is produced through a variable, not autoregressively.** Final answers are returned via `FINAL(text)` or `FINAL_VAR(var_name)` tags; because the answer can be a REPL variable, response length is not bounded by `M`'s output-token limit.
3. **Symbolic recursion.** The REPL exposes an `llm_query(...)` (or sub-RLM) function that `M` can invoke inside arbitrary Python programs — loops, list comprehensions, regex-driven slicing — so it can fire Ω(|P|) or even Ω(|P|²) sub-calls over programmatic slices of `P` and stitch the results back through variables.

**Concrete instantiation (§3.2, "RLM with REPL"):** A Python REPL with `context` (a string, list of strings, or structured object), `llm_query(prompt)` (one-shot call to a sub-LM that "can handle around 500K chars"), `print()`, and the `FINAL` / `FINAL_VAR` sentinel tags. The system prompt (Appendix C, pp. 15-17) is fixed and task-agnostic and includes worked examples (chunk-and-summarise a book, regex-filter for keywords, Markdown-header chunking). Max recursion depth is 1 (sub-calls are plain LM calls, not further RLMs). All sub-calls are synchronous / blocking (Appendix B, p. 14).

**Training a "native" RLM (Appendix A, p. 13):** They distill RLM behaviour into Qwen3-8B by running RLM(Qwen3-Coder-480B-A35B) on 750 LongBenchPro tasks, collecting 2,250 candidate trajectories. They drop scores of 0.0 and single-turn trajectories (down to 1,072), split each root-LM turn into its own SFT (input = history, output = turn), filter turns that exceed ~100k chars, and programmatically patch template mistakes (~16% of turns mis-used `FINAL`, ~13% mis-used `FINAL_VAR`). Training uses the `prime-rl` library, batch size 64, 300 steps, 48 H100-hours. Key hypothesis: sub-calls are just general-purpose LLM requests, so training only the root model's REPL-driving behaviour is enough.

## 4. Results

Main table: Table 1, p. 5. Four benchmarks, two frontier models (GPT-5 with medium reasoning + GPT-5-mini sub-calls; Qwen3-Coder-480B-A35B), plus Qwen3-8B for the fine-tuning study.

| Benchmark | Processing complexity | Length range | Baseline (GPT-5 base) | RLM(GPT-5) |
|---|---|---|---|---|
| S-NIAH (RULER, 50 tasks) | O(1) | 2¹³–2¹⁸ | degrades fast past 2¹⁴ | stays high |
| CodeQA (LongBench-v2) | O(files) | 23K–4.2M tok | 24.0% | **62.0%** |
| BrowseComp-Plus, 1K docs | multi-hop | 6M–11M tok | 0.0% (OOC) | **91.3%** |
| OOLONG (trec_coarse) | O(N) linear | 131K tok | 44.0% | **56.5%** |
| OOLONG-Pairs (new, 20 queries) | O(N²) quadratic | 32K tok | 0.1% F1 | **58.0% F1** |

Headline numbers (§4):

- **BrowseComp-Plus 1K docs (6M–11M input tokens):** RLM(GPT-5) 91.3% vs. best non-RLM baseline (Summary agent) 70.5% vs. base GPT-5 0.0% (out of context). Average API cost $0.99/query, vs. a linearly extrapolated $1.50–$2.75 just for GPT-5-mini to ingest the prompt.
- **OOLONG-Pairs:** GPT-5 and Qwen3-Coder base models both score <0.1% F1; RLM lifts them to 58.0% and 23.1% respectively.
- **RLM-Qwen3-8B (fine-tuned):** beats base Qwen3-8B-as-an-RLM by median 28.3% across the 4 tasks, and beats base Qwen3-8B by much more since the base model is effectively 0 on three of them.
- **Cost (§4, Figure 3, p. 6):** Median RLM run is cheaper than the median base-GPT-5 run (since the base model pays for the whole prompt), but the tail is heavy — 95th-percentile RLM runs are substantially more expensive than any base-model call. RLMs are up to ~3× cheaper than the Summary agent while scoring higher.
- **Degradation curves (Figure 1):** base GPT-5 accuracy drops sharply with both context length and task complexity; RLM(GPT-5) stays roughly flat, and continues past GPT-5's 272K-token window.

Ablation (§4, Observation 2): removing recursive sub-calls (REPL-only RLM) keeps most of the scaling benefit on S-NIAH / CodeQA / BrowseComp+ but loses 10–59 points on information-dense tasks (OOLONG, OOLONG-Pairs), so the REPL handles "scale" while sub-calls handle "density".

## 5. Key Assumptions / Limitations

Stated by the authors (§6, Appendix B):

- **Sync, blocking sub-calls only.** Every `llm_query` is sequential, which is the dominant cause of wall-clock slowness; async / sandboxed REPLs are left as future work.
- **Recursion depth = 1.** Sub-calls are plain LM calls, not further RLMs; deeper recursion untested.
- **Coding ability is a prerequisite for the root model.** Qwen3-8B without the RLM fine-tune does badly because it can't write coherent REPL code; Qwen3-235B runs out of thinking tokens mid-trajectory.
- **The `FINAL` / `FINAL_VAR` sentinel is brittle.** Models occasionally emit plans as final answers, or mix `FINAL` and `FINAL_VAR` incorrectly (~16% / ~13% of training-turn errors), requiring programmatic post-fix.
- **Prompts are model-specific in practice.** GPT-5 and Qwen3-Coder need slightly different system prompts (the Qwen variant adds a "don't make too many sub-calls" warning) or they either over- or under-use `llm_query`.
- **Cost variance is high.** 50th-percentile cost is competitive but 95th-percentile is a long tail driven by trajectory length.
- **Base LM slightly beats RLM on short prompts** (§4, Obs. 3) — there's a break-even length below which the overhead doesn't pay off.

Not addressed by the paper:

- No security discussion of executing model-generated Python in a persistent REPL over adversarial input.
- No empirical study of RLM failure on prompts where recursive decomposition is misleading (e.g., prompts whose answer genuinely requires holistic attention).
- Caching and deduplication of sub-calls (an RLM may call `llm_query` with overlapping inputs) not analysed.
- "Semantic horizon" is used informally; no formal expressivity proof.

## 6. Terminology Glossary

- **RLM (Recursive Language Model):** A scaffold around a base LLM `M` that treats the prompt as an environment variable and lets `M` drive it via code plus recursive self-calls (§2, Algorithm 1).
- **REPL (Read-Eval-Print Loop):** The persistent Python interpreter inside which the RLM's variables (including `context`) live; `M` interacts with it by emitting code fences.
- **Root LLM / sub-LLM:** The outer model that drives the REPL (root) vs. the models invoked by `llm_query(...)` (sub). In this paper root = GPT-5 or Qwen3-Coder-480B; sub = GPT-5-mini or same-family smaller models.
- **Context rot:** Empirical phenomenon (Hong et al. 2025) where LLM quality degrades as prompt length grows even within the nominal window (§1).
- **Effective context window:** Task-dependent length at which quality collapses, distinct from the nominal window; the paper argues more complex tasks rot at shorter lengths (§3, Hsieh et al. 2024, Goldman et al. 2025).
- **S-NIAH / OOLONG / OOLONG-Pairs / BrowseComp-Plus / LongBench-v2 CodeQA:** The four benchmark families used (§3.1), categorised by how their processing complexity scales with `|P|` — constant, linear, quadratic, multi-hop QA, and multi-file code reasoning respectively.
- **CodeAct:** Wang et al. 2024 agent baseline that executes Python inside a ReAct loop but keeps the prompt in the LM's context window, so it inherits the window limit (§3.2).
- **Summary agent / context compaction:** Baseline family (Smith 2025; Sun et al. 2025; Wu et al. 2025) that repeatedly summarises context when it overflows; lossy, and the paper argues under-expressive for dense tasks (§1, §5).
- **Symbolic recursion:** The capability that code inside the REPL can call `llm_query` inside arbitrary loops / programs over programmatically derived slices of `P`, as opposed to the model verbalising sub-task delegations in tokens (§2, design point 3).
- **FINAL / FINAL_VAR:** Sentinel markers the root model uses to signal termination of the RLM loop; `FINAL(x)` returns `x` verbatim, `FINAL_VAR(name)` returns the current REPL value of the variable `name` (Appendix C, p. 16).
- **LongBenchPro:** Training distribution (Chen et al. 2026) used only for distilling RLM-Qwen3-8B; it is disjoint from the four evaluation benchmarks (Appendix A).
