//! Synthetic fleet and workload specs for the engine tests.
//!
//! Nodes: a dev Mac (unified memory, Metal), a Pi 5, a Cognitum Seed, a Pi
//! Zero class node (slow), an x86 box with Docker emulation, a CUDA GPU box,
//! a fake Coral TPU node and a fake TSU node. Specs are built the way a kind
//! would build them, but the engine only ever sees capabilities.

use crate::placement::capability::{AttrValue, Capability, CapabilityId, Provenance};
use crate::placement::engine::{
    Execution, ExecutionVariant, LatencyClass, Liveness, PerfTarget, PlacementFacts, Preference,
    TrustTier, WorkloadSpec,
};
use crate::placement::memory::MemoryDemand;
use crate::placement::perf;
use crate::placement::requirement::{AttrPredicate, Requirement};

pub const GIB: i64 = 1 << 30;

/// A node for tests.
#[derive(Debug, Clone)]
pub struct TestNode {
    pub id: String,
    pub caps: Vec<Capability>,
    pub liveness: Liveness,
    pub tier: TrustTier,
    pub expires: Option<u64>,
    pub load: Option<f64>,
}

impl PlacementFacts for TestNode {
    fn node_id(&self) -> &str {
        &self.id
    }
    fn capabilities(&self) -> &[Capability] {
        &self.caps
    }
    fn liveness(&self) -> Liveness {
        self.liveness
    }
    fn trust_tier(&self) -> TrustTier {
        self.tier
    }
    fn facts_expire_at_ms(&self) -> Option<u64> {
        self.expires
    }
    fn load(&self) -> Option<f64> {
        self.load
    }
}

pub fn cap(id: &str) -> Capability {
    Capability::new(CapabilityId::new(id).unwrap(), Provenance::Probed)
}

pub fn list(items: &[&str]) -> AttrValue {
    AttrValue::List(items.iter().map(|s| AttrValue::from(*s)).collect())
}

fn node(id: &str, tier: TrustTier, load: f64, caps: Vec<Capability>) -> TestNode {
    TestNode {
        id: id.to_string(),
        caps,
        liveness: Liveness::Alive,
        tier,
        expires: Some(10_000),
        load: Some(load),
    }
}

fn mem_system(free_gib: i64) -> Capability {
    cap("mem.system").with_attr("free", free_gib * GIB)
}

fn native_rt(arch: &str) -> Capability {
    cap("runtime.native").with_attr("arches_native", list(&[arch]))
}

fn feed(lan: &str) -> Capability {
    cap("feed.esp32-csi-udp").with_attr("lan_id", lan)
}

fn cycle(cog: &str, ms: f64) -> Capability {
    perf::cog_cycle_ms(cog, ms).unwrap()
}

pub fn mac() -> TestNode {
    node(
        "mac-dev",
        TrustTier::Pinned,
        0.5,
        vec![
            cap("os.macos"),
            cap("cpu.arch.aarch64"),
            cap("node.class.dev-mac"),
            cap("runtime.container.apple").with_attr("arches_native", list(&["aarch64"])),
            cap("runtime.container.docker")
                .with_attr("arches_native", list(&["aarch64"]))
                .with_attr("arches_emulated", list(&["armv7", "x86_64"])),
            cap("runtime.infer.llamacpp"),
            cap("format.gguf"),
            cap("accel.gpu.metal")
                .with_attr("unified", true)
                .with_attr("mem_bytes", 128 * GIB),
            cap("mem.unified").with_attr("free", 90 * GIB),
        ],
    )
}

pub fn pi5() -> TestNode {
    node(
        "pi5",
        TrustTier::Paired,
        0.2,
        vec![
            cap("os.linux"),
            cap("cpu.arch.aarch64"),
            cap("node.class.pi5"),
            native_rt("aarch64"),
            mem_system(6),
            feed("lan-a"),
            cycle("fall-detect", 1000.0),
        ],
    )
}

pub fn seed() -> TestNode {
    node(
        "seed",
        TrustTier::Paired,
        0.1,
        vec![
            cap("os.linux"),
            cap("cpu.arch.armv7"),
            cap("node.class.cognitum-seed"),
            native_rt("armv7"),
            mem_system(1),
            feed("lan-a"),
            cycle("fall-detect", 1100.0),
        ],
    )
}

pub fn zero() -> TestNode {
    node(
        "zero",
        TrustTier::Paired,
        0.0,
        vec![
            cap("os.linux"),
            cap("cpu.arch.armv7"),
            native_rt("armv7"),
            mem_system(1),
            feed("lan-a"),
            cycle("fall-detect", 6000.0),
        ],
    )
}

pub fn x86() -> TestNode {
    node(
        "x86",
        TrustTier::Paired,
        0.1,
        vec![
            cap("os.linux"),
            cap("cpu.arch.x86_64"),
            native_rt("x86_64"),
            cap("runtime.container.docker")
                .with_attr("arches_native", list(&["x86_64"]))
                .with_attr("arches_emulated", list(&["aarch64", "armv7"])),
            mem_system(32),
        ],
    )
}

