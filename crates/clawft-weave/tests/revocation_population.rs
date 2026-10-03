//! Population test for revocation (card "Revocation cannot be bypassed and
//! is always chained"). The first guarantee is the type: a `WorkloadGate` is
//! built with its revocation list (`WorkloadGate::new(.., list)`,
//! `with_rules(.., list)`), so a gate that never denies a revoked package
//! cannot be written by accident. The one way around it is the explicit
//! `WorkloadGate::exempt(.., why)`. This test is the backstop, over every
//! crate's production sources (`crates/*/src`; unit-test files and
//! `#[cfg(test)]` items are cut out, the code after them is still scanned;
//! `examples`, `benches` and `tests` are not production):
//!
//! * `exempt(` appears only in the files listed in `EXEMPT_ALLOWED`, each
//!   with the reason the subject list does not apply there;
//! * a revocation is applied, and lifted, only through the forms that name
//!   who acted (`revoke_subject_by` / `revoke_audited`, `unrevoke_subject_by`
//!   / `unrevoke_audited`): every bare `.revoke_subject(` / `.unrevoke_subject(`
//!   call in production code (outside the kernel's `revocation` module) is a
//!   failure, so the `workload.revoke` / `workload.unrevoke` record always
//!   says `revoked_by` / `unrevoked_by`.

/// Production files that may build an exempt gate, and why.
const EXEMPT_ALLOWED: &[(&str, &str)] = &[(
    "clawft-weave/src/project_supervisor/mod.rs",
    "a project workload is authorised by a project certificate, revoked through project_identity",
)];

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

/// `text` with `//` comments and every `#[cfg(test)]` item (a `mod` block, a
/// `mod x;`, a `use`, a `fn`) removed. The code after such an item is kept.
fn strip_test_items(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(|l| l.split("//").next().unwrap_or("")).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !lines[i].trim_start().starts_with("#[cfg(test)]") {
            out.push(lines[i]);
            i += 1;
            continue;
        }
        i += 1;
        // Further attributes, then the item itself.
        while i < lines.len() && lines[i].trim_start().starts_with("#[") {
            i += 1;
        }
        let (mut depth, mut opened) = (0i32, false);
        while i < lines.len() {
            let l = lines[i];
            i += 1;
            for c in l.chars() {
                match c {
                    '{' => {
                        depth += 1;
                        opened = true;
                    }
                    '}' => depth -= 1,
                    _ => {}
                }
            }
            if (opened && depth <= 0) || (!opened && l.trim_end().ends_with(';')) {
                break;
            }
        }
    }
    out.join("\n")
}

/// Production sources: `crates/*/src/**/*.rs`, minus unit-test files, with
/// `#[cfg(test)]` items cut out.
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
        out.push((rel, strip_test_items(&std::fs::read_to_string(&f).unwrap())));
    }
    out
}

#[test]
fn the_scan_cuts_test_items_but_keeps_the_code_after_them() {
    let src = "fn a() {}\n#[cfg(test)]\nuse x::y;\nfn prod() { WorkloadGate::exempt(1) }\n\
               #[cfg(test)]\nmod t {\n  fn n() { WorkloadGate::exempt(2) }\n}\nfn after() { z() }\n";
    let out = strip_test_items(src);
    assert!(out.contains("exempt(1)"), "code after a cfg(test) use is kept: {out}");
    assert!(!out.contains("exempt(2)"), "the test module is cut: {out}");
    assert!(out.contains("fn after"), "{out}");
}

#[test]
fn only_listed_production_files_build_an_exempt_workload_gate() {
    let mut bad = Vec::new();
    let mut seen = Vec::new();
    for (path, text) in production() {
        if path.ends_with("workload_governance/gate.rs") {
            continue; // the definition
        }
        for (i, l) in text.lines().enumerate() {
            if l.contains("WorkloadGate::exempt(") {
                seen.push(path.clone());
                if !EXEMPT_ALLOWED.iter().any(|(f, _)| path == *f) {
                    bad.push(format!("{path}:{}: {}", i + 1, l.trim()));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "a WorkloadGate without the revocation list never denies a revoked package. Pass the \
         list to WorkloadGate::new / with_rules, or add the file to EXEMPT_ALLOWED with the \
         reason:\n  {}",
        bad.join("\n  ")
    );
    for (f, _) in EXEMPT_ALLOWED {
        assert!(seen.iter().any(|p| p == f), "{f} is allowed but no longer builds an exempt gate: drop it");
    }
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
