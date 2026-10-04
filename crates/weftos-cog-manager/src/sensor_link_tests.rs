//! Tests for `sensor_link`: link resolution, the panel state machine, stats and docs.

use crate::client::{HostCog, HostStatus};
use crate::sensor_link::*;
use weftos_cog_market::hw::{Firmware, HwCatalog};
use weftos_cog_market::{CatalogItem, Source};

fn item(id: &str, source: Source, also: bool) -> CatalogItem {
    CatalogItem {
        id: id.into(),
        name: id.into(),
        version: "0.1.0".into(),
        category: "presence".into(),
        description: String::new(),
        source,
        signed: source == Source::WeaveLogic,
        arches: vec!["arm".into()],
        size_kb: None,
        sha256_arm: None,
        also_in_other_source: also,
    }
}
fn market(items: Vec<CatalogItem>) -> Catalog {
    Catalog { items }
}
fn host_cog(id: &str, running: bool, enabled: bool, refusal: Option<&str>) -> HostCog {
    HostCog {
        id: id.into(),
        version: "0.1.0".into(),
        running,
        enabled,
        licence_refusal: refusal.map(str::to_string),
        ..Default::default()
    }
}
fn host(cogs: Vec<HostCog>) -> HostStatus {
    HostStatus { ok: true, running: cogs.iter().filter(|c| c.running).count(), cogs, ..Default::default() }
}

#[test]
fn no_host_shows_availability_only() {
    let m = market(vec![item("ld2450-radar", Source::WeaveLogic, true)]);
    let v = cog_view("ld2450-radar", Some(&m), None, None);
    assert!(!v.connected && v.installed.is_none());
    assert_eq!(v.available.len(), 2, "dual-listed shows both sources");
    assert_eq!(v.preferred_source().unwrap().source, Source::WeaveLogic);
    assert_eq!(module_badge(&[v.clone()]), Badge::Available);
    let a = actions(&v);
    assert!(!a[0].enabled && !a[0].why.is_empty(), "install is disabled with a reason");
    assert!(software_line(&v).contains("availability only"));
}

#[test]
fn state_machine_running_stopped_refused() {
    let m = market(vec![item("c", Source::Cognitum, false)]);
    let run = cog_view("c", Some(&m), Some(&host(vec![host_cog("c", true, true, None)])), None);
    assert_eq!(run.state(), Some(CogState::Running));
    assert_eq!(module_badge(&[run.clone()]), Badge::Running);
    let acts: Vec<_> = actions(&run).into_iter().map(|a| (a.action, a.enabled)).collect();
    // "c" has no bundled guide and no known export port, so even running it has nothing to open
    assert_eq!(acts, vec![(Action::Stop, true), (Action::Configure, false), (Action::OpenGuide, false)]);

    let stopped = cog_view("c", Some(&m), Some(&host(vec![host_cog("c", false, false, None)])), None);
    assert_eq!(stopped.state(), Some(CogState::Stopped));
    assert_eq!(module_badge(&[stopped.clone()]), Badge::Installed);
    let acts: Vec<_> = actions(&stopped).into_iter().map(|a| (a.action, a.enabled)).collect();
    // "c" ships no guide, so the guide buttons are disabled with a reason
    assert_eq!(acts, vec![(Action::Start, true), (Action::Configure, false), (Action::OpenGuide, false)]);

    let refused = cog_view("c", Some(&m), Some(&host(vec![host_cog("c", false, false, Some("no_grant"))])), None);
    assert_eq!(refused.state(), Some(CogState::Refused));
    assert!(!actions(&refused)[0].enabled);
    assert!(software_line(&refused).contains("no_grant"));
}

#[test]
fn not_installed_on_connected_host_can_install() {
    let m = market(vec![item("c", Source::Cognitum, false)]);
    let v = cog_view("c", Some(&m), Some(&host(vec![])), None);
    assert_eq!(actions(&v), vec![on(Action::Install(Source::Cognitum))]);
}

