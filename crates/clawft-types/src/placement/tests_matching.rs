//! Matching, unknown / `x.` ids, unified memory, perf and serde round-trip.

use super::capability::{AttrValue as V, Capability, CapabilityId, CapabilityState, Provenance};
use super::memory::{MemoryDemand, MemoryLedger, MemoryPool};
use super::perf;
use super::requirement::{AttrPredicate as P, MatchFailure, PredicateOp, Requirement};

const GIB: i64 = 1 << 30;

fn id(s: &str) -> CapabilityId {
    CapabilityId::new(s).unwrap()
}
fn cap(s: &str) -> Capability {
    Capability::new(id(s), Provenance::Probed)
}
fn req(s: &str) -> Requirement {
    Requirement::exact(id(s))
}

/// Synthetic nodes: the dev Mac, a Pi 5, and a box with an unseen accelerator.
fn mac() -> Vec<Capability> {
    vec![
        cap("cpu.arch.aarch64"),
        cap("os.macos"),
        cap("mem.system")
            .with_attr("total", 128 * GIB)
            .with_attr("free", 100 * GIB),
        cap("mem.unified")
            .with_attr("total", 128 * GIB)
            .with_attr("free", 100 * GIB),
        cap("accel.gpu.metal")
            .with_attr("unified", true)
            .with_attr("mem_bytes", 128 * GIB)
            .with_attr("cores", 40i64)
            .with_attr("formats", V::List(vec!["gguf".into(), "mlx".into()])),
        cap("runtime.container.docker")
            .with_attr("variant", "orbstack")
            .with_attr("arches_native", V::List(vec!["aarch64".into()]))
            .with_attr(
                "arches_emulated",
                V::List(vec!["armv7".into(), "x86_64".into()]),
            ),
    ]
}
fn pi5() -> Vec<Capability> {
    vec![
        cap("cpu.arch.aarch64"),
        cap("os.linux"),
        cap("runtime.native"),
    ]
}
fn odd_box() -> Vec<Capability> {
    vec![
        cap("cpu.arch.x86_64"),
        cap("sensor.lidar.velodyne").with_attr("beams", 64i64),
    ]
}

#[test]
fn exact_id_matches_only_that_id() {
    assert!(req("cpu.arch.aarch64").matches(&mac()));
    assert!(!req("cpu.arch.armv7").matches(&mac()));
    assert_eq!(
        req("cpu.arch.armv7").match_caps(&mac()),
        Err(MatchFailure::NoSuchId {
            selector: "cpu.arch.armv7".into()
        })
    );
}

#[test]
fn prefix_matches_on_segment_boundary_only() {
    let caps = vec![cap("accel.npux.fake"), cap("accel.npu.hailo")];
    let r = Requirement::prefix("accel.npu").unwrap();
    assert_eq!(r.match_caps(&caps), Ok(vec![1]));
    assert!(!r.matches(&[cap("accel.npux.fake")]));
    assert!(Requirement::prefix("accel").unwrap().matches(&mac()));
    assert!(
        Requirement::prefix("accel.gpu.metal")
            .unwrap()
            .matches(&mac())
    );
}

#[test]
fn unseen_id_matches_only_the_node_advertising_it() {
    let r = req("sensor.lidar.velodyne").with_where(P::gte("beams", 32.0));
    let nodes = [("mac", mac()), ("pi5", pi5()), ("odd", odd_box())];
    let hits: Vec<&str> = nodes
        .iter()
        .filter(|(_, c)| r.matches(c))
        .map(|(n, _)| *n)
        .collect();
    assert_eq!(hits, vec!["odd"]);
}

#[test]
fn experimental_ids_match_only_advertisers() {
    let r = req("x.hw.radar-60ghz");
    assert!(!r.matches(&mac()));
    let mut pi = pi5();
    pi.push(cap("x.hw.radar-60ghz"));
    assert!(r.matches(&pi));
    assert!(Requirement::prefix("x.hw").unwrap().matches(&pi));
    assert!(id("x.hw.radar-60ghz").is_experimental());
}

