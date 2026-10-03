//! Spec validation and the workload wrapper.

use std::time::Duration;

use super::config::RestartPolicy;
use super::fakes::{spec, workload};
use super::spec::*;
use crate::workload_runtime::types::{RuntimeError, VerifiedWorkload, WorkloadSource};

fn ok() -> InferenceSpec {
    spec("coder-daily", InferFlavor::LlamaCpp, 49321)
}

#[test]
fn a_plain_spec_validates_and_names_its_endpoint() {
    let s = ok();
    s.validate().unwrap();
    assert_eq!(s.base_url().unwrap(), "http://127.0.0.1:49321");
}

#[test]
fn the_conventional_ports_are_defaults_only() {
    let s = InferenceSpec::new("r", InferFlavor::Ollama);
    assert_eq!(s.port().unwrap(), 11434);
    assert_eq!(InferFlavor::LlamaCpp.default_port(), 8090);
    assert_eq!(InferFlavor::MlxLm.default_port(), 8081);
}

#[test]
fn a_wider_than_loopback_bind_is_refused() {
    for host in ["0.0.0.0", "192.0.2.7", "::", "example.com"] {
        let mut s = ok();
        s.serve.host = Some(host.into());
        assert!(
            matches!(s.validate(), Err(RuntimeError::InvalidConfig(_))),
            "{host} must be refused"
        );
    }
    let mut s = ok();
    s.serve.host = Some("localhost".into());
    s.validate().unwrap();
}

fn args(v: &[&str]) -> InferenceSpec {
    let mut s = ok();
    s.serve.extra_args = v.iter().map(|a| a.to_string()).collect();
    s
}

#[test]
fn extra_args_are_an_allowlist_with_arity() {
    args(&[
        "--threads",
        "4",
        "--flash-attn",
        "-ngl",
        "99",
        "--temp",
        "-0.5",
    ])
    .validate()
    .unwrap();
    args(&[]).validate().unwrap();
    // Missing and surplus values.
    assert!(args(&["--threads"]).validate().is_err());
    assert!(args(&["--flash-attn", "on"]).validate().is_err());
    // A value that reads as a flag.
    assert!(args(&["--threads", "--host"]).validate().is_err());
    // mlx has its own list; Ollama takes none.
    let mut m = spec("r", InferFlavor::MlxLm, 49322);
    m.serve.extra_args = vec!["--max-tokens".into(), "512".into()];
    m.validate().unwrap();
    m.serve.extra_args = vec!["--jinja".into()];
    assert!(m.validate().is_err(), "a llama.cpp flag on mlx");
    let mut o = spec("r", InferFlavor::Ollama, 49323);
    o.serve.extra_args = vec!["--threads".into(), "2".into()];
    assert!(o.validate().is_err());
}

#[test]
fn flags_that_bypass_a_denylist_are_all_refused() {
    let bad_flags = [
        // owned by the adapter, in every spelling
        "--host",
        "--hos",
        "--ho",
        "--port",
        "--por",
        "-m",
        "-md",
        "-hf",
        "-mu",
        "--model",
        "--model-draft",
        "--draft",
        "--ctx",
        "--ctx-size",
        "-c",
        "--kv",
        // other models, files and listeners
        "--lora",
        "--lora-scaled",
        "--mmproj",
        "--api-key",
        "--api-key-file",
        "--ssl-key-file",
        "--ssl-cert-file",
        "--path",
        "--log-file",
        "--slot-save-path",
        "--chat-template-file",
        "--grammar-file",
        "--hf-repo",
        "--model-url",
    ];
    for f in bad_flags {
        assert!(args(&[f]).validate().is_err(), "{f} must be refused");
        assert!(args(&[f, "x"]).validate().is_err(), "{f} x must be refused");
        assert!(args(&["--threads", "2", f]).validate().is_err());
    }
}

#[test]
fn smuggled_and_malformed_tokens_are_refused() {
    for a in [
        "--host 0.0.0.0",
        "--host=0.0.0.0",
        "--threads=4",
        "--threads\t4",
        "bad\nline",
        "",
        "stray",
    ] {
        assert!(args(&[a]).validate().is_err(), "{a:?} must be refused");
    }
    assert!(args(&["--threads", "4 --host 0.0.0.0"]).validate().is_err());
    assert!(args(&["--threads", "$(id)"]).validate().is_err());
}

#[test]
fn memory_figures_are_capped() {
    let mut s = ok();
    s.memory.weights_bytes = u64::MAX;
    assert!(s.validate().is_err());
    let mut s = ok();
    s.memory.kv_budget_bytes = (1 << 42) + 1;
    assert!(s.validate().is_err());
    let mut s = ok();
    s.memory.weights_bytes = 1 << 42;
    s.validate().unwrap();
}

#[test]
fn identity_fields_are_tokens_and_ports_are_nonzero() {
    let mut s = ok();
    s.role = "has space".into();
    assert!(s.validate().is_err());
    let mut s = ok();
    s.model = Some("../etc/passwd".into());
    assert!(s.validate().is_err());
    let mut s = ok();
    s.serve.port = Some(0);
    assert!(s.validate().is_err());
    let mut s = spec("r", InferFlavor::MlxLm, 49322);
    s.serve.draft_model = Some("d".into());
    assert!(s.validate().is_err(), "drafts are llama.cpp only");
}

#[test]
fn the_workload_wrapper_carries_kind_spec_and_a_revocation_ref() {
    let w = workload(ok());
    assert_eq!(w.kind, KIND_INFERENCE);
    assert_eq!(w.id, "coder-daily");
    assert!(matches!(w.source, WorkloadSource::Inference(_)));
    let (pkg, keys, hashes) = w.revocation_refs();
    assert_eq!(pkg, "inference.coder-daily.adopted");
    assert!(keys.is_empty() && hashes.is_empty());
    assert!(
        w.signed("native").is_err(),
        "a signed-package adapter refuses it"
    );
    assert!(w.inference_spec("x").is_ok());
    let mut bad = ok();
    bad.serve.host = Some("0.0.0.0".into());
    assert!(VerifiedWorkload::inference(bad).is_err());
}

#[test]
fn spec_round_trips_and_rejects_unknown_fields() {
    let s = ok();
    let j = serde_json::to_string(&s).unwrap();
    assert!(j.contains("infer.llamacpp"));
    assert_eq!(serde_json::from_str::<InferenceSpec>(&j).unwrap(), s);
    let bad = j.replace("\"role\"", "\"surprise\":1,\"role\"");
    assert!(serde_json::from_str::<InferenceSpec>(&bad).is_err());
}

#[test]
fn restart_backoff_doubles_and_caps() {
    let p = RestartPolicy {
        max_restarts: 9,
        base: Duration::from_secs(2),
        cap: Duration::from_secs(10),
        ..RestartPolicy::default()
    };
    let d: Vec<u64> = (0..5).map(|i| p.delay(i).as_secs()).collect();
    assert_eq!(d, [2, 4, 8, 10, 10]);
}
