//! Acceptance tests for ADR-105: config, the three source kinds, licensing,
//! resolution, the catalog. Fixtures only; no network, no real keys.

use std::path::PathBuf;

use ed25519_dalek::Signer;

use crate::catalog::{build_rows, parse_cog_toml, Access, Expectations, RunMode};
use crate::config::{load_effective, CogSource, EffectiveSources, LicensedCogs, SourceKind, SourcesFile};
use crate::error::SourceError;
use crate::fetch::FsReader;
use crate::install::{fetch_verified, install_into_host, read_provenance, FetchCtx};
use crate::resolve::{load_all, load_source, parse_ref, resolve};
use crate::testkit::*;
use weftos_cog_repo::WEAVELOGIC_PUBKEY_HEX;

fn eff(sources: Vec<CogSource>, licences: Vec<crate::config::CogLicence>) -> EffectiveSources {
    EffectiveSources { sources, licences }
}

fn ctx<'a>(reader: &'a dyn crate::fetch::Reader, eff: &'a EffectiveSources) -> FetchCtx<'a> {
    FetchCtx { reader, licences: &eff.licences, now: now(), extra_weftos_keys: &[] }
}

// ── config ────────────────────────────────────────────────────────────────

#[test]
fn config_parses_all_three_kinds_and_round_trips() {
    let k = pub_hex(&key(1));
    let text = format!(
        r#"
[[cog_source]]
name = "weftos"
kind = "weftos"
url = "https://example.invalid/weftos/registry.json"
priority = 10

[[cog_source]]
name = "cognitum"
kind = "cognitum"
url = "https://example.invalid/app-registry.json"
enabled = false

[[cog_source]]
name = "acme-private"
kind = "private"
url = "/srv/acme-cogs"
pinned_keys = ["{k}"]
priority = 50

[[cog_licence]]
source = "cognitum"
cogs = ["fall-detect", "baby-cry"]
account = "acme"
expires = "2027-01-31"

[[cog_licence]]
source = "cognitum"
cogs = "all"
"#
    );
    let f = SourcesFile::parse(&text).unwrap();
    assert_eq!(f.cog_source.len(), 3);
    assert_eq!(f.cog_source[0].kind, SourceKind::Weftos);
    assert!(!f.cog_source[1].enabled);
    assert!(f.cog_source[2].enabled, "enabled defaults to true");
    assert_eq!(f.cog_source[2].pinned_keys, vec![k]);
    assert!(matches!(f.cog_licence[1].cogs, LicensedCogs::All(_)));
    assert_eq!(SourcesFile::parse(&f.to_toml().unwrap()).unwrap(), f);
}

#[test]
fn config_rejects_bad_input() {
    let bad = |t: &str| SourcesFile::parse(t).unwrap_err();
    // private without a key
    assert!(bad("[[cog_source]]\nname=\"p\"\nkind=\"private\"\nurl=\"/x\"\n").to_string().contains("pin at least one key"));
    // non-hex / short key
    assert!(bad("[[cog_source]]\nname=\"p\"\nkind=\"private\"\nurl=\"/x\"\npinned_keys=[\"abcd\"]\n").to_string().contains("not a 64-hex"));
    // ':' in a name would break namespacing
    assert!(bad("[[cog_source]]\nname=\"a:b\"\nkind=\"weftos\"\nurl=\"/x\"\n").to_string().contains("bad source name"));
    // duplicate names
    let dup = "[[cog_source]]\nname=\"a\"\nkind=\"weftos\"\nurl=\"/x\"\n[[cog_source]]\nname=\"a\"\nkind=\"weftos\"\nurl=\"/y\"\n";
    assert!(bad(dup).to_string().contains("duplicate"));
    // unknown field (typo) and bad expiry and bad licence coverage
    assert!(matches!(bad("[[cog_source]]\nname=\"a\"\nkind=\"weftos\"\nurl=\"/x\"\npriorty=1\n"), SourceError::Config(_)));
    assert!(bad("[[cog_licence]]\nsource=\"cognitum\"\ncogs=\"all\"\nexpires=\"next year\"\n").to_string().contains("expiry"));
    assert!(bad("[[cog_licence]]\nsource=\"cognitum\"\ncogs=\"some\"\n").to_string().contains("\"all\" or a list"));
    assert!(bad("[[cog_licence]]\nsource=\"cognitum\"\ncogs=[]\n").to_string().contains("empty"));
    assert_eq!(bad("[[cog_source]]\nname=\"a\"\nkind=\"nope\"\nurl=\"/x\"\n").code(), "config_invalid");
}