#[test]
fn each_predicate_operator() {
    let m = mac();
    assert!(
        req("runtime.container.docker")
            .with_where(P::eq("variant", "orbstack"))
            .matches(&m)
    );
    assert!(
        !req("runtime.container.docker")
            .with_where(P::eq("variant", "desktop"))
            .matches(&m)
    );
    assert!(
        req("accel.gpu.metal")
            .with_where(P::gte("mem_bytes", (64 * GIB) as f64))
            .matches(&m)
    );
    assert!(
        !req("accel.gpu.metal")
            .with_where(P::gte("mem_bytes", (256 * GIB) as f64))
            .matches(&m)
    );
    assert!(
        req("accel.gpu.metal")
            .with_where(P::lte("cores", 40.0))
            .matches(&m)
    );
    assert!(
        !req("accel.gpu.metal")
            .with_where(P::lte("cores", 39.0))
            .matches(&m)
    );
    let in_ok = P::is_in("variant", vec!["engine".into(), "orbstack".into()]);
    assert!(
        req("runtime.container.docker")
            .with_where(in_ok)
            .matches(&m)
    );
    let in_no = P::is_in("variant", vec!["engine".into()]);
    assert!(
        !req("runtime.container.docker")
            .with_where(in_no)
            .matches(&m)
    );
    assert!(
        req("runtime.container.docker")
            .with_where(P::has("arches_emulated", "armv7"))
            .matches(&m)
    );
    assert!(
        !req("runtime.container.docker")
            .with_where(P::has("arches_native", "armv7"))
            .matches(&m)
    );
    // Int attribute equals a float operand numerically.
    assert!(
        req("accel.gpu.metal")
            .with_where(P::eq("cores", 40.0))
            .matches(&m)
    );
}

#[test]
fn missing_attribute_fails_and_names_predicate() {
    let r = req("cpu.arch.aarch64").with_where(P::gte("cores", 4.0));
    assert_eq!(
        r.match_caps(&pi5()),
        Err(MatchFailure::PredicateFailed {
            attr: "cores".into(),
            op: PredicateOp::Gte
        })
    );
}

#[test]
fn provenance_minimum_is_enforced() {
    let claimed = vec![Capability::new(id("accel.npu.ane"), Provenance::Claimed)];
    let r = req("accel.npu.ane").with_min_provenance(Provenance::Probed);
    assert_eq!(
        r.match_caps(&claimed),
        Err(MatchFailure::ProvenanceTooLow {
            best: Provenance::Claimed,
            need: Provenance::Probed
        })
    );
    assert!(req("accel.npu.ane").matches(&claimed));
    assert!(Provenance::Claimed < Provenance::Probed && Provenance::Probed < Provenance::Measured);
}

#[test]
fn state_and_exclusive_rules() {
    let busy_shared = vec![cap("accel.gpu.cuda").with_state(CapabilityState::Busy)];
    assert!(req("accel.gpu.cuda").matches(&busy_shared));
    assert!(!req("accel.gpu.cuda").exclusive().matches(&busy_shared));
    let busy_excl = vec![
        cap("accel.tpu.coral")
            .exclusive()
            .with_state(CapabilityState::Busy),
    ];
    assert_eq!(
        req("accel.tpu.coral").match_caps(&busy_excl),
        Err(MatchFailure::Unavailable {
            state: CapabilityState::Busy
        })
    );
    for s in [CapabilityState::Reserved, CapabilityState::Degraded] {
        assert!(!req("store.tier.external").matches(&[cap("store.tier.external").with_state(s)]));
    }
}

#[test]
fn count_needs_distinct_capabilities() {
    let two = vec![cap("accel.gpu.cuda"), cap("accel.gpu.cuda")];
    assert_eq!(
        req("accel.gpu.cuda").with_count(2).match_caps(&two),
        Ok(vec![0, 1])
    );
    assert_eq!(
        req("accel.gpu.cuda").with_count(3).match_caps(&two),
        Err(MatchFailure::InsufficientCount { have: 2, need: 3 })
    );
}

