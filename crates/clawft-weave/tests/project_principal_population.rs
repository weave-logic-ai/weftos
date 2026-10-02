//! Population test for the verified project principal (ADR-103 A6, Phase 2
//! package I). `ProjectAttestation::from_verified` has to be `pub` (the
//! daemon verifies in another crate), so "only a `VerifiedProject` stamps a
//! principal" is held by construction path and by this test, which greps the
//! production sources:
//!
//! * `from_verified` is called only from `verified_project.rs`;
//! * `GatePrincipal::with_project` is called only where the attestation is
//!   already in hand (`governance.rs`, `governance_project.rs`);
//! * no `.project_id =` assignment exists in kernel or weave code outside
//!   `governance.rs` (the principal's field is private; this keeps it so);
//! * no `impl` of `ProjectAttestation` outside `governance.rs`, no
//!   `From`/`Deref`-style bridge into `VerifiedProject` outside
//!   `verified_project.rs`;
//! * every `GovernanceRequest` construction site in production code is in
//!   the table below, is attributed from the kernel attestation, and none
//!   reads a project from request parameters.
//!
//! "Production code" is a file's text before its first `#[cfg(test)]`,
//! minus `*_tests.rs` files and anything under a `tests/` directory.

use std::path::{Path, PathBuf};

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let p = e.path();
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        if p.is_dir() {
            if !matches!(name.as_str(), "target" | "tests" | "archive" | "benches" | "examples" | ".cargo-target") {
                rs_files(&p, out);
            }
        } else if name.ends_with(".rs") && !name.ends_with("_tests.rs") && !name.starts_with("tests_") {
            out.push(p);
        }
    }
}

/// The text before the file's test module: the first `#[cfg(test)]` whose
/// item is a `mod` (an attribute on a lone fn or import does not end
/// production code).
fn strip_test_module(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    for (i, l) in lines.iter().enumerate() {
        if l.trim() != "#[cfg(test)]" {
            continue;
        }
        let next = lines[i + 1..].iter().map(|l| l.trim()).find(|l| !l.starts_with("#["));
        if next.is_some_and(|n| n.starts_with("mod ") || n.starts_with("pub mod ") || n.starts_with("pub(crate) mod ")) {
            return lines[..i].join("\n");
        }
    }
    text.to_owned()
}

/// `(relative path under crates/, production text)` of every source file.
fn production() -> Vec<(String, String)> {
    let root = crates_dir();
    let mut files = Vec::new();
    for krate in std::fs::read_dir(&root).unwrap().flatten() {
        let src = krate.path().join("src");
        if krate.file_name() != "archive" && src.is_dir() {
            rs_files(&src, &mut files);
        }
    }
    files
        .into_iter()
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap();
            let prod = strip_test_module(&text);
            (p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/"), prod)
        })
        .collect()
}

/// Code lines only: comments (`//`, `///`, `//!`) dropped.
fn code(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines().enumerate().filter(|(_, l)| !l.trim_start().starts_with("//")).map(|(i, l)| (i + 1, l))
}

fn files_with(needle: &str, only_kernel_weave: bool) -> Vec<String> {
    production()
        .into_iter()
        .filter(|(f, _)| !only_kernel_weave || f.starts_with("clawft-kernel/") || f.starts_with("clawft-weave/"))
        .filter(|(_, t)| code(t).any(|(_, l)| l.contains(needle)))
        .map(|(f, _)| f)
        .collect()
}

#[test]
fn from_verified_is_called_only_by_verified_project_attest() {
    let mut hits = files_with("ProjectAttestation::from_verified(", false);
    hits.sort();
    assert_eq!(hits, ["clawft-weave/src/verified_project.rs"], "attestations are minted only by VerifiedProject::attest");
}

#[test]
fn with_project_is_called_only_with_an_attestation_in_hand() {
    let mut hits = files_with(".with_project(", false);
    hits.sort();
    assert_eq!(hits, ["clawft-kernel/src/governance.rs", "clawft-kernel/src/governance_project.rs"]);
}

#[test]
fn no_project_id_assignment_outside_governance() {
    for (f, t) in production() {
        if !(f.starts_with("clawft-kernel/") || f.starts_with("clawft-weave/")) || f == "clawft-kernel/src/governance.rs" {
            continue;
        }
        for (n, l) in code(&t) {
            let l = l.replace(' ', "");
            assert!(
                !(l.contains(".project_id=") && !l.contains(".project_id==")),
                "{f}:{n}: `.project_id =` outside governance.rs: {l}"
            );
        }
    }
}