#[test]
fn unpublished_cog_is_linked_only_and_has_no_actions() {
    let m = market(vec![]);
    let v = cog_view("ghost", Some(&m), Some(&host(vec![])), None);
    assert_eq!(module_badge(&[v.clone()]), Badge::Linked);
    assert!(actions(&v).is_empty());
    assert_eq!(module_badge(&[]), Badge::NoCog);
}

#[test]
fn badge_takes_the_strongest_cog() {
    let m = market(vec![item("a", Source::WeaveLogic, false), item("b", Source::WeaveLogic, false)]);
    let h = host(vec![host_cog("b", true, true, None)]);
    let views = [cog_view("a", Some(&m), Some(&h), None), cog_view("b", Some(&m), Some(&h), None)];
    assert_eq!(module_badge(&views), Badge::Running);
}

#[test]
fn bundled_catalog_resolves_ld2450_end_to_end() {
    let cat = HwCatalog::bundled();
    let m = cat.module("hlk-ld2450").unwrap();
    assert_eq!(default_export_port(&m.cogs[0]), 8052);
    let docs = doc_entries(m);
    assert!(docs.iter().any(|d| d.target == DocTarget::Guide("ld2450-radar".into())));
    assert_eq!(detect_bus(m), Bus::Uart);
    assert!(!bus_steps(Bus::Uart).is_empty());
    let views = [cog_view("ld2450-radar", None, None, None)];
    // firmware: the cog can read it once query_firmware is on, and reports it when it does
    assert_eq!(firmware_read(m, &views, None), FwRead::CanRead { cog: "ld2450-radar".into(), key: "query_firmware".into() });
    let out = CogOutput::parse(&serde_json::json!({"firmware": "V1.02.22062416"}));
    assert_eq!(firmware_read(m, &views, Some(&out)), FwRead::Reported("V1.02.22062416".into()));
}

#[test]
fn firmware_without_a_reader_is_not_supported() {
    let mut m = Module::default();
    assert_eq!(firmware_read(&m, &[], None), FwRead::NotSupported);
    m.firmware = Some(Firmware { version: "x".into(), ..Default::default() });
    assert_eq!(firmware_read(&m, &[], None), FwRead::NotSupported);
}

#[test]
fn cog_output_parses_leniently_and_stats_rows_follow() {
    let v = serde_json::json!({
        "health":"degraded","quality":0.98,"frame_rate_hz":9.8,"parse_errors":2,
        "timestamp_ms": 1_000_000u64, "reasons":["parse_errors"], "source":{"simulated":true}, "firmware": null
    });
    let o = CogOutput::parse(&v);
    assert_eq!(o.firmware, None);
    assert!(o.simulated);
    let inst = Installed { version: "1".into(), state: CogState::Running, refusal: None, uptime_s: Some(125), restarts: 1, pid: Some(7), rss_kb: Some(4096), last_exit: None };
    let rows = stat_rows(&inst, Some(&o), 1_005_000);
    let get = |k: &str| rows.iter().find(|(a, _)| a == k).map(|(_, b)| b.clone());
    assert_eq!(get("uptime").as_deref(), Some("2m"));
    assert_eq!(get("last output").as_deref(), Some("5s ago"));
    assert_eq!(get("frame rate").as_deref(), Some("9.8 Hz"));
    assert_eq!(get("parse errors").as_deref(), Some("2"));
    assert_eq!(get("pid / RSS").as_deref(), Some("7 / 4 MB"));
    assert!(get("health").unwrap().contains("parse_errors"));
    // host-only stats when the export did not answer
    assert!(stat_rows(&inst, None, 0).iter().all(|(k, _)| k != "frame rate"));
    // an empty object parses to defaults
    assert_eq!(CogOutput::parse(&serde_json::json!({})), CogOutput::default());
}

#[test]
fn short_name_drops_the_parenthetical() {
    assert_eq!(short_name("HLK-LD2450 (24 GHz position tracking radar)"), "HLK-LD2450");
    assert_eq!(short_name("KY-038"), "KY-038");
}

