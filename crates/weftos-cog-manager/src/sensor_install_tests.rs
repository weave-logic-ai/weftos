//! Tests for `sensor_install`: pre-install rows from node facts, post-install rows from output.

use crate::client::{DevList, EnabledCog, NodeFacts, UartFacts};
use crate::sensor_install::*;
use crate::sensor_link::{CogOutput, CogState, Installed};
use weftos_cog_market::hw::HwCatalog;
use weftos_cog_market::{CatalogItem, Source};

fn facts(uart: &[&str], console: Option<bool>, i2c: &[&str], usb: &[&str]) -> NodeFacts {
    NodeFacts {
        node: "cog0".into(),
        arch: "arm".into(),
        uart: UartFacts { devices: uart.iter().map(|s| s.to_string()).collect(), console_on_uart: console, console: console.filter(|c| *c).map(|_| "console=serial0,115200".to_string()) },
        i2c: DevList { devices: i2c.iter().map(|s| s.to_string()).collect() },
        usb_serial: usb.iter().map(|s| s.to_string()).collect(),
        enabled_cogs: vec![],
    }
}
fn item(arches: &[&str], signed: bool) -> CatalogItem {
    CatalogItem {
        id: "ld2450-radar".into(),
        name: "x".into(),
        version: "0.1.0".into(),
        category: "presence".into(),
        description: String::new(),
        source: if signed { Source::WeaveLogic } else { Source::Cognitum },
        signed,
        arches: arches.iter().map(|s| s.to_string()).collect(),
        size_kb: None,
        sha256_arm: None,
        also_in_other_source: false,
    }
}
fn by<'a>(c: &'a [Check], name: &str) -> &'a Check {
    c.iter().find(|x| x.name == name).unwrap_or_else(|| panic!("no check {name}: {c:?}"))
}

#[test]
fn uart_radar_on_a_node_with_the_console_on_the_uart_fails_with_a_fix() {
    let cat = HwCatalog::bundled();
    let m = cat.module("hlk-ld2450").unwrap();
    let c = preinstall(m, "ld2450-radar", Some(&item(&["arm", "arm64"], true)), Some(&facts(&["/dev/serial0"], Some(true), &[], &[])), &cat);
    assert_eq!(by(&c, "architecture").verdict, Verdict::Pass);
    assert_eq!(by(&c, "UART device").verdict, Verdict::Pass);
    let console = by(&c, "serial console");
    assert_eq!(console.verdict, Verdict::Fail);
    assert!(console.detail.contains("serial0") && console.fix.contains("owner's approval"));
    assert!(!ready(&c));
}

#[test]
fn a_clean_uart_node_is_ready_and_signed_provenance_passes() {
    let cat = HwCatalog::bundled();
    let m = cat.module("hlk-ld2450").unwrap();
    let c = preinstall(m, "ld2450-radar", Some(&item(&["arm"], true)), Some(&facts(&["/dev/serial0"], Some(false), &[], &[])), &cat);
    assert!(ready(&c), "{c:?}");
    assert_eq!(by(&c, "provenance").verdict, Verdict::Pass);
}

#[test]
fn missing_bus_and_wrong_arch_fail_and_unreadable_cmdline_is_manual() {
    let cat = HwCatalog::bundled();
    let m = cat.module("hlk-ld2450").unwrap();
    let c = preinstall(m, "ld2450-radar", Some(&item(&["arm64"], false)), Some(&facts(&[], None, &[], &[])), &cat);
    assert_eq!(by(&c, "architecture").verdict, Verdict::Fail);
    assert_eq!(by(&c, "UART device").verdict, Verdict::Fail);
    assert_eq!(by(&c, "licence / provenance").verdict, Verdict::Manual);
    // a USB-serial adapter satisfies the UART need
    let c = preinstall(m, "ld2450-radar", Some(&item(&["arm"], true)), Some(&facts(&[], None, &[], &["/dev/ttyUSB0"])), &cat);
    assert_eq!(by(&c, "UART device").verdict, Verdict::Pass);
    // on-board UART present but cmdline unreadable: not a pass, a manual step
    let c = preinstall(m, "ld2450-radar", Some(&item(&["arm"], true)), Some(&facts(&["/dev/serial0"], None, &[], &[])), &cat);
    assert_eq!(by(&c, "serial console").verdict, Verdict::Manual);
}

#[test]
fn i2c_sensor_needs_i2c_and_the_address_is_checked_by_hand() {
    let cat = HwCatalog::bundled();
    let m = cat.module("sen0628-tof").unwrap();
    let c = preinstall(m, "sen0628-tof", None, Some(&facts(&[], None, &[], &[])), &cat);
    assert_eq!(by(&c, "I2C bus").verdict, Verdict::Fail);
    let c = preinstall(m, "sen0628-tof", None, Some(&facts(&[], None, &["/dev/i2c-1"], &[])), &cat);
    assert_eq!(by(&c, "I2C bus").verdict, Verdict::Pass);
    assert_eq!(by(&c, "I2C address").verdict, Verdict::Manual);
    assert_eq!(by(&c, "architecture").verdict, Verdict::Manual, "no marketplace build to compare");
}