#[test]
fn unified_memory_is_one_pool_not_double_counted() {
    let mut ledger = MemoryLedger::from_capabilities(&mac());
    assert!(ledger.is_unified());
    assert_eq!(ledger.host_free(), (100 * GIB) as u64);
    assert_eq!(ledger.vram_free(), 0, "metal mem_bytes must not be added");
    // 70 GiB of weights plus 40 GiB host: fits separately, not in one pool.
    let big = MemoryDemand {
        host_bytes: (40 * GIB) as u64,
        accel_bytes: (70 * GIB) as u64,
    };
    let short = ledger.check(big).unwrap_err();
    assert_eq!(short.pool, MemoryPool::Unified);
    assert_eq!(short.need, (110 * GIB) as u64);
    // Reservations subtract from the shared pool.
    let model = MemoryDemand {
        host_bytes: 0,
        accel_bytes: (60 * GIB) as u64,
    };
    ledger.reserve(model).unwrap();
    let job = MemoryDemand {
        host_bytes: (30 * GIB) as u64,
        accel_bytes: 0,
    };
    ledger.reserve(job).unwrap();
    assert_eq!(ledger.host_free(), (10 * GIB) as u64);
    assert!(ledger.reserve(job).is_err());
}

#[test]
fn discrete_memory_keeps_separate_pools() {
    let caps = vec![
        cap("mem.system")
            .with_attr("total", 64 * GIB)
            .with_attr("free", 60 * GIB),
        cap("mem.vram").with_attr("free", 24 * GIB),
        cap("accel.gpu.cuda").with_attr("mem_bytes", 24 * GIB),
    ];
    let mut l = MemoryLedger::from_capabilities(&caps);
    assert!(!l.is_unified());
    assert_eq!(
        l.vram_free(),
        (24 * GIB) as u64,
        "mem_bytes is not counted twice"
    );
    l.reserve(MemoryDemand {
        host_bytes: (50 * GIB) as u64,
        accel_bytes: (20 * GIB) as u64,
    })
    .unwrap();
    let e = l
        .check(MemoryDemand {
            host_bytes: 0,
            accel_bytes: (5 * GIB) as u64,
        })
        .unwrap_err();
    assert_eq!(e.pool, MemoryPool::Vram);
}

#[test]
fn measured_perf_capabilities_and_budgets() {
    let pi5 = vec![perf::cog_cycle_ms("fall-detect", 1000.0).unwrap()];
    let zero2 = vec![perf::cog_cycle_ms("fall-detect", 6000.0).unwrap()];
    assert_eq!(pi5[0].provenance, Provenance::Measured);
    let budget = perf::require_cog_cycle_within("fall-detect", 2000.0).unwrap();
    assert!(budget.matches(&pi5));
    assert!(!budget.matches(&zero2));
    assert!(
        !perf::require_cog_cycle_within("baby-cry", 2000.0)
            .unwrap()
            .matches(&pi5)
    );
    // A claimed number does not satisfy a measured budget.
    let mut claimed = pi5.clone();
    claimed[0].provenance = Provenance::Claimed;
    assert!(!budget.matches(&claimed));
    let tok = vec![perf::infer_tok_s("qwen3-coder", 42.5).unwrap()];
    assert!(
        perf::require_infer_tok_s_at_least("qwen3-coder", 30.0)
            .unwrap()
            .matches(&tok)
    );
    assert!(
        !perf::require_infer_tok_s_at_least("qwen3-coder", 50.0)
            .unwrap()
            .matches(&tok)
    );
    assert!(perf::cog_cycle_ms("", 1.0).is_err());
    assert!(perf::cog_cycle_ms("fall-detect", -1.0).is_err());
    assert!(perf::infer_tok_s("m", f64::NAN).is_err());
}

#[test]
fn serde_round_trip_json_and_toml() {
    let caps = mac();
    let js = serde_json::to_string(&caps).unwrap();
    assert_eq!(serde_json::from_str::<Vec<Capability>>(&js).unwrap(), caps);
    let r = Requirement::prefix("accel.gpu")
        .unwrap()
        .with_where(P::has("formats", "gguf"))
        .with_where(P::gte("mem_bytes", 8.0 * GIB as f64))
        .with_where(P::is_in("vendor", vec!["apple".into(), "nvidia".into()]))
        .with_count(1)
        .exclusive()
        .with_min_provenance(Provenance::Probed);
    let js = serde_json::to_value(&r).unwrap();
    assert_eq!(js["id_prefix"], "accel.gpu");
    assert_eq!(js["where"][0]["op"], "has");
    assert_eq!(serde_json::from_value::<Requirement>(js).unwrap(), r);
    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Doc {
        caps: Vec<Capability>,
        reqs: Vec<Requirement>,
    }
    let doc = Doc {
        caps,
        reqs: vec![
            r,
            perf::require_cog_cycle_within("fall-detect", 1500.0).unwrap(),
        ],
    };
    let t = toml::to_string(&doc).unwrap();
    assert_eq!(toml::from_str::<Doc>(&t).unwrap(), doc);
}