#[test]
fn project_overlays_user_defaults_and_can_disable() {
    let tmp = tempfile::tempdir().unwrap();
    let up = tmp.path().join("user.toml");
    let pp = tmp.path().join("proj.toml");
    std::fs::write(&up, "[[cog_source]]\nname=\"weftos\"\nkind=\"weftos\"\nurl=\"/user-wl\"\n[[cog_source]]\nname=\"cognitum\"\nkind=\"cognitum\"\nurl=\"/user-cg\"\n[[cog_licence]]\nsource=\"cognitum\"\ncogs=\"all\"\n").unwrap();
    std::fs::write(&pp, "[[cog_source]]\nname=\"cognitum\"\nkind=\"cognitum\"\nurl=\"/proj-cg\"\nenabled=false\n").unwrap();
    let e = load_effective(Some(&up), Some(&pp)).unwrap();
    assert_eq!(e.sources.len(), 2);
    assert_eq!(e.source("weftos").unwrap().url, "/user-wl", "user-only entry kept");
    let cg = e.source("cognitum").unwrap();
    assert_eq!((cg.url.as_str(), cg.enabled), ("/proj-cg", false), "project entry replaces the user one");
    assert_eq!(e.enabled().count(), 1);
    assert_eq!(e.licences.len(), 1, "user licence still applies");
    // missing files are empty
    assert!(load_effective(Some(&tmp.path().join("none.toml")), None).unwrap().sources.is_empty());
}

#[test]
fn edit_helpers_add_remove_enable_and_save() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join(".weftos").join("cog-sources.toml");
    let mut f = SourcesFile::load(&path).unwrap();
    f.add_source(source("wl", SourceKind::Weftos, &PathBuf::from("/x"), &[], 0)).unwrap();
    assert!(f.add_source(source("wl", SourceKind::Weftos, &PathBuf::from("/y"), &[], 0)).is_err());
    f.set_enabled("wl", false).unwrap();
    f.add_licence(licence("cognitum", LicensedCogs::All("all".into()), None)).unwrap();
    f.add_licence(licence("cognitum", LicensedCogs::All("all".into()), Some("2030-01-01"))).unwrap();
    assert_eq!(f.cog_licence.len(), 1, "same coverage replaces");
    f.save(&path).unwrap();
    let back = SourcesFile::load(&path).unwrap();
    assert_eq!(back, f);
    assert!(!back.cog_source[0].enabled);
    let mut back = back;
    back.remove_source("wl").unwrap();
    assert_eq!(back.remove_source("wl").unwrap_err().code(), "unknown_source");
    back.remove_licences("cognitum").unwrap();
    assert!(back.remove_licences("cognitum").is_err());
}

#[test]
fn effective_keys_follow_the_kind() {
    let k = pub_hex(&key(3));
    let extra = pub_hex(&key(4));
    let w = source("w", SourceKind::Weftos, &PathBuf::from("/x"), &[k.clone()], 0);
    let keys = w.effective_keys(&[extra.clone()]);
    assert_eq!(keys, vec![WEAVELOGIC_PUBKEY_HEX.to_string(), extra, k.clone()]);
    let p = source("p", SourceKind::Private, &PathBuf::from("/x"), &[k.clone()], 0);
    assert_eq!(p.effective_keys(&[pub_hex(&key(4))]), vec![k], "a private source never trusts the WeaveLogic key implicitly");
    let c = source("c", SourceKind::Cognitum, &PathBuf::from("/x"), &[], 0);
    assert!(c.effective_keys(&[]).is_empty());
}

// ── weftos + private sources (signed) ─────────────────────────────────────

