//! Route model for the tailnet router (ADR-116 §2).
//!
//! Each registered project declares routes in its own `compose/ports.yaml`
//! next to its port claims:
//!
//! ```yaml
//! project: shastaos
//! claims:
//!   - { port: 18110, use: process-compose-http }
//! routes:
//!   - { prefix: /shastaos, port: 18120, health: /api/health, default: true,
//!       allow: [alice@example.com] }
//! ```
//!
//! The prefix defaults to `/<project>`. `allow` (R2) restricts a route to the
//! listed tailnet logins. Routes also come from a dashboard overlay
//! (`~/.weftos/routes/<ULID>.yaml`, R3, see [`crate::router_overlay`]); those
//! carry `source: dashboard` and are admitted after every repository route,
//! so the repository wins on the same prefix. The table is built in
//! registration order; a prefix or port already taken by an earlier project,
//! a second `default: true`, or an invalid declaration is **refused and
//! reported**, never resolved silently.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Prefixes no project may route (the router and the daemon's own surfaces).
pub const RESERVED_PREFIXES: &[&str] = &["/", "/api", "/console", "/_weftos"];
/// The `use:` of the claim that names a project's process-compose HTTP port.
pub const PC_HTTP_USE: &str = "process-compose-http";
/// Most routes read from one `ports.yaml` (and from one overlay).
pub const MAX_ROUTES_PER_PROJECT: usize = 32;
/// Lowest port a route may point at (no privileged ports behind the tailnet).
pub const MIN_PORT: u64 = 1024;
/// Most logins on one route's `allow` list.
pub const MAX_ALLOW: usize = 64;
/// Longest accepted login.
pub const MAX_LOGIN_LEN: usize = 254;

#[derive(Debug, Deserialize)]
struct PortsFile {
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    claims: Vec<Claim>,
    #[serde(default)]
    routes: Vec<RouteDecl>,
}

#[derive(Debug, Deserialize)]
struct Claim {
    #[serde(default)]
    port: Option<u64>,
    #[serde(default, rename = "use")]
    use_: Option<String>,
}

/// One `routes:` entry as written (the same shape in `ports.yaml` and in an overlay).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RouteDecl {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    pub port: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<String>,
    #[serde(default)]
    pub default: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
}

/// Where a route was declared.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The project's own `compose/ports.yaml`.
    #[default]
    Repo,
    /// A dashboard overlay (`~/.weftos/routes/<ULID>.yaml`).
    Dashboard,
}

/// One accepted route.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Route {
    /// Project slug the route belongs to.
    pub project: String,
    /// `/segment[/segment]`, no trailing slash. Kept on the upstream request.
    pub prefix: String,
    /// Loopback upstream port.
    pub port: u16,
    /// Health path (GET), when declared.
    pub health: Option<String>,
    /// Also serves unmatched paths at `/` (ADR-116 §3, transitional).
    pub default: bool,
    /// Tailnet logins allowed through (lower-cased); empty is open to the tailnet.
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default)]
    pub source: Source,
}

impl Route {
    /// Whether an allowlist applies.
    pub fn restricted(&self) -> bool {
        !self.allow.is_empty()
    }
}

/// A declaration the table did not accept, and why.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refused {
    pub project: String,
    pub prefix: String,
    pub port: u16,
    pub reason: String,
    #[serde(default)]
    pub source: Source,
}

/// A project as the router sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub slug: String,
    pub root: PathBuf,
    /// The `process-compose-http` claim, if any.
    pub pc_http: Option<u16>,
    /// The registered project's ULID, when the candidate is a manifest root.
    #[serde(default)]
    pub ulid: Option<String>,
}

/// Routes parsed from one project's `ports.yaml`, before table admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRoutes {
    pub info: ProjectInfo,
    pub routes: Vec<Route>,
    pub refused: Vec<Refused>,
}

impl ProjectRoutes {
    /// A project with no `ports.yaml` (overlay routes only).
    pub fn empty(fallback_slug: &str, root: &Path) -> Self {
        let slug = slugify(fallback_slug).unwrap_or_else(|| "project".to_owned());
        Self { info: ProjectInfo { slug, root: root.to_path_buf(), pc_http: None, ulid: None }, routes: Vec::new(), refused: Vec::new() }
    }
}

/// The admitted route table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteTable {
    /// Longest prefix first.
    pub routes: Vec<Route>,
    pub refused: Vec<Refused>,
    pub projects: Vec<ProjectInfo>,
}

/// Is `s` a project slug: `[a-z0-9-]+`, starting with a letter or digit?
pub fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !s.starts_with('-')
}