#[test]
fn records_built_in_code_that_would_not_read_back_do_not_serialize() {
    let base = || cap("mem.system");
    for bad in [
        base().with_attr("Bad Name", 1.0),
        base().with_attr("free", f64::NAN),
        base().with_attr("l", V::List(vec![V::List(vec![])])),
    ] {
        assert!(bad.validate().is_err());
        assert!(serde_json::to_string(&bad).is_err(), "{bad:?} serialized");
        assert!(toml::to_string(&bad).is_err(), "{bad:?} serialized");
    }
    assert!(base().try_with_attr("Bad Name", 1.0).is_err());
    assert!(base().try_with_attr("free", f64::INFINITY).is_err());
    let ok = base().try_with_attr("free", 1.0).unwrap();
    let js = serde_json::to_string(&ok).unwrap();
    assert_eq!(serde_json::from_str::<Capability>(&js).unwrap(), ok);

    let bad_req = req("accel.gpu.metal").with_where(P::gte("mem", f64::NAN));
    assert!(serde_json::to_string(&bad_req).is_err());
    assert!(serde_json::to_string(&P::gte("Bad", 1.0)).is_err());
    assert!(serde_json::to_string(&req("a.b").with_count(0)).is_err());
    let bad_prefix = Requirement {
        selector: super::requirement::IdSelector::Prefix("Not Valid".into()),
        ..req("a.b")
    };
    assert!(serde_json::to_string(&bad_prefix).is_err());
    assert!(
        req("a.b")
            .try_with_where(P::has("formats", V::List(vec![])))
            .is_err()
    );
    assert!(req("a.b").try_with_where(P::has("formats", "gguf")).is_ok());
}

#[test]
fn boundary_validation_rejects_malformed_input() {
    for bad in [
        "",
        "gpu",
        "Accel.gpu",
        "accel..gpu",
        "accel.gpu.",
        "accel.gpu metal",
        "a.b.c.d.e.f.g.h.i",
    ] {
        assert!(
            CapabilityId::new(bad).is_err(),
            "{bad:?} should be rejected"
        );
    }
    assert!(CapabilityId::new("x".repeat(200) + ".a").is_err());
    let bad_json = [
        r#"{"id":"accel.gpu.metal","provenance":"probed","attrs":{"mem":NaN}}"#,
        r#"{"id":"accel.gpu.metal","provenance":"probed","attrs":{"Bad":1}}"#,
        r#"{"id":"accel.gpu.metal","provenance":"probed","attrs":{"l":[[1]]}}"#,
        r#"{"id":"accel.gpu.metal","provenance":"guessed"}"#,
    ];
    for j in bad_json {
        assert!(
            serde_json::from_str::<Capability>(j).is_err(),
            "{j} should be rejected"
        );
    }
    let bad_reqs = [
        r#"{}"#,
        r#"{"id":"a.b","id_prefix":"a"}"#,
        r#"{"id":"a.b","count":0}"#,
        r#"{"id":"a.b","where":[{"attr":"x","op":"gte","value":"big"}]}"#,
        r#"{"id":"a.b","where":[{"attr":"x","op":"in","value":1}]}"#,
        r#"{"id":"a.b","where":[{"attr":"x","op":"has","value":[1]}]}"#,
    ];
    for j in bad_reqs {
        assert!(
            serde_json::from_str::<Requirement>(j).is_err(),
            "{j} should be rejected"
        );
    }
    assert!(serde_json::from_str::<Requirement>(r#"{"id_prefix":"accel"}"#).is_ok());
    let s = serde_json::from_str::<Capability>(r#"{"id":"a.b","provenance":"measured"}"#).unwrap();
    assert_eq!(s.state, CapabilityState::Available);
    assert!(!s.exclusive);
}
