# Episteme (working name) — Scientific Agent Skills inventory

Source: `k-dense-ai/scientific-agent-skills` (MIT, K-Dense Inc.), shallow-cloned read-only at `~/dev/scientific-agent-skills`. 166 skills. This inventory is derived statically from each skill's `SKILL.md` frontmatter/body, `tests/skill-requirements.toml` (the repo's own per-skill Python dependency map used to build isolated test environments), and the repo's own pre-generated `docs/security-report.json` (Cisco AI Defense skill-scanner output, run by the upstream maintainers, not by this review). No skill was executed, no package was installed, and no external database was queried to produce this table. See `security-review.md` for the security read and `adoption-notes.md` for how this maps into WeftOS.

## Domain counts

| Domain | Skills |
|---|---|
| Bioinformatics & Genomics | 26 |
| Machine Learning & Deep Learning | 18 |
| Analysis & Methodology | 12 |
| Scientific Communication & Publishing | 11 |
| Scientific Databases & Data Access | 10 |
| Data Analysis & Visualization | 9 |
| Cheminformatics & Drug Discovery | 9 |
| Document Processing & Conversion | 7 |
| Engineering & Simulation | 6 |
| Research Methodology & Proposal Writing | 6 |
| Protein Engineering & Design | 5 |
| Data Management & Infrastructure | 4 |
| Medical Imaging & Digital Pathology | 4 |
| Materials Science & Chemistry | 3 |
| Neuroscience & Electrophysiology | 3 |
| Clinical Documentation & Decision Support | 3 |
| Decision & Scenario Analysis | 3 |
| Workflow Platforms & Cloud Execution | 3 |
| Regulatory & Standards Evidence Preparation | 2 |
| Tool Discovery & Computational Resources | 2 |
| Phylogenetics & Evolutionary Biology | 2 |
| Web Search & Information Retrieval | 2 |
| Laboratory Automation | 2 |
| Electronic Lab Notebooks (ELN) | 2 |
| Proteomics & Mass Spectrometry | 2 |
| Autonomous Research & Optimization Frameworks | 1 |
| Laboratory Information Management Systems (LIMS) & R&D Platforms | 1 |
| Cloud Platforms for Genomics & Biomedical Data | 1 |
| Microscopy & Bio-image Data | 1 |
| Agent Frameworks | 1 |
| Pharmacology & Pharmacometrics | 1 |
| Protocol Management & Sharing | 1 |
| Healthcare AI & Clinical Machine Learning | 1 |
| Laboratory Automation & Equipment Control | 1 |
| Preclinical Research & Animal Welfare | 1 |
| **Total** | **166** |

## Flag summary

- **106/166** skills ship `scripts/` (execute code when run — Python unless noted).
- **28/166** declare network access in `compatibility` (an additional set touches network implicitly via a documented API without saying so explicitly in that field — see per-skill compatibility text).
- **47/166** name an API key, account, OAuth credential, or token somewhere in `compatibility` or a declared env var — includes optional/advanced integrations (e.g. an optional cloud-storage or experiment-tracking credential), not only skills that cannot run without one.
- **26/166** carry a structured `metadata.openclaw` credential-gating block (`primaryEnv` + `envVars`) — a machine-readable declaration read by OpenClaw-family hosts, not by Claude Code, Grok, or Codex.
- **5/166** explicitly mention paid/gated/commercial access: bgpt-paper-search, deepspot-m, generate-image, transformers, waypoint-bio.
- **136/166** end `SKILL.md` with a self-citation directive (see `security-review.md` — a repo-wide pattern, not skill-specific misbehavior).

## Full inventory, by domain

Columns: **Skill** (dir name) · **Purpose** (from `description`, truncated) · **Runtime deps** (from `tests/skill-requirements.toml`; "stdlib only" = ships scripts with no third-party package) · **Network/DB** · **Credential** (API key/account/OAuth named) · **Assets** (on-disk size of the skill directory) · **Flags** (X=executes code via `scripts/`, N=network/external API, D=downloads data, $=paid or credentialed API).

### Agent Frameworks (1)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `pi-agent` | Build with and use Pi, the minimal terminal coding harness. | — | unspecified | no | 324 KB | — |

