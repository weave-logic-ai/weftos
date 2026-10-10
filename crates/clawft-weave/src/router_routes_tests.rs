use std::path::Path;

use super::*;

fn parse(text: &str, name: &str) -> ProjectRoutes {
    parse_ports_yaml(text, name, Path::new("/p"))
}

#[test]
fn routes_parse_with_defaults_and_the_pc_http_claim() {
    let p = parse(
        "project: shastaos\nclaims:\n  - { port: 18110, use: process-compose-http }\n  - { port: 18120, use: shasta-field }\n\
         routes:\n  - { port: 18120, health: /api/health, default: true }\n  - { prefix: /shastaos/api/, port: 18113 }\n",
        "Shasta",
    );
    assert!(p.refused.is_empty(), "{:?}", p.refused);
    assert_eq!(p.info.slug, "shastaos");
    assert_eq!(p.info.pc_http, Some(18110));
    assert_eq!(p.routes[0], Route { project: "shastaos".into(), prefix: "/shastaos".into(), port: 18120, health: Some("/api/health".into()), default: true });
    assert_eq!(p.routes[1].prefix, "/shastaos/api", "trailing slash dropped");
}

#[test]
fn the_manifest_name_is_the_fallback_slug() {
    let p = parse("routes:\n  - { port: 4000 }\n", "My Project!");
    assert_eq!(p.info.slug, "my-project");
    assert_eq!(p.routes[0].prefix, "/my-project");
    assert_eq!(slugify("  ---  "), None);
    assert!(valid_slug("a-1") && !valid_slug("-a") && !valid_slug("A") && !valid_slug(""));
}

#[test]
fn bad_declarations_are_refused_one_by_one() {
    let yaml = [
        "project: x",
        "routes:",
        "  - { prefix: /Bad_Case, port: 3000 }",
        "  - { prefix: /api/v1, port: 3001 }",
        "  - { prefix: /_weftos, port: 3002 }",
        "  - { prefix: /console/x, port: 3003 }",
        "  - { prefix: /, port: 3004 }",
        "  - { prefix: /ok, port: 80 }",
        "  - { prefix: /ok, port: 70000 }",
        "  - { prefix: /ok, port: 3005, health: \"no slash\" }",
        "  - { prefix: /fine, port: 3006 }",
    ]
    .join("\n");
    let p = parse(&yaml, "x");
    assert_eq!(p.routes.len(), 1, "{:?}", p.routes);
    assert_eq!(p.routes[0].prefix, "/fine");
    let reasons: Vec<&str> = p.refused.iter().map(|r| r.reason.as_str()).collect();
    assert_eq!(p.refused.len(), 8, "{reasons:?}");
    assert!(reasons[0].contains("[a-z0-9-]"));
    assert!(reasons[1].contains("reserved") && reasons[2].contains("reserved") && reasons[3].contains("reserved"));
    assert!(reasons[4].contains("reserved") && reasons[4].contains("default: true"));
    assert!(reasons[5].contains("1024..=65535") && reasons[6].contains("1024..=65535"));
    assert!(reasons[7].contains("health"));
    assert_eq!(p.refused[0].project, "x");
}

#[test]
fn a_broken_file_or_bad_project_name_refuses_the_whole_project() {
    let p = parse("routes: [", "x");
    assert!(p.routes.is_empty());
    assert!(p.refused[0].reason.starts_with("compose/ports.yaml:"));
    let p = parse("project: Shasta OS\nroutes:\n  - { port: 3000 }\n", "shasta");
    assert!(p.routes.is_empty());
    assert!(p.refused[0].reason.contains("is not a slug"), "{:?}", p.refused);
    assert_eq!(p.info.slug, "shasta");
}

fn pr(slug: &str, routes: &[(&str, u16, bool)]) -> ProjectRoutes {
    ProjectRoutes {
        info: ProjectInfo { slug: slug.into(), root: Path::new("/p").join(slug), pc_http: None },
        routes: routes.iter().map(|(p, port, d)| Route { project: slug.into(), prefix: (*p).into(), port: *port, health: None, default: *d }).collect(),
        refused: Vec::new(),
    }
}

#[test]
fn later_duplicates_are_refused_and_named() {
    let t = RouteTable::build(vec![
        pr("a", &[("/a", 3000, false), ("/a/ws", 3000, false)]),
        pr("b", &[("/a", 3001, false), ("/b", 3000, false), ("/b2", 3002, false)]),
    ]);
    assert_eq!(t.routes.iter().map(|r| (r.prefix.as_str(), r.project.as_str())).collect::<Vec<_>>(), [("/a/ws", "a"), ("/b2", "b"), ("/a", "a")]);
    assert_eq!(t.refused.len(), 2);
    assert_eq!(t.refused[0].reason, "prefix /a is already routed by project a");
    assert_eq!(t.refused[1].reason, "port 3000 is already routed by project a (/a)");
    assert_eq!(t.refused[1].project, "b");
}

#[test]
fn only_one_default_route_is_admitted() {
    let t = RouteTable::build(vec![pr("a", &[("/a", 3000, true)]), pr("b", &[("/b", 3001, true)])]);
    assert_eq!(t.default_route().map(|r| r.project.as_str()), Some("a"));
    assert_eq!(t.routes.len(), 2, "b's route is still served at its prefix");
    assert!(!t.routes.iter().find(|r| r.project == "b").unwrap().default);
    assert!(t.refused[0].reason.contains("default route is already held by project a"));
}

#[test]
fn longest_prefix_wins_on_segment_boundaries_and_default_catches_the_rest() {
    let t = RouteTable::build(vec![pr("a", &[("/a", 3000, false), ("/a/deep", 3001, false)]), pr("ab", &[("/ab", 3002, true)])]);
    let m = |p: &str| t.matches(p).map(|r| r.port);
    assert_eq!(m("/a"), Some(3000));
    assert_eq!(m("/a/x?y=1"), Some(3000));
    assert_eq!(m("/a/deep"), Some(3001));
    assert_eq!(m("/a/deep/x"), Some(3001));
    assert_eq!(m("/a/deeper"), Some(3000), "`/a/deep` must not cover `/a/deeper`");
    assert_eq!(m("/ab/x"), Some(3002));
    assert_eq!(m("/"), Some(3002), "default route");
    assert_eq!(m("/zzz"), Some(3002));
    assert_eq!(m("/_weftos/routes.json"), None);
    assert_eq!(m("/_weftos"), None);
    let no_default = RouteTable::build(vec![pr("a", &[("/a", 3000, false)])]);
    assert_eq!(no_default.matches("/"), None);
    assert!(prefix_matches("/a", "/a") && prefix_matches("/a", "/a/") && !prefix_matches("/a", "/ab"));
}

#[test]
fn this_repos_own_ports_file_declares_the_docs_route() {
    let p = parse_ports_yaml(include_str!("../../../compose/ports.yaml"), "weftos", Path::new("/weftos"));
    assert!(p.refused.is_empty(), "{:?}", p.refused);
    assert_eq!(p.info.slug, "weftos");
    assert_eq!(p.info.pc_http, Some(18090));
    assert_eq!(p.routes, vec![Route { project: "weftos".into(), prefix: "/weftos-docs".into(), port: 4000, health: Some("/".into()), default: false }]);
}

#[test]
fn project_info_is_kept_in_registration_order() {
    let t = RouteTable::build(vec![pr("b", &[]), pr("a", &[("/a", 3000, false)])]);
    assert_eq!(t.projects.iter().map(|p| p.slug.as_str()).collect::<Vec<_>>(), ["b", "a"]);
    assert!(t.project("a").is_some() && t.project("zz").is_none());
}
