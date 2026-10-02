use clawft_mesh_local::addr::{AddrError, Node, WeftAddr};

const N: &str = "0123456789abcdef0123456789abcdef";
const U: &str = "fedcba9876543210fedcba9876543210";
const P: &str = "01ARZ3NDEKTSV4RRFFQ69G5FAV";

fn parse(s: &str) -> Result<WeftAddr, AddrError> {
    s.parse()
}

#[test]
fn valid_addresses_round_trip_exactly() {
    let cases = [
        format!("weft://{N}/{U}/{P}"),
        format!("weft://{N}/{U}/{P}/kernel.ipc"),
        format!("weft://{N}/{U}/{P}/substrate/node-1/sensor.temp"),
        format!("weft://{N}/{U}/_"),
        format!("weft://{N}/{U}/_/chat/rooms"),
        format!("weft://{N}/_/_"),
        format!("weft://{N}/_/_/health"),
        format!("weft://local/{U}/{P}/x"),
        "weft://local/_/_".to_string(),
    ];
    for s in cases {
        let a = parse(&s).unwrap_or_else(|e| panic!("{s}: {e}"));
        assert_eq!(a.to_string(), s);
        assert_eq!(parse(&a.to_string()).unwrap(), a);
    }
}

#[test]
fn fields_are_extracted() {
    let a = parse(&format!("weft://{N}/{U}/{P}/a/b.c")).unwrap();
    assert_eq!(a.node, Node::Id(N.into()));
    assert_eq!(a.user.as_deref(), Some(U));
    assert_eq!(a.project.as_deref(), Some(P));
    assert_eq!(a.topic, "a/b.c");
    let n = parse(&format!("weft://{N}/_/_")).unwrap();
    assert!(n.user.is_none() && n.project.is_none() && n.topic.is_empty());
}