#[test]
fn weftos_source_installs_when_signed_by_a_pinned_key() {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(5);
    let reg = signed_repo(&tmp.path().join("wl"), &k, &["fall-detect"]);
    let s = source("weftos", SourceKind::Weftos, &reg, &[pub_hex(&k)], 0);
    let e = eff(vec![s.clone()], vec![]);
    let l = load_source(&s, &FsReader).unwrap();
    let f = fetch_verified(&l, "fall-detect", "arm", &ctx(&FsReader, &e)).unwrap();
    assert_eq!(f.bytes, bin("fall-detect"));
    assert_eq!(f.provenance.trust, "ed25519-signed");
    assert_eq!(f.provenance.signer_pubkey.as_deref(), Some(pub_hex(&k).as_str()));
    assert!(f.provenance.placement_eligible);

    // The same repo signed by an unpinned key is refused.
    let other = source("weftos", SourceKind::Weftos, &reg, &[], 0);
    let l2 = load_source(&other, &FsReader).unwrap();
    let err = fetch_verified(&l2, "fall-detect", "arm", &ctx(&FsReader, &e)).unwrap_err();
    assert_eq!(err.code(), "verify_failed", "{err}");
}

#[test]
fn private_source_refuses_wrong_key_unsigned_and_hash_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let mine = key(7);
    let theirs = key(8);
    let dir = tmp.path().join("priv");
    let reg = signed_repo(&dir, &mine, &["acme-gauge"]);
    let e = eff(vec![], vec![]);

    // good: pinned to the project's own key
    let good = source("acme-private", SourceKind::Private, &reg, &[pub_hex(&mine)], 0);
    let l = load_source(&good, &FsReader).unwrap();
    let f = fetch_verified(&l, "acme-gauge", "arm", &ctx(&FsReader, &e)).unwrap();
    assert_eq!(f.provenance.source, "acme-private");
    assert_eq!(f.provenance.kind, "private");
    assert!(f.provenance.signer_key_id.as_deref().unwrap().starts_with("ed25519:"));

    // wrong key pinned: the repo is signed by `mine`, the project pins `theirs`
    let wrong = source("acme-private", SourceKind::Private, &reg, &[pub_hex(&theirs)], 0);
    let l = load_source(&wrong, &FsReader).unwrap();
    let err = fetch_verified(&l, "acme-gauge", "arm", &ctx(&FsReader, &e)).unwrap_err();
    assert!(err.to_string().contains("not from any key pinned"), "{err}");

    // unsigned: signature removed from the registry
    let mut r: weftos_cog_repo::Registry = serde_json::from_slice(&std::fs::read(&reg).unwrap()).unwrap();
    r.cogs[0].artifacts.get_mut("arm").unwrap().sig.clear();
    let unsigned_dir = tmp.path().join("unsigned");
    std::fs::create_dir_all(unsigned_dir.join("cogs/arm")).unwrap();
    std::fs::copy(dir.join("cogs/arm/cog-acme-gauge-arm"), unsigned_dir.join("cogs/arm/cog-acme-gauge-arm")).unwrap();
    std::fs::write(unsigned_dir.join("registry.json"), serde_json::to_vec(&r).unwrap()).unwrap();
    let us = source("acme-private", SourceKind::Private, &unsigned_dir, &[pub_hex(&mine)], 0);
    let l = load_source(&us, &FsReader).unwrap();
    assert_eq!(fetch_verified(&l, "acme-gauge", "arm", &ctx(&FsReader, &e)).unwrap_err().code(), "unsigned");

    // hash mismatch: binary changed after signing (same length so size passes)
    let mut tampered = bin("acme-gauge");
    let n = tampered.len();
    tampered[n - 1] ^= 1;
    std::fs::write(dir.join("cogs/arm/cog-acme-gauge-arm"), &tampered).unwrap();
    let l = load_source(&good, &FsReader).unwrap();
    let err = fetch_verified(&l, "acme-gauge", "arm", &ctx(&FsReader, &e)).unwrap_err();
    assert_eq!(err.code(), "verify_failed");
    assert!(err.to_string().contains("sha256"), "{err}");

    // a tampered binary with a fixed-up sha is still caught by the signature
    let mut r: weftos_cog_repo::Registry = serde_json::from_slice(&std::fs::read(&reg).unwrap()).unwrap();
    r.cogs[0].artifacts.get_mut("arm").unwrap().sha256 = weftos_cog_repo::sha256_hex(&tampered);
    std::fs::write(&reg, serde_json::to_vec(&r).unwrap()).unwrap();
    let l = load_source(&good, &FsReader).unwrap();
    let err = fetch_verified(&l, "acme-gauge", "arm", &ctx(&FsReader, &e)).unwrap_err();
    assert!(err.to_string().contains("signature"), "{err}");

    // a missing arch is its own error
    assert_eq!(fetch_verified(&l, "acme-gauge", "arm64", &ctx(&FsReader, &e)).unwrap_err().code(), "no_artifact");
}