### Analysis & Methodology (12)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `experimental-design` | Design experiments and studies BEFORE data is collected — choosing a design, randomizing, blocking, and laying out treatment combinations… | numpy, pandas, pyDOE3 | unspecified | no | 64 KB | X |
| `exploratory-data-analysis` | Perform bounded, local exploratory analysis of explicitly supported scientific files. | biopython, h5py, numpy, pandas, pillow +2 | none (declared) | no | 248 KB | X |
| `hypogenic` | Plans and audits use of ChicagoHAI HypoGeniC/HypoRefine for LLM-assisted hypothesis generation from labeled text datasets. | pyyaml | network | yes | 184 KB | $DNX |
| `hypothesis-generation` | Formulate evidence-bounded scientific questions, candidate hypotheses, rival explanations, causal or associational claims, discriminating… | stdlib only | none (declared) | no | 296 KB | X |
| `literature-review` | Conduct comprehensive, systematic literature reviews using multiple academic databases (PubMed, arXiv, bioRxiv, Semantic Scholar, etc.). | requests, python-dotenv | unspecified | yes | 156 KB | $X |
| `peer-review` | Prepare evidence-bounded, constructive peer-review drafts and structured manuscript assessments. | stdlib only | none (declared) | no | 236 KB | X |
| `scientific-brainstorming` | Facilitates evidence-aware scientific ideation with independent generation, structured discussion, explicit assumptions, transparent… | stdlib only | none (declared) | no | 148 KB | X |
| `scientific-critical-thinking` | Evaluate scientific claims and evidence quality. | — | none (declared) | yes | 116 KB | $ |
| `scientific-visualization` | Create and audit truthful, accessible, publication-ready scientific figures with Matplotlib, Seaborn, or Plotly. | matplotlib, seaborn, plotly, pypdf, pillow +1 | none (declared) | no | 228 KB | X |
| `scientific-writing` | Draft, revise, and audit scientific manuscripts or reports with explicit evidence provenance, reporting-guideline coverage, authorship… | stdlib only | unspecified | no | 232 KB | X |
| `statistical-analysis` | Guided statistical analysis for research data - test selection, assumption checking, effect sizes, power analysis, Bayesian alternatives,… | statsmodels, pingouin, pymc, arviz, scipy +5 | unspecified | no | 132 KB | X |
| `statistical-power` | Sample-size and statistical power calculations for planning studies. | statsmodels, lifelines, pingouin, matplotlib, numpy +2 | unspecified | no | 64 KB | X |

### Autonomous Research & Optimization Frameworks (1)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `arbor` | Autonomously improve a real artifact (code, training recipe, agent harness, data pipeline, prompt) against an objective and an evaluator,… | arbor | unspecified | no | 64 KB | X |

### Bioinformatics & Genomics (26)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `alphagenome` | Look up precomputed AlphaGenome Atlas effects for any GRCh38 single-nucleotide variant (AVI score with Phred and 18 SHAP feature… | alphagenome | network | yes | 116 KB | $DNX |
| `anndata` | Data structure for annotated matrices in single-cell analysis. | anndata, h5py, muon, fsspec, numpy +3 | unspecified | no | 80 KB | — |
| `arboreto` | Infer gene regulatory networks (GRNs) from gene expression data using scalable algorithms (GRNBoost2, GENIE3). | arboreto, dask[distributed], numpy, pandas, scipy | unspecified | no | 40 KB | X |
| `biopython` | Comprehensive molecular biology toolkit. | biopython, matplotlib, numpy, reportlab | network | yes | 116 KB | $DN |
| `bioservices` | Unified Python interface to 40+ bioinformatics services. | bioservices, biopython, networkx, pandas | network | no | 124 KB | DNX |
| `bulk-rnaseq` | End-to-end bulk RNA-seq orchestrator — takes raw FASTQ reads through QC and trimming (FastQC, fastp/Trim Galore), alignment and… | pydeseq2, pytximport, gseapy, gprofiler-official, pandas | unspecified | no | 64 KB | X |
| `cellxgene-census` | Query the CZ CELLxGENE Census programmatically for versioned public single-cell and spatial transcriptomics data. | cellxgene-census, tiledbsoma, tiledbsoma-ml, scanpy, spatialdata | unspecified | no | 44 KB | — |
| `deeptools` | NGS analysis toolkit. | deeptools | unspecified | no | 108 KB | X |
| `flowio` | Read, inspect, and write Flow Cytometry Standard (FCS) 2.0, 3.0, and 3.1 files with FlowIO. | flowio, numpy, pandas | none (declared) | no | 84 KB | X |
| `geniml` | Use Geniml for audited local genomic-interval workflows: validate BED and universe contracts, plan Region2Vec or scEmbed runs, inspect… | geniml, gtars, pyarrow, scanpy | none (declared) | no | 184 KB | X |
| `genomic-coordinates` | Convert genomic intervals between coordinate conventions, normalise and compare variant representations, and detect assembly or… | stdlib only | none (declared) | no | 120 KB | X |
| `genomic-intelligence` | Predict regulatory features, gene structure, and expression directly from DNA sequence using Genomic Intelligence's hosted transformer DNA… | requests | network | yes | 52 KB | $DN |
| `gget` | Fast CLI/Python queries to 20+ bioinformatics databases. | gget, matplotlib, numpy, pandas, scanpy>=1.10 | unspecified | no | 128 KB | X |
| `gtars` | Use Gtars for local genomic interval models and set algebra, overlaps and counts, consensus and coverage, tokenization, fragment… | gtars | none (declared) | no | 168 KB | X |
| `onekgpd` | Query the 1000 Genomes Project dataset (3,202 whole-genome-sequenced individuals, GRCh38) at the level of individual participants. | dnaerys | network | no | 1.1 MB | DNX |
| `pathway-enrichment` | Run pathway and gene-set enrichment analysis on gene lists or ranked gene data, then interpret the results. | gseapy, gprofiler-official, mygene, numpy, pandas | unspecified | no | 52 KB | X |
| `polars-bio` | High-performance genomic interval operations and bioinformatics file I/O on Polars DataFrames. | polars-bio, polars, pandas, bioframe | unspecified | yes | 80 KB | $ |
| `pydeseq2` | Differential gene expression analysis for bulk RNA-seq with PyDESeq2, including formulaic designs, Wald tests, FDR correction, LFC… | pydeseq2, anndata, matplotlib, numpy, pandas | unspecified | no | 72 KB | X |
| `pysam` | Python/HTSlib workflows for genomic files. | pysam | unspecified | no | 168 KB | X |
| `scanpy` | Standard single-cell RNA-seq analysis pipeline. | scanpy, python-igraph, leidenalg, dask, matplotlib +3 | unspecified | no | 168 KB | X |
| `scikit-bio` | Biological data toolkit. | scikit-bio, biom-format, tables, matplotlib, numpy +2 | unspecified | no | 44 KB | — |
| `scvelo` | RNA velocity analysis with scVelo. | scvelo, cellrank, scanpy, matplotlib, numpy | unspecified | no | 32 KB | X |
| `scvi-tools` | Deep generative models for single-cell omics. | scvi-tools, scanpy, squidpy, mudata, shap +8 | unspecified | no | 124 KB | — |
| `tiledbvcf` | Efficient storage and retrieval of genomic variant data using TileDB. | tiledb, tiledb-cloud | unspecified | yes | 16 KB | $ |
| `waypoint-bio` | Use when working with Outpost Bio's open microbiome foundation models - the Waypoint checkpoints (Waypoint-6m, Waypoint-45m,… | pandas, pyarrow | network | yes | 92 KB | $DNX |
| `zarr-python` | Chunked N-D arrays for cloud storage (Zarr-Python 3). | zarr, fsspec, s3fs, gcsfs, h5py +3 | unspecified | no | 52 KB | — |