#[test]
fn another_uart_cog_on_the_node_is_a_conflict_warning() {
    let cat = HwCatalog::bundled();
    let m = cat.module("hlk-ld2450").unwrap();
    let mut f = facts(&["/dev/serial0"], Some(false), &[], &[]);
    f.enabled_cogs = vec![EnabledCog { id: "rd-03e".into(), args: vec![] }, EnabledCog { id: "sound-detect".into(), args: vec![] }];
    let c = preinstall(m, "ld2450-radar", Some(&item(&["arm"], true)), Some(&f), &cat);
    let w = by(&c, "port conflict");
    assert_eq!(w.verdict, Verdict::Warn);
    assert!(w.detail.contains("rd-03e") && !w.detail.contains("sound-detect"), "sound-detect is on I2C/analog, not the UART");
    assert!(ready(&c), "a warning does not block");
}

#[test]
fn no_facts_means_verify_by_hand_never_a_pass() {
    let cat = HwCatalog::bundled();
    let c = preinstall(cat.module("hlk-ld2450").unwrap(), "ld2450-radar", None, None, &cat);
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].verdict, Verdict::Manual);
}

fn inst(state: CogState) -> Installed {
    Installed { version: "0.1.0".into(), state, refusal: None, uptime_s: Some(5), restarts: 0, pid: Some(1), rss_kb: None, last_exit: None }
}

#[test]
fn post_install_distinguishes_no_source_from_a_found_sensor() {
    let none = postinstall(None, None);
    assert_eq!(none[0].verdict, Verdict::Manual);
    let stopped = postinstall(Some(&inst(CogState::Stopped)), None);
    assert_eq!(stopped.len(), 1);
    assert_eq!(stopped[0].verdict, Verdict::Fail);
    let mut refused = inst(CogState::Refused);
    refused.refusal = Some("no_grant".into());
    assert!(postinstall(Some(&refused), None)[0].detail.contains("no_grant"));

    let run = inst(CogState::Running);
    assert_eq!(postinstall(Some(&run), None)[1].verdict, Verdict::Manual);
    let nosrc = CogOutput::parse(&serde_json::json!({"health": "no_source", "reasons": ["no_bytes"], "source": {"verified": false}}));
    let r = postinstall(Some(&run), Some(&nosrc));
    assert_eq!(r[1].verdict, Verdict::Fail);
    assert!(r[1].detail.contains("no_bytes") && r[1].fix.contains("wiring"));
    let ok = CogOutput::parse(&serde_json::json!({"health": "ok", "frame_rate_hz": 9.8, "source": {"verified": true}}));
    let r = postinstall(Some(&run), Some(&ok));
    assert_eq!(r[1].verdict, Verdict::Pass);
    assert!(r[1].detail.contains("9.8"));
    let sim = CogOutput::parse(&serde_json::json!({"health": "ok", "source": {"simulated": true}}));
    assert_eq!(postinstall(Some(&run), Some(&sim))[1].verdict, Verdict::Warn);
}

#[test]
fn peer_url_reuses_the_connected_port_for_tailnet_addresses_only() {
    assert_eq!(peer_url("http://100.109.125.18:9480", "100.64.0.3").as_deref(), Some("http://100.64.0.3:9480"));
    assert_eq!(peer_url("http://127.0.0.1:9481/", "100.100.1.2").as_deref(), Some("http://100.100.1.2:9481"));
    assert_eq!(peer_url("weird", "100.64.0.9").as_deref(), Some("http://100.64.0.9:9480"));
    assert_eq!(peer_url("http://h:9480", "fd7a:115c:a1e0::7").as_deref(), Some("http://[fd7a:115c:a1e0::7]:9480"));
    // anything that is not a literal tailnet address is refused, so a token cannot be sent there
    for bad in ["10.0.0.2", "192.168.1.1", "8.8.8.8", "evil.example", "100.64.0.3/../x", "100.64.0.3:80", "", "::1", "100.200.0.1"] {
        assert_eq!(peer_url("http://100.109.125.18:9480", bad), None, "{bad}");
    }
}

#[test]
fn loopback_hosts_are_detected() {
    assert!(is_loopback_host("http://127.0.0.1:9480"));
    assert!(is_loopback_host("localhost:9480"));
    assert!(!is_loopback_host("http://100.109.125.18:9480"));
    assert_eq!(marketplace_arch("aarch64"), "arm64");
}

#[test]
fn switching_host_never_carries_the_old_token_to_the_new_one() {
    let mut tokens = std::collections::BTreeMap::new();
    let t = switch_token(&mut tokens, "http://100.64.0.2:9480", "secret-a", "http://100.64.0.3:9480");
    assert_eq!(t, "", "a node never seen before starts with no token");
    // after the user enters b's token and later switches back and forth, each host keeps its own
    tokens.insert("http://100.64.0.3:9480".into(), "secret-b".into());
    assert_eq!(switch_token(&mut tokens, "http://100.64.0.3:9480", "secret-b", "http://100.64.0.2:9480"), "secret-a");
    assert_eq!(switch_token(&mut tokens, "http://100.64.0.2:9480", "secret-a", "http://100.64.0.3:9480"), "secret-b");
    // reconnecting to the same host keeps its token
    assert_eq!(switch_token(&mut tokens, "http://x:1 ", "tok", "http://x:1"), "tok");
}