// ── cognitum source + licence ─────────────────────────────────────────────

#[test]
fn cognitum_is_listable_but_unlicensed_install_is_refused_before_any_download() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = cognitum_repo(&tmp.path().join("cg"), &["baby-cry", "fall-detect"]);
    let s = source("cognitum", SourceKind::Cognitum, &reg, &[], 0);
    let e = eff(vec![s.clone()], vec![]);
    let l = load_source(&s, &FsReader).unwrap();
    assert_eq!(l.cogs().len(), 2, "always listable");

    let reader = CountingReader { needle: "cog-baby-cry-arm".into(), hits: Default::default() };
    let err = fetch_verified(&l, "baby-cry", "arm", &ctx(&reader, &e)).unwrap_err();
    assert_eq!(err.code(), "cog_unlicensed", "{err}");
    assert_eq!(*reader.hits.borrow(), 0, "refused without downloading the binary");
}

#[test]
fn licensed_cognitum_cog_installs_with_sha256_and_is_not_placement_eligible() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = cognitum_repo(&tmp.path().join("cg"), &["baby-cry", "fall-detect"]);
    let s = source("cognitum", SourceKind::Cognitum, &reg, &[], 0);
    let lic = licence("cognitum", LicensedCogs::List(vec!["baby-cry".into()]), Some("2026-12-31"));
    let e = eff(vec![s.clone()], vec![lic]);
    let l = load_source(&s, &FsReader).unwrap();

    let f = fetch_verified(&l, "baby-cry", "arm", &ctx(&FsReader, &e)).unwrap();
    assert_eq!(f.provenance.trust, "cognitum-sha256");
    assert_eq!(f.provenance.licence_account.as_deref(), Some("acct-test"));
    assert!(!f.provenance.placement_eligible, "ADR-100 6.3: upstream binaries need operator hash+sign for placement");
    assert!(f.provenance.signer_pubkey.is_none());

    // licence covers baby-cry only
    assert_eq!(fetch_verified(&l, "fall-detect", "arm", &ctx(&FsReader, &e)).unwrap_err().code(), "cog_unlicensed");

    // a binary that does not match the registry hash is refused even when licensed
    std::fs::write(tmp.path().join("cg/cogs/arm/cog-baby-cry-arm"), b"swapped").unwrap();
    assert_eq!(fetch_verified(&l, "baby-cry", "arm", &ctx(&FsReader, &e)).unwrap_err().code(), "verify_failed");
}

#[test]
fn expired_all_and_boundary_licences() {
    let tmp = tempfile::tempdir().unwrap();
    let reg = cognitum_repo(&tmp.path().join("cg"), &["baby-cry"]);
    let s = source("cognitum", SourceKind::Cognitum, &reg, &[], 0);
    let l = load_source(&s, &FsReader).unwrap();
    let check = |lics: Vec<crate::config::CogLicence>| {
        let e = eff(vec![s.clone()], lics);
        fetch_verified(&l, "baby-cry", "arm", &ctx(&FsReader, &e)).map(|_| ())
    };
    // now() is 2026-10-02 12:00 UTC
    let all = || LicensedCogs::All("all".into());
    assert_eq!(check(vec![licence("cognitum", all(), Some("2026-10-01"))]).unwrap_err().code(), "licence_expired");
    assert!(check(vec![licence("cognitum", all(), Some("2026-10-02"))]).is_ok(), "valid through the end of the expiry day");
    assert!(check(vec![licence("cognitum", all(), None)]).is_ok(), "no expiry");
    assert!(check(vec![licence("cognitum", all(), Some("2026-10-02T11:59:59Z"))]).is_err());
    // an expired licence next to a valid one: the valid one wins
    assert!(check(vec![licence("cognitum", all(), Some("2020-01-01")), licence("cognitum", all(), Some("2030-01-01"))]).is_ok());
    // a licence for another source never applies
    assert_eq!(check(vec![licence("elsewhere", all(), None)]).unwrap_err().code(), "cog_unlicensed");
}

