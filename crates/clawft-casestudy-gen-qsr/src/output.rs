//! Disk-serialization for the corpus.

use crate::dimensions::Dimensions;
use crate::events::DailyRollup;
use crate::ops_events::OpsEventLedger;
use crate::truth::TruthManifest;
use anyhow::Result;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;

pub fn write_dimensions(dims: &Dimensions, out_dir: &Path) -> Result<()> {
    let d = out_dir.join("dimensions");
    serde_json::to_writer_pretty(File::create(d.join("stores.json"))?, &dims.stores)?;
    serde_json::to_writer_pretty(File::create(d.join("people.json"))?, &dims.people)?;
    serde_json::to_writer_pretty(File::create(d.join("positions.json"))?, &dims.positions)?;
    serde_json::to_writer_pretty(File::create(d.join("promotions.json"))?, &dims.promotions)?;
    Ok(())
}

pub fn write_events(events: &[DailyRollup], out_dir: &Path) -> Result<()> {
    let path = out_dir.join("events").join("daily_rollups.jsonl");
    let mut w = BufWriter::new(File::create(path)?);
    for e in events {
        serde_json::to_writer(&mut w, e)?;
        w.write_all(b"\n")?;
    }
    w.flush()?;
    Ok(())
}

pub fn write_truth(truth: &TruthManifest, out_dir: &Path) -> Result<()> {
    let path = out_dir.join("truth").join("manifest.json");
    serde_json::to_writer_pretty(File::create(path)?, truth)?;
    Ok(())
}

pub fn write_ops_events(ops: &OpsEventLedger, out_dir: &Path) -> Result<()> {
    let d = out_dir.join("events");
    serde_json::to_writer_pretty(File::create(d.join("inventory.json"))?, &ops.inventory)?;
    serde_json::to_writer_pretty(File::create(d.join("audits.json"))?, &ops.audits)?;
    serde_json::to_writer_pretty(
        File::create(d.join("cert_renewals.json"))?,
        &ops.cert_renewals,
    )?;
    serde_json::to_writer_pretty(
        File::create(d.join("shift_adequacy.json"))?,
        &ops.shift_adequacy,
    )?;
    Ok(())
}

pub fn load_ops_events(corpus_dir: &Path) -> Result<OpsEventLedger> {
    let d = corpus_dir.join("events");
    let inventory = serde_json::from_reader(BufReader::new(File::open(d.join("inventory.json"))?))?;
    let audits = serde_json::from_reader(BufReader::new(File::open(d.join("audits.json"))?))?;
    let cert_renewals =
        serde_json::from_reader(BufReader::new(File::open(d.join("cert_renewals.json"))?))?;
    let shift_adequacy =
        serde_json::from_reader(BufReader::new(File::open(d.join("shift_adequacy.json"))?))?;
    Ok(OpsEventLedger {
        inventory,
        audits,
        cert_renewals,
        shift_adequacy,
    })
}

pub fn load_dimensions(corpus_dir: &Path) -> Result<Dimensions> {
    let d = corpus_dir.join("dimensions");
    let stores = serde_json::from_reader(BufReader::new(File::open(d.join("stores.json"))?))?;
    let people = serde_json::from_reader(BufReader::new(File::open(d.join("people.json"))?))?;
    let positions = serde_json::from_reader(BufReader::new(File::open(d.join("positions.json"))?))?;
    let promotions =
        serde_json::from_reader(BufReader::new(File::open(d.join("promotions.json"))?))?;
    Ok(Dimensions {
        stores,
        people,
        positions,
        promotions,
    })
}

pub fn load_events(corpus_dir: &Path) -> Result<Vec<DailyRollup>> {
    let path = corpus_dir.join("events").join("daily_rollups.jsonl");
    let mut content = String::new();
    BufReader::new(File::open(path)?).read_to_string(&mut content)?;
    let mut out = Vec::new();
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(line)?);
    }
    Ok(out)
}

pub fn load_truth(corpus_dir: &Path) -> Result<TruthManifest> {
    let path = corpus_dir.join("truth").join("manifest.json");
    Ok(serde_json::from_reader(BufReader::new(File::open(path)?))?)
}