### Cheminformatics & Drug Discovery (9)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `datamol` | Pythonic wrapper around RDKit with simplified interface and sensible defaults. | datamol, rdkit, s3fs, gcsfs | unspecified | no | 76 KB | — |
| `deepchem` | Molecular ML with diverse featurizers and pre-built datasets. | deepchem, torch, numpy, scikit-learn (py3.11) | unspecified | no | 88 KB | X |
| `diffdock` | DiffDock and DiffDock-L molecular docking. | torch, rdkit, openmm, esm, pandas | unspecified | no | 92 KB | X |
| `medchem` | Medicinal chemistry filters for compound triage. | medchem, datamol, rdkit, pandas, tqdm | unspecified | no | 52 KB | X |
| `molfeat` | Molecular featurization for ML (100+ featurizers). | molfeat, datamol, torch (py3.10) | unspecified | no | 68 KB | — |
| `pytdc` | Use Therapeutics Data Commons through the PyTDC Python package for registry discovery, approved dataset access, task-aware splits,… | pytdc, setuptools (py3.11) | unspecified | no | 132 KB | X |
| `rdkit` | Cheminformatics toolkit for fine-grained molecular control. | rdkit | unspecified | no | 108 KB | X |
| `rowan` | Rowan is a cloud-native molecular modeling and medicinal-chemistry workflow platform with a Python API. | rowan-python, rdkit, pandas | unspecified | yes | 48 KB | $ |
| `torchdrug` | Build and troubleshoot TorchDrug 0.2.1 workflows for molecular graphs, property prediction, self-supervised pretraining, molecule… | torch (py3.10) | unspecified | no | 76 KB | — |

### Clinical Documentation & Decision Support (3)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `clinical-decision-support` | Prepare and validate research-only clinical decision-support evaluation, evidence-profile, cohort, survival, biomarker/model, privacy, and… | stdlib only | none (declared) | no | 244 KB | X |
| `clinical-reports` | Create safety-bounded draft structures and run local deterministic checks for clinical case, diagnostic, trial, safety, and aggregate… | stdlib only | none (declared) | no | 264 KB | X |
| `treatment-plans` | Format and structurally validate local treatment-plan documentation after clinical decisions have already been supplied and verified by… | stdlib only | none (declared) | no | 200 KB | X |

### Cloud Platforms for Genomics & Biomedical Data (1)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `dnanexus-integration` | Build and operate reproducible genomics workloads on DNAnexus with the dx CLI, dxpy, apps/applets, native workflows, dxCompiler, and… | dxpy>=0.400 | network | yes | 168 KB | $DNX |