// ── resolution ────────────────────────────────────────────────────────────

struct World {
    _tmp: tempfile::TempDir,
    eff: EffectiveSources,
}

fn world(priorities: (i32, i32, i32)) -> World {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(11);
    let wl = signed_repo(&tmp.path().join("wl"), &k, &["fall-detect", "bridge"]);
    let pv = signed_repo(&tmp.path().join("pv"), &k, &["fall-detect", "acme-gauge"]);
    let cg = cognitum_repo(&tmp.path().join("cg"), &["fall-detect", "sleep-apnea"]);
    let eff = eff(
        vec![
            source("weftos", SourceKind::Weftos, &wl, &[pub_hex(&k)], priorities.0),
            source("acme-private", SourceKind::Private, &pv, &[pub_hex(&k)], priorities.1),
            source("cognitum", SourceKind::Cognitum, &cg, &[], priorities.2),
        ],
        vec![],
    );
    World { _tmp: tmp, eff }
}

#[test]
fn namespaced_ids_pick_the_named_source() {
    let w = world((0, 0, 0));
    let ls = load_all(&w.eff, &FsReader);
    assert!(ls.failures.is_empty());
    for (src, kind) in [("weftos", true), ("acme-private", true), ("cognitum", false)] {
        let r = resolve(&w.eff, &ls.loaded, &parse_ref(&format!("{src}:fall-detect")).unwrap()).unwrap();
        assert_eq!(r.namespaced(), format!("{src}:fall-detect"));
        assert_eq!(r.cog.signed, kind);
    }
    assert_eq!(resolve(&w.eff, &ls.loaded, &parse_ref("nope:fall-detect").unwrap()).unwrap_err().code(), "unknown_source");
    assert_eq!(resolve(&w.eff, &ls.loaded, &parse_ref("weftos:acme-gauge").unwrap()).unwrap_err().code(), "cog_not_found");
}

#[test]
fn bare_ids_resolve_uniquely_by_priority_and_ambiguity_is_explicit() {
    let w = world((0, 0, 0));
    let ls = load_all(&w.eff, &FsReader);
    // listed by exactly one source: fine
    assert_eq!(resolve(&w.eff, &ls.loaded, &parse_ref("bridge").unwrap()).unwrap().namespaced(), "weftos:bridge");
    assert_eq!(resolve(&w.eff, &ls.loaded, &parse_ref("sleep-apnea").unwrap()).unwrap().namespaced(), "cognitum:sleep-apnea");
    // listed by three sources at equal priority: refused, and the error says how to fix it
    let err = resolve(&w.eff, &ls.loaded, &parse_ref("fall-detect").unwrap()).unwrap_err();
    assert_eq!(err.code(), "ambiguous");
    let msg = err.to_string();
    assert!(msg.contains("acme-private:fall-detect") && msg.contains("cognitum:fall-detect") && msg.contains("weftos:fall-detect"), "{msg}");

    // priority breaks the tie: private 50 beats weftos 10
    let w = world((10, 50, 0));
    let ls = load_all(&w.eff, &FsReader);
    assert_eq!(resolve(&w.eff, &ls.loaded, &parse_ref("fall-detect").unwrap()).unwrap().namespaced(), "acme-private:fall-detect");

    // a tie at the top is still ambiguous even if a lower-priority source also lists it
    let w = world((50, 50, 0));
    let ls = load_all(&w.eff, &FsReader);
    match resolve(&w.eff, &ls.loaded, &parse_ref("fall-detect").unwrap()).unwrap_err() {
        SourceError::Ambiguous { sources, .. } => assert_eq!(sources, vec!["acme-private", "weftos"]),
        e => panic!("{e}"),
    }
}

