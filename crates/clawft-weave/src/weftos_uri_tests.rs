use super::*;

const MESH: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const ULID: &str = "01K6ZQ8N3T4V5W6X7Y8Z9A0B1C";

#[test]
fn every_accepted_name_round_trips_byte_for_byte() {
    let names = [
        format!("weftos://{MESH}/projects/{ULID}"),
        format!("weftos://{MESH}/projects/{ULID}/repos/tools"),
        format!("weftos://lab.local/projects/{ULID}/repos/a.b_c~d-e"),
        format!("weftos://{MESH}/nodes/87ee73837d4ba020e074790b42767a34/services/workload-host"),
        format!("weftos://{MESH}/cogs/weft-ecg-scope?rev=sha256:{MESH}"),
        format!("weftos://{MESH}/memory/notes-2026?rev=sha256:{MESH}&view=overview"),
        "weftos://m/companies/c1".to_owned(),
        "weftos://a-1.b2.c/teams/Core/members/alice".to_owned(),
    ];
    for n in names {
        let u = WeftosUri::parse(&n).unwrap_or_else(|e| panic!("{n}: {e}"));
        assert_eq!(u.to_string(), n, "round trip");
    }
    let u = WeftosUri::parse(&format!("weftos://{MESH}/projects/{ULID}/repos/tools")).unwrap();
    assert!(matches!(u.authority, Authority::Mesh(_)));
    assert_eq!(u.kind, Kind::Projects);
    assert_eq!(u.project_repo(), Some((ULID, "tools")));
    assert_eq!(WeftosUri::parse(&format!("weftos://{MESH}/projects/{ULID}")).unwrap().project_repo(), Some((ULID, ".")));
    assert_eq!(WeftosUri::for_project_repo(Authority::Alias("lab".into()), ULID, ".").to_string(), format!("weftos://lab/projects/{ULID}"));
    assert_eq!(WeftosUri::for_project_repo(Authority::Alias("lab".into()), ULID, "tools").to_string(), format!("weftos://lab/projects/{ULID}/repos/tools"));
    for k in Kind::ALL {
        assert_eq!(Kind::parse(k.1), Some(k.0));
    }
}

#[test]
fn the_reject_table() {
    let cases: [(&str, UriError); 30] = [
        ("http://m/projects/x", UriError::Scheme),
        ("WEFTOS://m/projects/x", UriError::Scheme),
        ("Weftos://m/projects/x", UriError::Scheme),
        ("weftos://m/projects/x%2Fy", UriError::Characters),
        ("weftos://m/projects/x\u{e9}", UriError::Characters),
        ("weftos://m/projects/x y", UriError::Characters),
        ("weftos://m/projects/x#top", UriError::Structure),
        ("weftos://user@m/projects/x", UriError::Structure),
        ("weftos://m:9470/projects/x", UriError::Structure),
        ("weftos://M/projects/x", UriError::Authority),
        ("weftos://-m/projects/x", UriError::Authority),
        ("weftos://m-/projects/x", UriError::Authority),
        ("weftos://m..n/projects/x", UriError::Authority),
        ("weftos:///projects/x", UriError::Authority),
        ("weftos://0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef/projects/x", UriError::Authority),
        ("weftos://m/Projects/x", UriError::Kind),
        ("weftos://m/things/x", UriError::Kind),
        ("weftos://m/projects", UriError::Segment),
        ("weftos://m/projects/", UriError::Segment),
        ("weftos://m/projects/x/", UriError::Segment),
        ("weftos://m/projects/x//y", UriError::Segment),
        ("weftos://m/projects/./y", UriError::Segment),
        ("weftos://m/projects/x/..", UriError::Segment),
        ("weftos://m/projects/x/re$po", UriError::Segment),
        ("weftos://m", UriError::Segment),
        ("weftos://m/projects/x?view=content", UriError::Query),
        ("weftos://m/projects/x?rev=sha256:abc", UriError::Query),
        ("weftos://m/projects/x?rev=SHA256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", UriError::Query),
        ("weftos://m/projects/x?rev=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef&rev=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", UriError::Query),
        ("weftos://m/projects/x?rev=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef&view=full", UriError::Query),
    ];
    for (input, want) in cases {
        assert_eq!(WeftosUri::parse(input), Err(want), "{input}");
    }
    // Size caps: 32 segments after the id and 2,048 bytes in all are the most.
    let thirty_two = format!("weftos://m/memory/store{}", "/s".repeat(32));
    assert!(WeftosUri::parse(&thirty_two).is_ok());
    assert_eq!(WeftosUri::parse(&format!("{thirty_two}/s")), Err(UriError::Segment));
    assert!(WeftosUri::parse(&format!("weftos://m/memory/{}", "a".repeat(128))).is_ok());
    assert_eq!(WeftosUri::parse(&format!("weftos://m/memory/{}", "a".repeat(129))), Err(UriError::Segment));
    let mut big = "weftos://m/memory/x".to_owned();
    while big.len() + 129 <= 2048 {
        big.push_str(&format!("/{}", "b".repeat(128)));
    }
    let at_cap = format!("{big}/{}", "c".repeat(2048 - big.len() - 1));
    assert_eq!(at_cap.len(), 2048);
    assert!(WeftosUri::parse(&at_cap).is_ok(), "exactly 2048 bytes parses");
    assert_eq!(WeftosUri::parse(&format!("{at_cap}c")), Err(UriError::Segment), "2049 bytes does not");
    // Uppercase hex in rev; a view before rev; an unknown key.
    for q in ["rev=sha256:0123456789ABCDEF0123456789abcdef0123456789abcdef0123456789abcdef", "view=content&rev=sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef", "x=1"] {
        assert_eq!(WeftosUri::parse(&format!("weftos://m/projects/x?{q}")), Err(UriError::Query), "{q}");
    }
    // The old three-segment form is not a name.
    assert!(WeftosUri::parse(&format!("weftos://{MESH}/{ULID}/.")).is_err());
    assert!(WeftosUri::parse(&format!("weftos://{MESH}/{ULID}/tools")).is_err());
    // A project repository needs a ULID and `repos/<plain dir>`; other paths are not repositories.
    assert_eq!(WeftosUri::parse("weftos://m/projects/not-a-ulid").unwrap().project_repo(), None);
    assert_eq!(WeftosUri::parse(&format!("weftos://m/projects/{ULID}/repos")).unwrap().project_repo(), None);
    assert_eq!(WeftosUri::parse(&format!("weftos://m/projects/{ULID}/repos/.hidden")).unwrap().project_repo(), None);
    assert_eq!(WeftosUri::parse(&format!("weftos://m/projects/{ULID}/files/x")).unwrap().project_repo(), None);
    assert_eq!(WeftosUri::parse(&format!("weftos://m/cogs/{ULID}")).unwrap().project_repo(), None);
}