### Data Analysis & Visualization (9)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `dask` | Distributed computing for larger-than-RAM pandas/NumPy workflows. | dask[distributed], dask-ml, dask-jobqueue, dask-kubernetes, pandas +6 | unspecified | no | 84 KB | — |
| `geomaster` | Comprehensive geospatial science skill covering remote sensing, GIS, spatial analysis, machine learning for earth observation, and 30+… | geopandas, shapely, pyproj, rasterio, rioxarray +22 | unspecified | no | 196 KB | — |
| `geopandas` | Guidance and local audit tools for Python workflows that directly use GeoPandas GeoSeries, GeoDataFrame, spatial operations, or… | geopandas, pyogrio, pyproj, shapely, pyarrow +2 | none (declared) | yes | 172 KB | $X |
| `matplotlib` | Low-level plotting library for full customization. | matplotlib, ipympl, numpy, pandas, scipy | unspecified | no | 100 KB | X |
| `networkx` | Create, analyze, and visualize complex networks and graphs in Python with NetworkX. | networkx, geopandas, momepy, matplotlib, pandas +3 | unspecified | no | 76 KB | — |
| `polars` | High-performance DataFrame library for Python ETL, analytics, and pandas migration. | polars, pyarrow, pandas, numpy, sqlalchemy | unspecified | no | 92 KB | — |
| `seaborn` | Statistical visualization with pandas integration. | seaborn, matplotlib, numpy, pandas, scipy +1 | unspecified | no | 104 KB | — |
| `uncertainty-and-units` | Track physical units and propagate measurement uncertainty in scientific calculations using pint and uncertainties. | pint, uncertainties, numpy, scipy | none (declared) | no | 232 KB | X |
| `vaex` | Use this skill for processing and analyzing large tabular datasets (billions of rows) that exceed available RAM. | vaex (py3.12) | unspecified | no | 100 KB | — |

### Data Management & Infrastructure (4)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `datalad` | Retrieve, version, and publish scientific datasets with DataLad and git-annex, and capture computational provenance with datalad run,… | — | network | yes | 52 KB | $DN |
| `lamindb` | Use when working with LaminDB, the open-source lineage-native lakehouse for biological datasets and models. | lamindb, bionty, lamindb-wetlab | unspecified | yes | 104 KB | $ |
| `modal` | Modal is a serverless cloud platform for running Python on demand, including on-demand GPUs. | modal | unspecified | yes | 96 KB | $ |
| `optimize-for-gpu` | GPU-accelerates scientific Python on NVIDIA hardware and verifies that the result is correct and faster. | — | network | no | 316 KB | DN |

### Decision & Scenario Analysis (3)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `consciousness-council` | Run a multi-perspective Mind Council deliberation on any question, decision, or creative challenge. | — | unspecified | no | 20 KB | — |
| `dhdna-profiler` | Extract cognitive patterns and thinking fingerprints from any text. | — | unspecified | no | 16 KB | — |
| `what-if-oracle` | Run structured What-If scenario analysis with 4–6 branch possibility exploration (best, likely, worst, wild card, contrarian, second-order). | — | unspecified | no | 20 KB | — |

### Document Processing & Conversion (7)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `docx` | Use this skill whenever the user wants to create, read, edit, or manipulate Word documents (.docx files) or Word templates (.dotx files). | defusedxml, lxml | unspecified | no | 1.2 MB | X |
| `liteparse` | Local document and PDF parsing that returns spatial text with bounding boxes. | liteparse | unspecified | no | 44 KB | X |
| `markdown-mermaid-writing` | Comprehensive markdown and Mermaid diagram writing skill. | — | unspecified | no | 352 KB | — |
| `markitdown` | Convert heterogeneous documents and selected URIs to Markdown with Microsoft MarkItDown for text analysis, search, and LLM/RAG ingestion. | markitdown, openai | unspecified | no | 124 KB | X |
| `pdf` | Use this skill whenever the user wants to do anything with PDF files. | pypdf, pdfplumber, pypdfium2, reportlab, pdf2image +4 | unspecified | no | 84 KB | X |
| `pptx` | Use this skill any time a .pptx or .potx file is involved in any way — as input, output, or both. | pillow, defusedxml, lxml | unspecified | no | 1.2 MB | X |
| `xlsx` | Create, edit, analyze, or convert Excel spreadsheets (.xlsx, .xlsm, .xltx) where the workbook file is the primary deliverable. | openpyxl, defusedxml, lxml | unspecified | no | 1.2 MB | X |

### Electronic Lab Notebooks (ELN) (2)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `labarchive-integration` | Securely integrate with the official LabArchives ELN REST-like API and Inventory API v1. | labapi | network | yes | 100 KB | $DNX |
| `open-notebook` | Self-hosted, open-source alternative to Google NotebookLM for AI-powered research and document analysis. | requests | unspecified | yes | 76 KB | $X |

