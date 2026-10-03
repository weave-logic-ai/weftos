//! Population test for revocation (card "Revocation cannot be bypassed and
//! is always chained"). A `WorkloadGate` that was built without the
//! revocation list never denies a revoked package, so "every place, load and
//! start path consults the list" is held by this scan of every crate's
//! production sources (`src`; test files, `#[cfg(test)]` code, `examples`,
//! `benches` and `tests` are not production gates):
//!
//! * every `WorkloadGate::new(` / `WorkloadGate::with_rules(` is followed,
//!   in the same builder chain (the next 25 lines), by `.with_revocations(`,
//!   or carries a `revocation-exempt:` comment (within 8 lines either side)
//!   that says why the subject list does not apply there;
//! * a revocation is applied, and lifted, only through the forms that name
//!   who acted (`revoke_subject_by` / `revoke_audited`, `unrevoke_subject_by`
//!   / `unrevoke_audited`): every bare `.revoke_subject(` / `.unrevoke_subject(`
//!   call in production code (outside the kernel's `revocation` module) is a
//!   failure, so the `workload.revoke` / `workload.unrevoke` record always
//!   says `revoked_by` / `unrevoked_by`.

use std::path::{Path, PathBuf};

fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

/// Production sources: `crates/*/src/**/*.rs`, minus unit-test files, with
/// the text from the first `#[cfg(test)]` on dropped.
fn production() -> Vec<(String, String)> {
    let mut files = Vec::new();
    for c in std::fs::read_dir(crates_dir()).unwrap().flatten() {
        walk(&c.path().join("src"), &mut files);
    }
    let mut out = Vec::new();
    for f in files {
        let rel = f.strip_prefix(crates_dir()).unwrap().to_string_lossy().replace('\\', "/");
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        let test_file = name.ends_with("_tests.rs")
            || name.starts_with("tests_")
            || name == "tests.rs"
            || name == "test_support.rs"
            || rel.contains("/tests/")
            || rel.contains("/fixtures/");
        if test_file {
            continue;
        }
        let text = std::fs::read_to_string(&f).unwrap();
        let prod = match text.find("#[cfg(test)]") {
            Some(i) => text[..i].to_owned(),
            None => text,
        };
        out.push((rel, prod));
    }
    out
}

fn lines_of(text: &str) -> Vec<&str> {
    text.lines().collect()
}

#[test]
fn every_production_workload_gate_consults_the_revocation_list() {
    let mut found = 0;
    let mut bad = Vec::new();
    for (path, text) in production() {
        let lines = lines_of(&text);
        for (i, l) in lines.iter().enumerate() {
            let code = l.split("//").next().unwrap_or("");
            if !(code.contains("WorkloadGate::new(") || code.contains("WorkloadGate::with_rules(")) {
                continue;
            }
            // The kernel's own doc examples and the constructor definitions.
            if code.trim_start().starts_with("pub fn") || path.ends_with("workload_governance/gate.rs") {
                continue;
            }
            found += 1;
            let window = lines[i..lines.len().min(i + 25)].join("\n");
            let around = lines[i.saturating_sub(8)..lines.len().min(i + 8)].join("\n");
            if !window.contains(".with_revocations(") && !around.contains("revocation-exempt:") {
                bad.push(format!("{path}:{}: {}", i + 1, l.trim()));
            }
        }
    }
    assert!(found >= 2, "the scan found {found} gate constructions: it is looking in the wrong place");
    assert!(
        bad.is_empty(),
        "a WorkloadGate built without the revocation list never denies a revoked \
         package. Attach it (`.with_revocations(list)`) or say why not with a \
         `revocation-exempt:` comment:\n  {}",
        bad.join("\n  ")
    );
}

#[test]
fn subject_revocations_are_only_applied_through_the_audited_list() {
    let mut calls = 0;
    let mut bad = Vec::new();
    for (path, text) in production() {
        if path.starts_with("clawft-kernel/src/revocation") {
            continue;
        }
        for (i, l) in text.lines().enumerate() {
            let code = l.split("//").next().unwrap_or("");
            for m in [".revoke_subject(", ".unrevoke_subject("] {
                if code.contains(m) {
                    calls += 1;
                    bad.push(format!("{path}:{}: {}", i + 1, l.trim()));
                }
            }
        }
    }
    assert!(
        calls == 0 && bad.is_empty(),
        "apply a revocation with revoke_subject_by / revoke_audited (and lift one with \
         unrevoke_subject_by / unrevoke_audited) so the record names who did it:\n  {}",
        bad.join("\n  ")
    );
}