#[test]
fn disabled_sources_are_skipped_and_one_bad_source_does_not_hide_the_rest() {
    let mut w = world((0, 0, 0));
    w.eff.sources.iter_mut().find(|s| s.name == "acme-private").unwrap().enabled = false;
    w.eff.sources.iter_mut().find(|s| s.name == "cognitum").unwrap().enabled = false;
    let ls = load_all(&w.eff, &FsReader);
    assert_eq!(ls.loaded.len(), 1);
    assert_eq!(resolve(&w.eff, &ls.loaded, &parse_ref("fall-detect").unwrap()).unwrap().namespaced(), "weftos:fall-detect");
    assert_eq!(resolve(&w.eff, &ls.loaded, &parse_ref("acme-private:fall-detect").unwrap()).unwrap_err().code(), "source_disabled");

    w.eff.sources.push(source("broken", SourceKind::Weftos, &PathBuf::from("/definitely/not/here"), &[], 0));
    let ls = load_all(&w.eff, &FsReader);
    assert_eq!(ls.loaded.len(), 1);
    assert_eq!(ls.failures.len(), 1);
    assert_eq!(ls.failures[0].0, "broken");
    assert_eq!(ls.failures[0].1.code(), "fetch_failed");

    assert!(parse_ref("a:b:c").is_err());
    assert!(parse_ref("Bad_ID").is_err());
    assert!(parse_ref(":x").is_err());
}

#[test]
fn network_urls_are_refused_by_the_filesystem_reader() {
    let s = source("x", SourceKind::Weftos, &PathBuf::from("https://example.invalid/r"), &[], 0);
    let err = load_source(&s, &FsReader).unwrap_err();
    assert_eq!(err.code(), "fetch_failed");
    assert!(err.to_string().contains("network access is not enabled"));
}

// ── install + provenance ──────────────────────────────────────────────────

#[test]
fn install_writes_binary_record_and_provenance() {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(12);
    let reg = signed_repo(&tmp.path().join("pv"), &k, &["acme-gauge"]);
    let s = source("acme-private", SourceKind::Private, &reg, &[pub_hex(&k)], 0);
    let e = eff(vec![s.clone()], vec![]);
    let l = load_source(&s, &FsReader).unwrap();
    let f = fetch_verified(&l, "acme-gauge", "arm", &ctx(&FsReader, &e)).unwrap();

    let root = tmp.path().join("host");
    let rec = install_into_host(&root, &f, true, &["--interval".into(), "1".into()]).unwrap();
    assert_eq!(rec.id, "acme-gauge");
    assert!(rec.enabled && rec.signed);
    assert_eq!(std::fs::read(root.join("acme-gauge/cog-acme-gauge-arm")).unwrap(), bin("acme-gauge"));
    let p = read_provenance(&root, "acme-gauge").unwrap();
    assert_eq!(p.source, "acme-private");
    assert_eq!(p.signer_pubkey.as_deref(), Some(pub_hex(&k).as_str()));
    let ev = p.chain_payload();
    assert_eq!(ev["kind"], "cog.source.resolved");
    assert_eq!(ev["provenance"]["source"], "acme-private");
    assert!(!ev.to_string().contains("BEGIN"), "no key material in the event");
    assert_eq!(weftos_cog_host::load_records(&root).len(), 1);
}

// ── catalog ───────────────────────────────────────────────────────────────

const EXPECTATIONS: &str = include_str!("../../../scripts/cogs/expectations.json");

#[test]
fn committed_baseline_reproduces_93_5_9() {
    let ex = Expectations::parse(EXPECTATIONS.as_bytes()).unwrap();
    let s = ex.summary();
    assert_eq!((s.clean, s.needs_interval, s.needs_extra_cli, s.no_build), (93, 5, 9, 1));
    assert_eq!(ex.cogs.len(), 108);
}

#[test]
fn cog_toml_gives_resources_secrets_and_hardware() {
    let m = parse_cog_toml(
        "[cog]\nid=\"x\"\nhardware_requirement=[\"pi-zero-2w\",\"v0-appliance\"]\n[resources]\nram_mb=64\ncpu_pct=25\n[config.api_key]\ntype=\"string\"\nsecret=true\n[config.interval]\ntype=\"integer\"\n",
    )
    .unwrap();
    assert_eq!((m.ram_mb, m.cpu_pct), (Some(64), Some(25)));
    assert_eq!(m.secrets, vec!["api_key"]);
    assert_eq!(m.hardware_requirement, vec!["pi-zero-2w", "v0-appliance"]);
    assert!(parse_cog_toml("not = [valid").is_err());
}