### Engineering & Simulation (6)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `fluidsim` | Plan, configure, inspect, restart, and analyze bounded FluidSim computational-fluid-dynamics simulations with explicit numerical-validity… | fluidsim, fluidfft, pyfftw, h5py, numpy | unspecified | no | 204 KB | X |
| `lab-hardware-cad` | Design custom laboratory hardware as parametric build123d models and export fabrication-ready STEP, STL, and DXF files - microfluidic… | build123d, matplotlib (py3.12) | none (declared) | no | 196 KB | X |
| `matlab` | Build, review, migrate, and safely plan MATLAB or GNU Octave numerical workflows, including arrays, tabular/time data, tests, projects,… | h5py, scipy | unspecified | no | 208 KB | X |
| `openpiv` | Particle Image Velocimetry (PIV) analysis with OpenPIV. | openpiv, numpy, scipy, scikit-image, matplotlib | none (declared) | no | 56 KB | X |
| `simpy` | Build, inspect, test, and analyze bounded process-based discrete-event simulations with SimPy, including events, resources, interrupts,… | simpy | none (declared) | no | 172 KB | X |
| `sympy` | Use when you need exact symbolic math in Python — algebra, calculus, equation solving, symbolic linear algebra, or code generation via… | sympy, mpmath, numpy, scipy, matplotlib +2 | unspecified | no | 84 KB | — |

### Healthcare AI & Clinical Machine Learning (1)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `pyhealth` | Build clinical/healthcare deep-learning pipelines with PyHealth — loading EHR/signal/imaging datasets (MIMIC-III/IV, eICU, OMOP, SleepEDF,… | pyhealth, torch | unspecified | no | 56 KB | — |

### Laboratory Automation (2)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `ginkgo-cloud-lab` | Submit and manage protocols on Ginkgo Bioworks Cloud Lab (cloud.ginkgo.bio), a web-based interface for autonomous lab execution on… | — | unspecified | no | 80 KB | — |
| `opentrons-integration` | Author, review, migrate, simulate, and troubleshoot official Opentrons Python Protocol API v2 protocols for Flex and OT-2 robots. | opentrons (py3.12) | unspecified | no | 132 KB | X |

### Laboratory Automation & Equipment Control (1)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `pylabrobot` | Develop and review PyLabRobot lab-automation resources, liquid-handling plans, offline simulations, and supported-device integrations. | pylabrobot | none (declared) | no | 140 KB | X |

### Laboratory Information Management Systems (LIMS) & R&D Platforms (1)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `benchling-integration` | Benchling Python SDK and REST API integration for registry entities, inventory, ELN entries, workflows, Benchling Apps, and Data Warehouse… | benchling-sdk, biopython, boto3, httpx | unspecified | yes | 84 KB | $ |