/// A manifest name reduced to a slug (`My Project` → `my-project`), if anything is left.
pub fn slugify(name: &str) -> Option<String> {
    let mut out = String::new();
    for c in name.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    valid_slug(&out).then_some(out)
}

/// Validate and normalise a route prefix: `/seg[/seg]` of `[a-z0-9-]`, a
/// trailing slash dropped, reserved prefixes refused.
pub fn normalize_prefix(raw: &str) -> Result<String, String> {
    let p = raw.trim().trim_end_matches('/');
    if p.is_empty() {
        return Err("prefix `/` is reserved (set default: true to hold the root)".into());
    }
    if !p.starts_with('/') || p.len() > 200 {
        return Err(format!("prefix {raw:?} must start with `/` and name short segments"));
    }
    for r in RESERVED_PREFIXES.iter().filter(|r| **r != "/") {
        if p == *r || p.starts_with(&format!("{r}/")) {
            return Err(format!("prefix {raw:?} is reserved ({r}/)"));
        }
    }
    for seg in p[1..].split('/') {
        if seg.is_empty() || !seg.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(format!("prefix {raw:?}: segment {seg:?} is not [a-z0-9-]"));
        }
    }
    Ok(p.to_owned())
}

/// Is `h` a health path the index may GET?
pub fn valid_health(h: &str) -> bool {
    h.starts_with('/') && h.len() <= 200 && h.chars().all(|c| c.is_ascii_graphic())
}

/// A route port: 1024..=65535.
pub fn port_of(n: u64, what: &str) -> Result<u16, String> {
    if !(MIN_PORT..=65535).contains(&n) {
        return Err(format!("{what} port {n} is outside {MIN_PORT}..=65535"));
    }
    Ok(n as u16)
}

/// Is `s` a tailnet login (`local@domain`, as Tailscale reports it)? Lenient on
/// purpose: `alice@example.com` and `alice@github` are both logins.
pub fn valid_login(s: &str) -> bool {
    let Some((local, domain)) = s.split_once('@') else { return false };
    !local.is_empty()
        && !domain.is_empty()
        && s.len() <= MAX_LOGIN_LEN
        && !domain.contains('@')
        && s.chars().all(|c| c.is_ascii_graphic() && !matches!(c, '<' | '>' | '"' | ','))
}

/// Validate an `allow` list: at most [`MAX_ALLOW`] logins, each login-shaped,
/// lower-cased and deduplicated (matching is case-insensitive).
pub fn normalize_allow(raw: &[String]) -> Result<Vec<String>, String> {
    if raw.len() > MAX_ALLOW {
        return Err(format!("allow lists more than {MAX_ALLOW} logins"));
    }
    let mut out: Vec<String> = Vec::with_capacity(raw.len());
    for a in raw {
        let a = a.trim();
        if !valid_login(a) {
            return Err(format!("allow entry {a:?} is not a login (user@domain)"));
        }
        let l = a.to_ascii_lowercase();
        if !out.contains(&l) {
            out.push(l);
        }
    }
    Ok(out)
}

/// Validate one declaration for project `slug`; a bad one becomes a [`Refused`].
pub fn admit_decl(d: RouteDecl, slug: &str, source: Source) -> Result<Route, Refused> {
    let raw_prefix = d.prefix.clone().unwrap_or_else(|| format!("/{slug}"));
    let port = d.port.min(u64::from(u16::MAX)) as u16;
    let outcome = (|| {
        let prefix = normalize_prefix(&raw_prefix)?;
        let port = port_of(d.port, "route")?;
        if let Some(h) = &d.health
            && !valid_health(h)
        {
            return Err(format!("health {h:?} must be a path"));
        }
        let allow = normalize_allow(&d.allow)?;
        Ok(Route { project: slug.to_owned(), prefix, port, health: d.health.clone(), default: d.default, allow, source })
    })();
    outcome.map_err(|reason| Refused { project: slug.to_owned(), prefix: raw_prefix, port, reason, source })
}

