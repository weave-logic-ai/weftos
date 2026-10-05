//! Pure URL-parameter parsing for the web build: query and fragment pairs, percent-decoding, the
//! project ULID check and the secret-stripping rewrite of the address bar. No web-sys in here, so
//! it is all unit-tested natively.

/// Query keys that carry a secret and are removed from the address bar once read.
pub const SECRET_KEYS: [&str; 2] = ["token", "gwtoken"];

/// Percent-decode a query/fragment value (`+` is a space).
pub fn pct_decode(s: &str) -> String {
    let src = s.replace('+', " ");
    let mut out = Vec::new();
    let mut it = src.bytes();
    while let Some(b) = it.next() {
        if b == b'%' {
            if let (Some(a), Some(c)) = (it.next(), it.next())
                && let (Some(x), Some(y)) = (hexval(a), hexval(c))
            {
                out.push(x * 16 + y);
                continue;
            }
        } else {
            out.push(b);
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hexval(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// `a=1&b=2` (with an optional leading `?` or `#`) as decoded pairs, in order.
pub fn parse_pairs(s: &str) -> Vec<(String, String)> {
    s.trim_start_matches(['?', '#'])
        .split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| match kv.split_once('=') {
            Some((k, v)) => (pct_decode(k), pct_decode(v)),
            None => (pct_decode(kv), String::new()),
        })
        .collect()
}

/// The value for `key`: the fragment wins over the query; empty values count as absent.
pub fn lookup(query: &str, fragment: &str, key: &str) -> Option<String> {
    let find = |s: &str| parse_pairs(s).into_iter().find(|(k, v)| k == key && !v.is_empty()).map(|(_, v)| v);
    find(fragment).or_else(|| find(query))
}

/// The query string (with its leading `?`, or empty) without the secret params.
pub fn strip_secret_query(search: &str) -> String {
    let kept: Vec<&str> = search
        .trim_start_matches('?')
        .split('&')
        .filter(|kv| !kv.is_empty())
        .filter(|kv| {
            let key = pct_decode(kv.split_once('=').map_or(*kv, |(k, _)| k));
            !SECRET_KEYS.contains(&key.as_str())
        })
        .collect();
    if kept.is_empty() {
        String::new()
    } else {
        format!("?{}", kept.join("&"))
    }
}

/// A WeftOS project id: a ULID, 26 characters of Crockford base32 (no I, L, O, U), first <= 7.
pub fn is_project_ulid(s: &str) -> bool {
    const ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    s.len() == 26
        && s.chars().all(|c| ALPHABET.contains(c.to_ascii_uppercase()))
        && s.chars().next().is_some_and(|c| c <= '7')
}

/// Validate a raw project setting: `(project, warning)`. Empty is no project and no warning; an
/// invalid one is ignored with a warning the console shows.
pub fn check_project(raw: &str) -> (String, Option<String>) {
    let raw = raw.trim();
    if raw.is_empty() {
        return (String::new(), None);
    }
    if is_project_ulid(raw) {
        return (raw.to_ascii_uppercase(), None);
    }
    (String::new(), Some(format!("ignoring project \"{raw}\": not a 26-character ULID (Crockford base32)")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ULID: &str = "01J8ZQ4W7K3M9N2P5R6T8V0XYZ";

    #[test]
    fn pairs_decode_and_keep_order() {
        let p = parse_pairs("?gw=http%3A%2F%2F10.0.0.1%3A8080&x=a+b&flag");
        assert_eq!(p[0], ("gw".into(), "http://10.0.0.1:8080".into()));
        assert_eq!(p[1], ("x".into(), "a b".into()));
        assert_eq!(p[2], ("flag".into(), String::new()));
        assert_eq!(parse_pairs("#a=1")[0].0, "a");
        assert!(parse_pairs("").is_empty());
    }

    #[test]
    fn fragment_wins_over_query() {
        assert_eq!(lookup("?token=q", "#token=f", "token").as_deref(), Some("f"));
        assert_eq!(lookup("?token=q", "", "token").as_deref(), Some("q"));
        assert_eq!(lookup("?token=q", "#gwtoken=g", "token").as_deref(), Some("q"));
        assert_eq!(lookup("?token=", "#token=", "token"), None);
        assert_eq!(lookup("", "#gwtoken=a%2Bb", "gwtoken").as_deref(), Some("a+b"));
    }

    #[test]
    fn secret_params_are_stripped_and_others_kept() {
        assert_eq!(strip_secret_query("?host=h&token=s&gwtoken=t&project=P"), "?host=h&project=P");
        assert_eq!(strip_secret_query("?token=s&gwtoken=t"), "");
        assert_eq!(strip_secret_query(""), "");
        assert_eq!(strip_secret_query("?tokens=keep&to%6Ben=gone"), "?tokens=keep");
    }

    #[test]
    fn ulid_validation() {
        assert!(is_project_ulid(ULID));
        assert!(is_project_ulid(&ULID.to_lowercase()));
        assert!(!is_project_ulid("01J8ZQ4W7K3M9N2P5R6T8V0XY")); // 25
        assert!(!is_project_ulid("01J8ZQ4W7K3M9N2P5R6T8V0XYZZ")); // 27
        assert!(!is_project_ulid("01J8ZQ4W7K3M9N2P5R6T8V0XYU")); // U is not Crockford
        assert!(!is_project_ulid("01J8ZQ4W7K3M9N2P5R6T8V0XYL")); // nor L
        assert!(!is_project_ulid("81J8ZQ4W7K3M9N2P5R6T8V0XYZ")); // timestamp overflow
        assert!(!is_project_ulid("lab"));
    }

    #[test]
    fn project_setting_validates_with_a_warning() {
        assert_eq!(check_project(""), (String::new(), None));
        assert_eq!(check_project(&format!(" {} ", ULID.to_lowercase())), (ULID.to_string(), None));
        let (p, w) = check_project("lab");
        assert!(p.is_empty());
        assert!(w.unwrap().contains("lab"));
    }
}