### Machine Learning & Deep Learning (18)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `aeon` | This skill should be used for time series machine learning tasks including classification, regression, clustering, forecasting, anomaly… | aeon, matplotlib, numpy, scikit-learn | unspecified | no | 96 KB | — |
| `cirq` | Google quantum computing framework. | cirq, cirq-aqt, cirq-google, cirq-ionq, cirq-pasqal +3 | unspecified | yes | 96 KB | $ |
| `pennylane` | Hardware-agnostic quantum ML framework with automatic differentiation. | pennylane, pennylane-lightning, pennylane-cirq, pennylane-ionq, pennylane-qiskit +4 | unspecified | no | 116 KB | — |
| `pufferlib` | Version-aware guidance for PufferLib reinforcement-learning environments, vectorization, policies, PuffeRL training, evaluation, and safe… | gymnasium, numpy | unspecified | yes | 160 KB | $X |
| `pymc` | Bayesian modeling with PyMC. | pymc, arviz, matplotlib, numpy, pandas +1 | unspecified | no | 116 KB | X |
| `pymoo` | Multi-objective optimization framework. | pymoo, joblib, optuna, matplotlib, numpy | unspecified | no | 108 KB | X |
| `pytorch-lightning` | Deep learning framework (PyTorch Lightning / lightning package). | lightning, torch, torchvision, tensorboard, wandb +2 | unspecified | no | 160 KB | X |
| `qiskit` | Build, simulate, transpile, and execute quantum circuits with Qiskit and IBM Quantum Runtime. | qiskit[visualization], qiskit-aer, qiskit-ibm-runtime, qiskit-algorithms, qiskit-nature +8 | network | yes | 160 KB | $DNX |
| `qutip` | Simulate and audit closed and open quantum-system models with QuTiP 5, including deterministic, trajectory, steady-state, spectral, and… | qutip, qutip-qip, qutip-qtrl, qutip-jax, matplotlib +1 | none (declared) | no | 172 KB | X |
| `scikit-learn` | Machine learning in Python with scikit-learn. | scikit-learn, category-encoders, imbalanced-learn, umap-learn, matplotlib +3 | unspecified | no | 132 KB | X |
| `scikit-survival` | Build, evaluate, and audit right-censored or competing-risk survival workflows with scikit-survival, including leakage-safe preprocessing,… | scikit-survival, scikit-learn, ecos, osqp, joblib +5 | none (declared) | no | 172 KB | X |
| `shap` | Explain and audit machine-learning predictions with SHAP. | shap, matplotlib, numpy, pandas, scikit-learn | unspecified | no | 140 KB | X |
| `stable-baselines3` | Production-ready reinforcement learning algorithms (PPO, SAC, DQN, TD3, DDPG, A2C) with scikit-learn-like API. | stable-baselines3, sb3-contrib, gymnasium, numpy, torch +1 | unspecified | no | 100 KB | X |
| `statsmodels` | Statistical models library for Python. | statsmodels, scikit-learn, matplotlib, numpy, pandas +1 | unspecified | no | 128 KB | — |
| `timesfm-forecasting` | Zero-shot time series forecasting with Google's TimesFM foundation model. | timesfm, jax, flax, torch, scikit-learn +5 | unspecified | no | 2.1 MB | X |
| `torch-geometric` | PyTorch Geometric (PyG) for graph neural networks — node/link/graph classification, message passing (GCN, GAT, GraphSAGE, GIN),… | torch-geometric, torch, captum, lightning, networkx +3 | unspecified | no | 76 KB | — |
| `transformers` | Hugging Face Transformers for loading Hub models, running pipeline inference, text generation, and Trainer fine-tuning on NLP, vision,… | transformers, datasets, evaluate, accelerate, huggingface-hub +7 | unspecified | yes | 68 KB | $ |
| `umap-learn` | Use UMAP-learn for nonlinear dimensionality reduction, 2D/3D embeddings, clustering preprocessing, supervised or semi-supervised UMAP,… | umap-learn, hdbscan, matplotlib, numba, numpy +1 | unspecified | no | 44 KB | — |

### Materials Science & Chemistry (3)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `astropy` | Core Python library for astronomy and astrophysics workflows that need Astropy APIs, including units/quantities, coordinates, FITS I/O,… | astropy, matplotlib, numpy, pandas, dask | network | no | 84 KB | DN |
| `cobrapy` | Constraint-based metabolic modeling (COBRA). | cobra, matplotlib, pandas, seaborn | unspecified | no | 60 KB | — |
| `pymatgen` | Analyze, validate, convert, and transform materials structures and computed materials data with current pymatgen APIs, including local… | pymatgen==2026.5.4, pymatgen-core==2026.7.16, mp-api==0.46.4 | unspecified | yes | 184 KB | $X |

### Medical Imaging & Digital Pathology (4)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `deepspot-m` | Generate transcriptome-wide virtual spatial transcriptomics from H&E histology with DeepSpot-M. | — | unspecified | no | 24 KB | $ |
| `histolab` | Lightweight WSI tile extraction and preprocessing. | histolab, pooch (py3.11) | unspecified | no | 92 KB | — |
| `pathml` | Use PathML for local, research-only computational pathology workflows: load and tile slides, build preprocessing and QC pipelines, manage… | stdlib only (py3.12) | none (declared) | no | 168 KB | X |
| `pydicom` | Use pydicom to read, inspect, write, transform, and safely preflight local DICOM datasets and pixel data. | pydicom, numpy, pillow, pylibjpeg, pylibjpeg-libjpeg +4 | none (declared) | no | 204 KB | X |

### Microscopy & Bio-image Data (1)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `omero-integration` | Securely inspect and automate microscopy data workflows against OMERO.server with omero-py, BlitzGateway, OMERO CLI, tables, annotations,… | numpy, pillow | network | yes | 184 KB | $DNX |

### Neuroscience & Electrophysiology (3)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `bids` | Use this skill when working with Brain Imaging Data Structure (BIDS) datasets: organizing neuroscience and biomedical data (MRI, EEG, MEG,… | pybids, nibabel, pydicom, heudiconv, dcm2bids | unspecified | no | 908 KB | X |
| `neurokit2` | Use NeuroKit2 to build or audit reproducible research workflows for physiological time-series preprocessing, event/interval analysis,… | neurokit2, numpy, pandas | unspecified | no | 212 KB | X |
| `neuropixels-analysis` | Analyze Neuropixels extracellular recordings end-to-end with SpikeInterface. | spikeinterface, probeinterface, neo, matplotlib, numpy +2 | unspecified | yes | 188 KB | $X |

### Pharmacology & Pharmacometrics (1)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `pkpd-modeling` | Pharmacokinetic and pharmacodynamic modelling and simulation - non-compartmental analysis, compartmental and population PK, PK/PD and… | numpy, scipy | none (declared) | no | 364 KB | X |