#[test]
fn the_attestation_path_has_no_impls_outside_its_modules() {
    for (f, t) in production() {
        for (n, l) in code(&t) {
            let l = l.trim();
            if l.starts_with("impl") && l.contains("ProjectAttestation") {
                assert_eq!(f, "clawft-kernel/src/governance.rs", "{f}:{n}: {l}");
            }
            if l.starts_with("impl") && l.contains("for VerifiedProject") {
                panic!("{f}:{n}: trait impl into VerifiedProject: {l}");
            }
            if l.starts_with("impl") && l.contains("VerifiedProject") && l.contains("From<") {
                panic!("{f}:{n}: conversion into VerifiedProject: {l}");
            }
        }
    }
}

/// Every production `GovernanceRequest` construction site and how it takes
/// its project. Add a row when adding a site; a site with no row fails.
const NEW_SITES: &[(&str, &str)] = &[
    ("clawft-kernel/src/http_api.rs", "GovernanceRequest::new: attributed() inside new; client context keys filtered"),
    ("clawft-kernel/src/profile_store.rs", "GovernanceRequest::new"),
    ("clawft-kernel/src/hnsw_service.rs", "GovernanceRequest::new"),
    ("clawft-kernel/src/causal.rs", "GovernanceRequest::new"),
    ("clawft-kernel/src/wasm_runner/runner.rs", "GovernanceRequest::new"),
];
const LITERAL_SITES: &[&str] = &[
    "clawft-kernel/src/gate.rs",
    "clawft-kernel/src/workload_governance/gate.rs",
    "clawft-kernel/src/governance.rs", // `new` itself
];

#[test]
fn every_governance_request_site_is_enumerated_and_attributed_from_the_attestation() {
    let prod = production();
    let mut seen_new = Vec::new();
    let mut seen_literal = Vec::new();
    for (f, t) in &prod {
        if !f.starts_with("clawft-kernel/") && !f.starts_with("clawft-weave/") {
            continue;
        }
        let has_new = code(t).any(|(_, l)| l.contains("GovernanceRequest::new("));
        let has_lit = code(t).any(|(_, l)| l.contains("GovernanceRequest {") && !l.contains("struct GovernanceRequest") && !l.trim_start().starts_with("impl"));
        if has_new && f != "clawft-kernel/src/governance.rs" {
            seen_new.push(f.clone());
        }
        if has_lit {
            seen_literal.push(f.clone());
            if f != "clawft-kernel/src/governance.rs" {
                assert!(t.contains(".attributed"), "{f}: literal GovernanceRequest not attributed from the attestation");
            }
        }
    }
    seen_new.sort();
    seen_literal.sort();
    let mut want_new: Vec<_> = NEW_SITES.iter().map(|(f, _)| (*f).to_owned()).collect();
    want_new.sort();
    let mut want_lit: Vec<_> = LITERAL_SITES.iter().map(|f| (*f).to_owned()).collect();
    want_lit.sort();
    assert_eq!(seen_new, want_new, "GovernanceRequest::new sites changed: update NEW_SITES (and review them)");
    assert_eq!(seen_literal, want_lit, "GovernanceRequest literal sites changed: update LITERAL_SITES (and review them)");
}

#[test]
fn no_governance_request_site_reads_a_project_from_request_params() {
    let prod = production();
    let sites: Vec<&str> = NEW_SITES.iter().map(|(f, _)| *f).chain(LITERAL_SITES.iter().copied()).collect();
    for (f, t) in &prod {
        if !sites.contains(&f.as_str()) || f == "clawft-kernel/src/governance.rs" {
            continue;
        }
        for (n, l) in code(t) {
            for bad in ["\"project_id\"", "\"project\"", "params.project", ".project_id"] {
                // The workload gate may read the attestation's id; nothing
                // else may name a project key.
                let allowed = f.ends_with("workload_governance/gate.rs") && (l.contains("project_id") && !l.contains('"'));
                assert!(!l.contains(bad) || allowed, "{f}:{n}: governance site names a project: {l}");
            }
        }
    }
}

#[test]
fn reserved_context_keys_are_the_only_project_keys_and_are_kernel_owned() {
    let t = production()
        .into_iter()
        .find(|(f, _)| f == "clawft-kernel/src/governance_project.rs")
        .unwrap()
        .1;
    assert!(t.contains(r#"RESERVED_CONTEXT_KEYS: &[&str] = &["project_id", "instance_id"]"#));
}