#[test]
fn docs_include_buy_and_sources_and_bus_detection_covers_i2c_and_analog() {
    let mut m = Module::default();
    m.datasheet = "https://x/y.pdf".into();
    m.seen_in = vec!["docs/hardware/sensors.md".into()];
    m.spec.insert("interface".into(), "analog (0-3.3 V)".into());
    assert_eq!(detect_bus(&m), Bus::Analog);
    m.spec.insert("interface".into(), "I2C 0x48".into());
    assert_eq!(detect_bus(&m), Bus::I2c);
    let d = doc_entries(&m);
    assert_eq!(d[0].target, DocTarget::Web("https://x/y.pdf".into()));
    assert!(d.iter().any(|e| e.target == DocTarget::Path("docs/hardware/sensors.md".into())));
}

/// Every catalog project that declares an export port must have the same port here, so the
/// Sensors tab finds its `/guide` without a prompt.
#[test]
fn export_ports_match_the_bundled_catalog() {
    let catalog = weftos_cog_market::hw::HwCatalog::bundled();
    for p in &catalog.projects {
        if let Some(port) = p.export_port {
            assert_eq!(default_export_port(&p.id), port, "{}", p.id);
        }
    }
    assert_eq!(default_export_port("ld2450-radar"), 8052);
    assert_eq!(default_export_port("not-a-cog"), 0);
}

#[test]
fn the_guide_is_readable_before_install_and_with_no_host() {
    // ld2450-radar ships a guide: Open guide is enabled even though nothing is installed
    let m = market(vec![item("ld2450-radar", Source::WeaveLogic, false)]);
    let v = cog_view("ld2450-radar", Some(&m), None, None);
    assert!(v.guide && v.installed.is_none() && !v.connected);
    let a: Vec<_> = actions(&v).into_iter().map(|a| (a.action, a.enabled)).collect();
    assert_eq!(a[0], (Action::Install(Source::WeaveLogic), false), "install needs a host");
    assert!(a.contains(&(Action::OpenGuide, true)));
    // stopped but guided: guide buttons enabled
    let v = cog_view("ld2450-radar", Some(&m), Some(&host(vec![host_cog("ld2450-radar", false, false, None)])), None);
    assert!(actions(&v).iter().any(|a| a.action == Action::OpenGuide && a.enabled));
}

/// Every guide bundled with the catalog parses and passes the ADR-104 validator, so a user never
/// opens a broken hook-up guide.
#[test]
fn every_bundled_guide_parses_and_validates() {
    for (id, json) in weftos_cog_market::guides::GUIDES {
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let b = weftos_sensor_guide::GuideBundle::from_json(&v).unwrap_or_else(|e| panic!("{id}: {e}"));
        let errs = b.validate();
        assert!(errs.is_empty(), "{id}: {errs:?}");
        assert!(!b.pages.is_empty(), "{id}");
    }
}

#[test]
fn the_host_reported_export_port_beats_the_built_in_map() {
    let mut c = host_cog("ld2450-radar", true, true, None);
    c.export_ports = vec![9999];
    let h = host(vec![c]);
    assert_eq!(export_port("ld2450-radar", Some(&h)), 9999);
    // a host that predates export_ports (or a stopped cog) falls back to the map
    let h = host(vec![host_cog("ld2450-radar", true, true, None)]);
    assert_eq!(export_port("ld2450-radar", Some(&h)), 8052);
    assert_eq!(export_port("ld2450-radar", None), 8052);
    assert_eq!(export_port("unknown-cog", None), 0);
    // several listeners: the declared port wins over a lower one the cog also holds
    let mut c = host_cog("ld2450-radar", true, true, None);
    c.export_ports = vec![80, 8052, 9000];
    assert_eq!(export_port("ld2450-radar", Some(&host(vec![c]))), 8052);
    // an unknown cog takes the first listener the host reports
    let mut c = host_cog("mystery", true, true, None);
    c.export_ports = vec![7000, 7001];
    assert_eq!(export_port("mystery", Some(&host(vec![c]))), 7000);
}
