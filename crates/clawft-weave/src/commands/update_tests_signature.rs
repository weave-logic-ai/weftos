//! Release authenticity tests for `weaver update`, against the loopback mock
//! release server signed with a throwaway key. The real release key is never
//! used, read or printed here.

use std::path::Path;
use std::process::Command;

use super::update_flow::{Opts, Outcome};
use super::update_signature::{self as signature, Trust};
use super::update_test_support::{Rel, Sign, test_key};
use super::update_tests::{Fake, OLD, World, no, run};

/// The card's "done when": someone who can replace release assets (but does
/// not hold the key) swaps an archive and republishes matching `.sha256` and
/// `sha256.sum`. The checksums agree; the signed list does not.
#[test]
fn a_tampered_but_rehashed_release_is_refused() {
    for stem in ["weftos", "clawft-cli"] {
        let w = World::receipt_install("0.8.0", "0.9.0", &Rel { sign: Sign::TamperRehash(stem), ..Rel::default() });
        let host = Fake::default();
        let (r, out) = run(&w.ctx(w.weaver(), &host, &no), Opts::default());
        let msg = format!("{:#}", r.unwrap_err());
        assert!(msg.contains("does not match the signed release"), "{stem}: {msg}\n{out}");
        assert_eq!(w.versions(), OLD, "{stem}");
        assert!(w.leftovers().is_empty(), "{stem}: {:?}", w.leftovers());
    }
}

#[test]
fn unsigned_or_badly_signed_releases_are_refused_before_any_download() {
    for (mode, why) in [
        (Sign::Unsigned, "refusing an unsigned release"),
        (Sign::NoSig, "refusing an unsigned release"),
        (Sign::WrongKey, "signature rejected"),
        (Sign::Garbage, "not hex"),
        (Sign::NoDomain, "signature rejected"),
        (Sign::OtherTag, "signed release is for v0.8.5"),
        (Sign::TamperManifest, "dist-manifest.json does not match the signed release"),
    ] {
        let w = World::receipt_install("0.8.0", "0.9.0", &Rel { sign: mode, ..Rel::default() });
        let host = Fake::default();
        let ctx = w.ctx(w.weaver(), &host, &no);
        for opts in [Opts::default(), Opts { check: true, ..Opts::default() }, Opts { force: true, ..Opts::default() }] {
            let msg = format!("{:#}", run(&ctx, opts).0.unwrap_err());
            assert!(msg.contains(why), "{why}: {msg}");
        }
        assert_eq!(w.versions(), OLD);
        assert_eq!(w.mock.asset_requests(), 0, "{why}: an archive was fetched before the signature was checked");
    }
}

#[test]
fn a_signed_release_installs_and_says_so() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel::default());
    let host = Fake::default();
    let (r, out) = run(&w.ctx(w.weaver(), &host, &no), Opts::default());
    assert!(matches!(r.unwrap(), Outcome::Installed { .. }), "{out}");
    assert!(out.contains("Signature: verified"), "{out}");
    assert!(out.contains("matches the signed release"), "{out}");
    assert!(!out.contains("WARNING"), "{out}");
}

#[test]
fn insecure_skip_signature_warns_and_still_checks_sha256() {
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel { sign: Sign::Unsigned, ..Rel::default() });
    let host = Fake::default();
    let mut ctx = w.ctx(w.weaver(), &host, &no);
    ctx.trust = Trust::Skip;
    let (r, out) = run(&ctx, Opts::default());
    assert!(matches!(r.unwrap(), Outcome::Installed { .. }), "{out}");
    assert!(out.contains("WARNING: --insecure-skip-signature"), "{out}");
    assert!(!out.contains("Signature: verified"), "{out}");

    let w = World::receipt_install("0.8.0", "0.9.0", &Rel { sign: Sign::Unsigned, corrupt: Some("weftos"), ..Rel::default() });
    let mut ctx = w.ctx(w.weaver(), &host, &no);
    ctx.trust = Trust::Skip;
    assert!(run(&ctx, Opts::default()).0.unwrap_err().to_string().contains("checksum mismatch"));
    assert_eq!(w.versions(), OLD);
}

fn tools_for_script() -> bool {
    let ok = |c: &str, a: &str| Command::new(c).arg(a).output().is_ok_and(|o| o.status.success());
    let ssl3 = Command::new("openssl")
        .arg("version")
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).starts_with("OpenSSL 3"));
    ssl3 && ok("jq", "--version") && ok("xxd", "-v") && ok("bash", "--version")
}

/// What CI produces is what weaver accepts: run the real signing script over
/// an unsigned mock release (throwaway key), publish its output, update.
#[test]
fn the_ci_signing_script_produces_what_weaver_verifies() {
    if !tools_for_script() {
        eprintln!("skipped: sign-release.sh needs OpenSSL 3, jq and xxd");
        return;
    }
    let w = World::receipt_install("0.8.0", "0.9.0", &Rel { sign: Sign::Unsigned, ..Rel::default() });
    let dir = w.root.join("artifacts");
    std::fs::create_dir_all(&dir).unwrap();
    let prefix = "/r/download/v0.9.0/";
    for (path, body) in w.mock.routes.lock().unwrap().iter() {
        if let Some(name) = path.strip_prefix(prefix) {
            std::fs::write(dir.join(name), body).unwrap();
        }
    }
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/release/sign-release.sh");
    let sign = |expect: &str| {
        Command::new("bash")
            .arg(&script)
            .arg(&dir)
            .arg("v0.9.0")
            .env("WEAVELOGIC_RELEASE_KEY", hex::encode(test_key().to_bytes()))
            .env("SIGN_RELEASE_EXPECT_PUBKEY", expect)
            .output()
            .unwrap()
    };
    // The script refuses a key other than the expected one.
    assert!(!sign(&"0".repeat(64)).status.success());
    let o = sign(&hex::encode(test_key().verifying_key().to_bytes()));
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let doc = std::fs::read(dir.join(signature::SIGNED_DOC)).unwrap();
    let sig = std::fs::read_to_string(dir.join(signature::SIGNATURE)).unwrap();
    let signed = signature::verify(&doc, &sig, &test_key().verifying_key()).unwrap();
    assert_eq!(signed.tag, "v0.9.0");
    assert!(signed.assets.contains_key("dist-manifest.json"));
    {
        let mut routes = w.mock.routes.lock().unwrap();
        routes.insert(format!("{prefix}{}", signature::SIGNED_DOC), doc);
        routes.insert(format!("{prefix}{}", signature::SIGNATURE), sig.into_bytes());
    }
    let host = Fake::default();
    let (r, out) = run(&w.ctx(w.weaver(), &host, &no), Opts::default());
    assert!(matches!(r.unwrap(), Outcome::Installed { .. }), "{out}");
    assert!(out.contains("Signature: verified"), "{out}");
}