/// Parse one project's `ports.yaml`. `fallback_slug` (the manifest name) is
/// used when the file has no `project:`. A file-level error refuses every
/// route of that project; a bad route refuses only itself.
pub fn parse_ports_yaml(text: &str, fallback_slug: &str, root: &Path) -> ProjectRoutes {
    let mut out = ProjectRoutes::empty(fallback_slug, root);
    let fallback = out.info.slug.clone();
    let refuse = |out: &mut ProjectRoutes, reason: String| {
        out.refused.push(Refused { project: fallback.clone(), prefix: "-".into(), port: 0, reason, source: Source::Repo });
    };
    let file: PortsFile = match serde_yaml::from_str(text) {
        Ok(f) => f,
        Err(e) => {
            refuse(&mut out, format!("compose/ports.yaml: {e}"));
            return out;
        }
    };
    match file.project.as_deref().map(str::trim) {
        Some(p) if valid_slug(p) => out.info.slug = p.to_owned(),
        Some(p) => {
            refuse(&mut out, format!("compose/ports.yaml: project {p:?} is not a slug ([a-z0-9-])"));
            return out;
        }
        None => {}
    }
    out.info.pc_http = file
        .claims
        .iter()
        .find(|c| c.use_.as_deref() == Some(PC_HTTP_USE))
        .and_then(|c| c.port)
        .and_then(|p| port_of(p, "process-compose-http").ok());
    let slug = out.info.slug.clone();
    if file.routes.len() > MAX_ROUTES_PER_PROJECT {
        out.refused.push(Refused { project: slug.clone(), prefix: "-".into(), port: 0, reason: format!("more than {MAX_ROUTES_PER_PROJECT} routes"), source: Source::Repo });
    }
    for d in file.routes.into_iter().take(MAX_ROUTES_PER_PROJECT) {
        match admit_decl(d, &slug, Source::Repo) {
            Ok(r) => out.routes.push(r),
            Err(x) => out.refused.push(x),
        }
    }
    out
}

impl RouteTable {
    /// Admit projects in registration order: every repository route first,
    /// then every overlay route, so the repository wins on the same prefix. A
    /// prefix taken earlier, a port taken by an earlier *other* project, or a
    /// second default is refused.
    pub fn build(projects: Vec<ProjectRoutes>) -> Self {
        let mut t = RouteTable::default();
        let (mut repo, mut dash) = (Vec::new(), Vec::new());
        for p in projects {
            t.refused.extend(p.refused);
            for r in p.routes {
                if r.source == Source::Dashboard { dash.push(r) } else { repo.push(r) }
            }
            t.projects.push(p.info);
        }
        let mut default_holder: Option<String> = None;
        for r in repo.into_iter().chain(dash) {
            t.admit(r, &mut default_holder);
        }
        t.routes.sort_by(|a, b| b.prefix.len().cmp(&a.prefix.len()).then_with(|| a.prefix.cmp(&b.prefix)));
        t
    }

    fn admit(&mut self, mut r: Route, default_holder: &mut Option<String>) {
        let refuse = |r: &Route, reason: String| Refused { project: r.project.clone(), prefix: r.prefix.clone(), port: r.port, reason, source: r.source };
        if let Some(prev) = self.routes.iter().find(|x| x.prefix == r.prefix) {
            let reason = if prev.project == r.project && prev.source == Source::Repo && r.source == Source::Dashboard {
                format!("prefix {} is declared in project {}'s compose/ports.yaml; the repository wins", r.prefix, prev.project)
            } else {
                format!("prefix {} is already routed by project {}", r.prefix, prev.project)
            };
            self.refused.push(refuse(&r, reason));
            return;
        }
        if let Some(prev) = self.routes.iter().find(|x| x.port == r.port && x.project != r.project) {
            self.refused.push(refuse(&r, format!("port {} is already routed by project {} ({})", r.port, prev.project, prev.prefix)));
            return;
        }
        if r.default {
            match default_holder {
                Some(h) => {
                    self.refused.push(refuse(&r, format!("default route is already held by project {h}; this route is admitted without default")));
                    r.default = false;
                }
                None => *default_holder = Some(r.project.clone()),
            }
        }
        self.routes.push(r);
    }

    /// The route whose prefix is the longest match for `path` (query ignored),
    /// else the default route. `/_weftos` is never routed.
    pub fn matches(&self, path: &str) -> Option<&Route> {
        let path = path.split(['?', '#']).next().unwrap_or("");
        if path == "/_weftos" || path.starts_with("/_weftos/") {
            return None;
        }
        self.routes.iter().find(|r| prefix_matches(&r.prefix, path)).or_else(|| self.default_route())
    }

    /// The transitional root route, if a project holds it.
    pub fn default_route(&self) -> Option<&Route> {
        self.routes.iter().find(|r| r.default)
    }

    /// Project info by slug.
    pub fn project(&self, slug: &str) -> Option<&ProjectInfo> {
        self.projects.iter().find(|p| p.slug == slug)
    }
}

/// Does `prefix` cover `path` on a segment boundary (`/a` covers `/a` and
/// `/a/x`, never `/ab`)?
pub fn prefix_matches(prefix: &str, path: &str) -> bool {
    path == prefix || path.strip_prefix(prefix).is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
#[path = "router_routes_tests.rs"]
mod tests;