pub fn gpu() -> TestNode {
    node(
        "gpu-box",
        TrustTier::Pinned,
        0.3,
        vec![
            cap("os.linux"),
            cap("cpu.arch.x86_64"),
            native_rt("x86_64"),
            cap("runtime.infer.llamacpp"),
            cap("format.gguf"),
            cap("accel.gpu.cuda")
                .with_attr("mem_bytes", 24 * GIB)
                .with_attr("precisions", list(&["fp16", "int4"])),
            cap("mem.vram").with_attr("free", 24 * GIB),
            mem_system(64),
        ],
    )
}

pub fn coral() -> TestNode {
    node(
        "coral",
        TrustTier::Paired,
        0.0,
        vec![
            cap("os.linux"),
            cap("cpu.arch.aarch64"),
            native_rt("aarch64"),
            cap("accel.tpu.coral")
                .with_attr("formats", list(&["tflite"]))
                .exclusive(),
            cap("format.tflite"),
            mem_system(4),
        ],
    )
}

pub fn tsu() -> TestNode {
    node(
        "tsu",
        TrustTier::Paired,
        0.0,
        vec![
            cap("os.linux"),
            cap("cpu.arch.x86_64"),
            native_rt("x86_64"),
            Capability::new(
                CapabilityId::new("accel.tsu.acme").unwrap(),
                Provenance::Claimed,
            )
            .with_attr("mem_bytes", 8 * GIB)
            .exclusive(),
            mem_system(16),
        ],
    )
}

pub fn fleet() -> Vec<TestNode> {
    vec![mac(), pi5(), seed(), zero(), x86(), gpu(), coral(), tsu()]
}

fn req(id: &str) -> Requirement {
    Requirement::exact(CapabilityId::new(id).unwrap())
}

fn variant(name: &str, execution: Execution, reqs: Vec<Requirement>) -> ExecutionVariant {
    ExecutionVariant {
        name: name.to_string(),
        execution,
        requirements: reqs,
    }
}

/// The routes a binary for `arch` can take, native first.
fn arch_routes(arch: &str) -> Vec<ExecutionVariant> {
    let rt = |prefix: &str, attr: &str| {
        Requirement::prefix(prefix)
            .unwrap()
            .with_where(AttrPredicate::has(attr, arch))
    };
    vec![
        variant(
            &format!("{arch}-native"),
            Execution::Native,
            vec![
                req(&format!("cpu.arch.{arch}")),
                rt("runtime.native", "arches_native"),
            ],
        ),
        variant(
            &format!("{arch}-container"),
            Execution::Native,
            vec![rt("runtime.container", "arches_native")],
        ),
        variant(
            &format!("{arch}-emulated"),
            Execution::Emulated,
            vec![rt("runtime.container", "arches_emulated")],
        ),
    ]
}

/// A sensor workload with binaries for `arches`, optionally tied to a feed.
pub fn sensor_spec(kind: &str, name: &str, arches: &[&str], lan: Option<&str>) -> WorkloadSpec {
    let mut s = WorkloadSpec::new(kind, name);
    s.requirements.variants = arches.iter().flat_map(|a| arch_routes(a)).collect();
    if let Some(lan) = lan {
        s.requirements
            .common
            .push(req("feed.esp32-csi-udp").with_where(AttrPredicate::eq("lan_id", lan)));
    }
    s.requirements.memory = MemoryDemand {
        host_bytes: 64 << 20,
        accel_bytes: 0,
    };
    s
}

/// fall-detect on lan-a, interactive with a 2 s cycle budget.
pub fn fall_detect() -> WorkloadSpec {
    let mut s = sensor_spec("cog", "fall-detect", &["aarch64", "armv7"], Some("lan-a"));
    s.policy.latency_class = LatencyClass::Interactive;
    s.policy.perf = Some(PerfTarget {
        id: CapabilityId::new(perf::PERF_COG_CYCLE_MS).unwrap(),
        param: "cog_id".into(),
        param_value: "fall-detect".into(),
        better: Default::default(),
        budget: Some(2000.0),
    });
    s
}

/// A GGUF model server needing a GPU and `accel_gib` of accelerator memory.
pub fn gguf_server(name: &str, accel_gib: i64) -> WorkloadSpec {
    let mut s = WorkloadSpec::new("inference", name);
    s.requirements.common = vec![
        req("runtime.infer.llamacpp"),
        req("format.gguf"),
        Requirement::prefix("accel.gpu").unwrap(),
    ];
    s.requirements.memory = MemoryDemand {
        host_bytes: GIB as u64,
        accel_bytes: (accel_gib * GIB) as u64,
    };
    s.policy.preferences = vec![Preference {
        name: "weights-present".into(),
        requirement: req("model.present")
            .with_where(AttrPredicate::has("shards", "blake3-model-a")),
        weight: 25.0,
    }];
    s
}

/// A TFLite job that needs the Coral exclusively.
pub fn coral_job() -> WorkloadSpec {
    let mut s = WorkloadSpec::new("accelerator-job", "tflite-detect");
    s.requirements.common = vec![req("accel.tpu.coral").exclusive(), req("format.tflite")];
    s
}

/// A job on a TSU that requires a measured TSU.
pub fn tsu_job() -> WorkloadSpec {
    let mut s = WorkloadSpec::new("accelerator-job", "tsu-sample");
    s.requirements.common = vec![
        Requirement::prefix("accel.tsu")
            .unwrap()
            .exclusive()
            .with_min_provenance(Provenance::Measured),
    ];
    s
}
