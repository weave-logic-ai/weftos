//! QSR synthetic corpus generator.
//!
//! Phase 0 harness for WeftOS ECC scenario reasoning. Generates deterministic,
//! reproducible synthetic restaurant-operations data (stores, people, positions,
//! promotions, daily rollups) with a parallel "truth manifest" that records the
//! ground-truth causal structure the system-under-test will be scored against.
//!
//! See `.planning/clients/qsr/weftos-implementation-analysis.md` §5.

pub mod audit;
pub mod chaos;
pub mod coherence;
pub mod config;
pub mod dashboard;
pub mod dimensions;
pub mod eml;
pub mod engine;
pub mod events;
pub mod gaps;
pub mod governance;
pub mod graph;
pub mod impulse;
pub mod ingest;
pub mod ops_events;
pub mod output;
pub mod privacy;
pub mod recall;
pub mod rng;
pub mod rollup;
pub mod scenarios;
pub mod scoring;
pub mod shard;
pub mod truth;

pub use config::{GeneratorConfig, ScaleTier};
pub use dimensions::Dimensions;
pub use events::DailyRollup;
pub use ops_events::OpsEventLedger;
pub use truth::{CounterfactualTruth, TruthManifest};

use anyhow::Result;
use std::path::Path;

/// A generated corpus: dimensions + events + ops ledger + truth manifest.
pub struct Corpus {
    pub dims: Dimensions,
    pub truth: TruthManifest,
    pub events: Vec<DailyRollup>,
    pub ops: OpsEventLedger,
}

/// Run the generator and write the corpus to `out_dir`.
pub fn generate(config: &GeneratorConfig, out_dir: &Path) -> Result<Corpus> {
    std::fs::create_dir_all(out_dir.join("dimensions"))?;
    std::fs::create_dir_all(out_dir.join("events"))?;
    std::fs::create_dir_all(out_dir.join("truth"))?;

    let dims = dimensions::generate(config);
    let events = events::generate(config, &dims);
    let ops = ops_events::generate(config, &dims, &events);
    let truth = truth::build(config, &dims);

    output::write_dimensions(&dims, out_dir)?;
    output::write_events(&events, out_dir)?;
    output::write_ops_events(&ops, out_dir)?;
    output::write_truth(&truth, out_dir)?;

    Ok(Corpus {
        dims,
        events,
        ops,
        truth,
    })
}