#[test]
fn rejection_table() {
    let upper = N.to_uppercase();
    let lower_ulid = P.to_lowercase();
    let cases: Vec<(String, AddrError)> = vec![
        ("".into(), AddrError::MissingScheme),
        (format!("http://{N}/{U}/{P}"), AddrError::MissingScheme),
        (format!("WEFT://{N}/{U}/{P}"), AddrError::MissingScheme),
        (format!("weft:/{N}/{U}/{P}"), AddrError::MissingScheme),
        ("weft://".into(), AddrError::Incomplete),
        (format!("weft://{N}"), AddrError::Incomplete),
        (format!("weft://{N}/{U}"), AddrError::Incomplete),
        (format!("weft://{upper}/{U}/{P}"), AddrError::BadNode),
        (format!("weft://{}/{U}/{P}", &N[..31]), AddrError::BadNode),
        (format!("weft://{N}0/{U}/{P}"), AddrError::BadNode),
        (format!("weft://zz{}/{U}/{P}", &N[2..]), AddrError::BadNode),
        (format!("weft://LOCAL/{U}/{P}"), AddrError::BadNode),
        (format!("weft://_/{U}/{P}"), AddrError::BadNode),
        (format!("weft:///{U}/{P}"), AddrError::BadNode),
        (format!("weft://{N}/{}/{P}", U.to_uppercase()), AddrError::BadUser),
        (format!("weft://{N}/1000/{P}"), AddrError::BadUser),
        (format!("weft://{N}//{P}"), AddrError::BadUser),
        (format!("weft://{N}/{U}/{lower_ulid}"), AddrError::BadProject),
        (format!("weft://{N}/{U}/{}", &P[..25]), AddrError::BadProject),
        (format!("weft://{N}/{U}/{P}0"), AddrError::BadProject),
        (format!("weft://{N}/{U}/01ARZ3NDEKTSV4RRFFQ69G5FAI"), AddrError::BadProject),
        (format!("weft://{N}/{U}/01ARZ3NDEKTSV4RRFFQ69G5FAU"), AddrError::BadProject),
        (format!("weft://{N}/{U}/81ARZ3NDEKTSV4RRFFQ69G5FAV"), AddrError::BadProject),
        (format!("weft://{N}/{U}/"), AddrError::BadProject),
        (format!("weft://{N}/_/{P}"), AddrError::ProjectWithoutUser),
        (format!("weft://{N}/{U}/{P}/"), AddrError::BadTopic("empty segment")),
        (format!("weft://{N}/{U}/{P}//x"), AddrError::BadTopic("empty segment")),
        (format!("weft://{N}/{U}/{P}/a//b"), AddrError::BadTopic("empty segment")),
        (format!("weft://{N}/{U}/{P}/.."), AddrError::BadTopic("dot segment")),
        (format!("weft://{N}/{U}/{P}/a/../b"), AddrError::BadTopic("dot segment")),
        (format!("weft://{N}/{U}/{P}/a..b"), AddrError::BadTopic("empty dotted component")),
        (format!("weft://{N}/{U}/{P}/.hidden"), AddrError::BadTopic("empty dotted component")),
        (format!("weft://{N}/{U}/{P}/a."), AddrError::BadTopic("empty dotted component")),
        (format!("weft://{N}/{U}/{P}/a b"), AddrError::BadChar(' ')),
        (format!("weft://{N}/{U}/{P}/a?x=1"), AddrError::BadChar('?')),
        (format!("weft://{N}/{U}/{P}/a#f"), AddrError::BadChar('#')),
        (format!("weft://{N}/{U}/{P}/a\\b"), AddrError::BadChar('\\')),
        (format!("weft://{N}/{U}/{P}/a%2fb"), AddrError::BadChar('%')),
        (format!("weft://{N}/{U}/{P}/a\0b"), AddrError::BadChar('\0')),
        (format!("weft://{N}/{U}/{P}/caf\u{e9}"), AddrError::BadChar('\u{e9}')),
        (format!("weft://{N}@evil/{U}/{P}"), AddrError::BadChar('@')),
        (format!("weft://{N}/{U}/{P}/{}", "a".repeat(600)), AddrError::TooLong),
    ];
    for (s, want) in cases {
        let got = parse(&s).expect_err(&s);
        assert_eq!(got, want, "input {s:?}");
    }
}

#[test]
fn local_resolution_and_wire_rule() {
    let a = parse(&format!("weft://local/{U}/{P}/x")).unwrap();
    assert_eq!(a.require_wire(), Err(AddrError::LocalOnWire));
    let r = a.resolve_local(N).unwrap();
    assert_eq!(r.to_string(), format!("weft://{N}/{U}/{P}/x"));
    r.require_wire().unwrap();
    assert!(a.resolve_local("nothex").is_err());
    // Resolving a concrete address is the identity.
    assert_eq!(r.resolve_local("1".repeat(32).as_str()).unwrap(), r);
}

#[test]
fn constructor_validates() {
    assert!(WeftAddr::new(Node::Id(N.into()), None, Some(P.into()), "").is_err());
    assert!(WeftAddr::new(Node::Id("x".into()), None, None, "").is_err());
    assert!(WeftAddr::new(Node::Local, Some(U.into()), None, "a.b").is_ok());
}

#[test]
fn fuzz_ish_never_panics_and_accepted_inputs_round_trip() {
    let alphabet: Vec<char> = "weft:/_.-aA0fFzZ \0?#%\u{e9}".chars().collect();
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..20_000 {
        let len = (next() % 70) as usize;
        let body: String = (0..len).map(|_| alphabet[(next() % alphabet.len() as u64) as usize]).collect();
        for s in [body.clone(), format!("weft://{body}"), format!("weft://{N}/{U}/{P}/{body}")] {
            if let Ok(a) = parse(&s) {
                assert_eq!(a.to_string(), s, "accepted but not canonical");
                assert!(a.validate().is_ok());
            }
        }
    }
}