#[test]
fn catalog_lists_cogs_across_sources_with_run_mode_policy_and_access() {
    let tmp = tempfile::tempdir().unwrap();
    let k = key(13);
    let wl = signed_repo(&tmp.path().join("wl"), &k, &["fall-detect", "swarm-deploy"]);
    let cg = cognitum_repo(&tmp.path().join("cg"), &["sleep-apnea", "baby-cry", "swarm-deploy"]);
    let toml_dir = tmp.path().join("tomls");
    std::fs::create_dir_all(toml_dir.join("fall-detect")).unwrap();
    std::fs::write(
        toml_dir.join("fall-detect/cog.toml"),
        "[cog]\nid=\"fall-detect\"\n[resources]\nram_mb=48\ncpu_pct=20\n[config.token]\nsecret=true\n",
    )
    .unwrap();
    let e = eff(
        vec![
            source("weftos", SourceKind::Weftos, &wl, &[pub_hex(&k)], 0),
            source("cognitum", SourceKind::Cognitum, &cg, &[], 0),
        ],
        vec![licence("cognitum", LicensedCogs::List(vec!["baby-cry".into()]), None)],
    );
    let ls = load_all(&e, &FsReader);
    let ex = Expectations::parse(EXPECTATIONS.as_bytes()).unwrap();
    let rows = build_rows(&ls.loaded, Some(&toml_dir), Some(&ex), &e.licences, now());
    assert_eq!(rows.len(), 5);
    let row = |r: &str| rows.iter().find(|x| x.reference == r).unwrap();

    let fd = row("weftos:fall-detect");
    assert_eq!(fd.run.mode, RunMode::Once);
    assert_eq!((fd.ram_mb, fd.cpu_pct), (Some(48), Some(20)));
    assert_eq!(fd.secrets, vec!["token"]);
    assert_eq!(fd.access, Access::Signed);
    assert!(fd.policy.requires.contains(&"cpu.arch.armv7".to_string()));
    assert!(fd.policy.requires.contains(&"mem.system>=48mb".to_string()));
    assert!(fd.policy.requires.contains(&"x.hw.pi-zero-2w".to_string()));
    assert!(fd.policy.requires.contains(&"feed.esp32-csi-udp".to_string()));
    assert!(fd.policy.v1_scope && fd.policy.signed_package_required && !fd.policy.allow_emulated);
    assert_eq!(fd.policy.min_trust_tier, "paired");

    let sd = row("weftos:swarm-deploy");
    assert_eq!(sd.run.mode, RunMode::NeedsSeed);
    assert!(sd.seed_store && sd.also_in == vec!["cognitum".to_string()], "same id in two sources is flagged");
    assert!(!sd.policy.sensor_feed && !sd.policy.v1_scope);

    let sa = row("cognitum:sleep-apnea");
    assert_eq!(sa.run.mode, RunMode::Interval);
    assert_eq!(sa.run.interval, Some(1));
    assert_eq!(sa.access, Access::NeedsLicence);
    assert_eq!(row("cognitum:baby-cry").access, Access::Licensed);
    assert_eq!(sa.arches, vec!["arm"]);
    assert!(sa.seed_store && sa.also_in.is_empty());
    assert!(!fd.seed_store, "listed by the WeftOS source only, not the Cognitum (Seed) store");

    let table = crate::catalog::render_table(&rows);
    assert!(table.contains("cognitum:sleep-apnea") && table.contains("interval 1") && table.contains("needs-licence"), "{table}");
}

#[test]
fn signing_helper_matches_registry_format() {
    // guards the fixture itself: a registry signed by key A does not verify under key B
    let tmp = tempfile::tempdir().unwrap();
    let a = key(21);
    let reg = signed_repo(tmp.path(), &a, &["x"]);
    let r: weftos_cog_repo::Registry = serde_json::from_slice(&std::fs::read(reg).unwrap()).unwrap();
    let art = &r.cogs[0].artifacts["arm"];
    assert!(weftos_cog_repo::verify_artifact(&bin("x"), art, &key(21).verifying_key()).is_ok());
    assert!(weftos_cog_repo::verify_artifact(&bin("x"), art, &key(22).verifying_key()).is_err());
    let _ = a.sign(b"");
}