### Phylogenetics & Evolutionary Biology (2)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `etetoolkit` | Analyze, manipulate, compare, annotate, and visualize phylogenetic or other hierarchical trees with ETE 4. | ete4 | network | no | 112 KB | DNX |
| `phylogenetics` | Build and analyze phylogenetic trees using MAFFT (multiple alignment), IQ-TREE 2 (maximum likelihood), and FastTree (fast NJ/ML). | ete3, matplotlib (py3.12) | unspecified | no | 36 KB | X |

### Preclinical Research & Animal Welfare (1)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `relsa-severity-assessment` | Multivariate severity assessment and humane endpoint prediction for laboratory animal studies using the RELSA (RELative Severity… | numpy, pandas, scipy, statsmodels, matplotlib | none (declared) | no | 136 KB | X |

### Protein Engineering & Design (5)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `adaptyv` | How to use the Adaptyv Bio Foundry API and Python SDK for protein experiment design, submission, and results retrieval. | — | unspecified | yes | 32 KB | $ |
| `esm` | Use when working directly with the `esm` Python SDK, ESM3 or ESMC model IDs, Forge/Biohub inference clients, or ESMFold2 folding workflows. | esm, torch, matplotlib, numpy, scikit-learn +2 | unspecified | yes | 100 KB | $ |
| `glycoengineering` | Analyze and engineer protein glycosylation. | glycoshield, pandas, requests | unspecified | no | 24 KB | — |
| `molecular-dynamics` | Run and analyze molecular dynamics simulations with OpenMM and MDAnalysis. | mdanalysis, openmm, matplotlib, numpy, pandas +1 | unspecified | no | 24 KB | — |
| `tamarind` | Access a collection of open-source molecular design and structural biology tools on the Tamarind Bio platform, via its REST API or MCP… | tamarind, requests | network | yes | 68 KB | $DN |

### Proteomics & Mass Spectrometry (2)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `matchms` | Process, clean, compare, and search tandem mass spectra with matchms. | matchms, numpy | unspecified | no | 104 KB | X |
| `pyopenms` | Complete mass spectrometry analysis platform. | pyopenms, matplotlib, numpy, pandas | unspecified | no | 188 KB | X |

### Protocol Management & Sharing (1)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `protocolsio-integration` | Read, validate, and safely export protocols.io data with current official REST/MCP contracts, or create non-executing mutation plans. | stdlib only | none (declared) | yes | 168 KB | $X |

### Regulatory & Standards Evidence Preparation (2)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `analytical-method-validation` | Plan, execute, and document validation, verification, and transfer of analytical procedures under the governing framework - ICH Q2(R2) and… | stdlib only | none (declared) | no | 228 KB | X |
| `iso-standards-readiness` | Prepares and structurally reviews readiness evidence for ISO management-system and laboratory-competence standards - ISO 13485 medical… | stdlib only | none (declared) | no | 320 KB | X |

### Research Methodology & Proposal Writing (6)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `paper-lookup` | Search 18 scholarly APIs for papers, preprints, citations, open-access full text, repository records, and journal OA status, and return… | stdlib only | network | yes | 208 KB | $DNX |
| `paperclip` | Search and read full-text biomedical papers, FDA/PMDA/EMA regulatory documents, clinical trial registries, and UniProt/PDB/ChEMBL entries… | — | network | yes | 100 KB | $DN |
| `paperzilla` | Chat with your agent about projects, recommendations, and canonical papers in Paperzilla. | — | unspecified | no | 4 KB | — |
| `research-grants` | Write competitive research proposals for NSF, NIH, DOE, DARPA, and Taiwan NSTC. | — | none (declared) | yes | 276 KB | $ |
| `research-lookup` | Compile current scholarly evidence for a scientific manuscript or research brief. | requests | network | yes | 96 KB | $DNX |
| `scholar-evaluation` | Provide qualitative-first, evidence-traceable developmental review of scholarly works and audit low-stakes research-assessment rubrics… | stdlib only | none (declared) | no | 196 KB | X |

### Scientific Communication & Publishing (11)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `bgpt-paper-search` | Search scientific papers and retrieve structured experimental data extracted from full-text studies via the BGPT MCP server. | — | network | yes | 4 KB | $DN |
| `citation-management` | Comprehensive citation management for academic research. | requests, scholarly | network | yes | 312 KB | $DNX |
| `generate-image` | Generate or edit images with AI models through the OpenRouter Image API (Gemini, Seedream, Recraft, GPT-Image, Riverflow). | stdlib only | network | yes | 60 KB | $DNX |
| `infographics` | Create professional infographics using Nano Banana Pro AI with smart iterative refinement. | requests, python-dotenv | unspecified | yes | 168 KB | $X |
| `latex-posters` | Create professional research posters in LaTeX using beamerposter, tikzposter, or baposter. | requests, python-dotenv | unspecified | yes | 268 KB | $X |
| `market-research-reports` | Build evidence-traceable market research reports and assumption-driven market sizing or forecast scenarios. | stdlib only | none (declared) | no | 240 KB | X |
| `pptx-posters` | Create and audit editable scientific posters in macro-free PowerPoint (.pptx) from author-approved local content and assets. | python-pptx, pillow, lxml | none (declared) | no | 292 KB | X |
| `pyzotero` | Interact with Zotero reference management libraries using the pyzotero Python client. | pyzotero, bibtexparser, python-dotenv | unspecified | yes | 64 KB | $ |
| `scientific-schematics` | Create publication-quality scientific diagrams using Nano Banana 2 AI with smart iterative refinement. | requests, python-dotenv | unspecified | yes | 104 KB | $X |
| `scientific-slides` | Build slide decks and presentations for research talks. | pymupdf, python-pptx, pillow, matplotlib, seaborn +4 | unspecified | yes | 416 KB | $X |
| `venue-templates` | Prepare journal manuscripts, conference papers, research posters, and grant documents using venue-specific formatting guidance and bundled… | stdlib only | unspecified | no | 460 KB | X |

### Scientific Databases & Data Access (10)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `database-lookup` | Query documented public database APIs with explicit endpoints, filters, pagination, and provenance. | zeep | unspecified | yes | 536 KB | $ |
| `depmap` | Query the Cancer Dependency Map (DepMap) for cancer cell line gene dependency scores (CRISPR Chronos), drug sensitivity data, and gene… | numpy, pandas, requests, scipy | unspecified | no | 20 KB | — |
| `folklore-variant-evidence` | Retrieve ClinGen gene-disease validity assertions for a public gene or disease, and review source-linked public evidence and literature… | — | network | no | 16 KB | DN |
| `hugging-science` | Use when the user is doing AI/ML work in a scientific domain such as biology, chemistry, physics, astronomy, climate, genomics, materials,… | transformers, datasets, huggingface-hub, accelerate, gradio-client +2 | unspecified | yes | 64 KB | $X |
| `imaging-data-commons` | Query and download public cancer imaging data from NCI Imaging Data Commons. | idc-index, duckdb, google-cloud-bigquery, pydicom, simpleitk +4 | unspecified | no | 260 KB | X |
| `ncats-arax` | Queries the NCATS Translator ARAX production API for bounded, typed, provenance-rich one-hop and endpoint-pinned two-hop biomedical… | stdlib only | unspecified | no | 108 KB | X |
| `ontology-term-resolution` | Resolve free-text scientific labels to ontology term IDs and validate existing CURIEs against the EBI Ontology Lookup Service (OLS4). | stdlib only | network | no | 116 KB | DNX |
| `pathogen-variant-surveillance` | Query live pathogen genomic surveillance data through the GenSpectrum LAPIS API to find which viral lineages are circulating now, how fast… | stdlib only | network | no | 128 KB | DNX |
| `primekg` | Query the Precision Medicine Knowledge Graph (PrimeKG) for multiscale biological data including genes, drugs, diseases, phenotypes, and… | pandas | unspecified | no | 16 KB | X |
| `usfiscaldata` | Query the U.S. | pandas, requests | unspecified | no | 76 KB | — |

### Tool Discovery & Computational Resources (2)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `autoskill` | Observe the user's screen via screenpipe, detect repeated research workflows, match them against existing scientific-agent-skills, and… | httpx, pyyaml, sentence-transformers | unspecified | yes | 76 KB | $X |
| `get-available-resources` | Detect host inventory and effective CPU, memory, disk, scheduler, container, and accelerator limits when a user asks for resource-aware… | psutil | unspecified | no | 148 KB | X |

### Web Search & Information Retrieval (2)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `exa-search` | Web toolkit powered by Exa, tuned for scientific and technical content. | exa-py, python-dotenv | network | yes | 32 KB | $DNX |
| `parallel-web` | Use Parallel CLI for web search, URL extraction, deep research, structured data enrichment, entity discovery, and recurring web monitoring. | — | network | yes | 32 KB | $DN |

### Workflow Platforms & Cloud Execution (3)

| Skill | Purpose | Runtime deps | Network/DB | Cred | Assets | Flags |
|---|---|---|---|---|---|---|
| `latchbio-integration` | Build, register, debug, and operate bioinformatics workflows on Latch using the Python SDK, CLI, Latch Data and Registry, Nextflow,… | latch | network | yes | 112 KB | $DNX |
| `nextflow` | Build, run, and debug Nextflow data pipelines and nf-core workflows end to end. | nf-core | unspecified | no | 92 KB | — |
| `pacsomatic` | Operator toolkit for nf-core/pacsomatic matched tumor-normal workflows from BAM inputs. | stdlib only | unspecified | no | 64 KB | X |
