/**
 * agents-team-validate.mjs — ADR-112 TM1 team spec v1 rules.
 *
 * Pure function over a parsed team.yaml plus a package context; called by
 * agents-validate.mjs for every team that declares `schema: 1`, and driven
 * directly by agents-team-validate.test.mjs with seeded failing fixtures.
 * Schema: agents/teams/SCHEMA.md.
 */
import { readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

export const MODEL_TIERS = new Set(["top", "mid", "small"]);
export const AUTHORITIES = new Set(["autonomous", "supervised", "escalated"]);
export const EDGE_KINDS = new Set(["consumes", "delegates_to", "consults"]);
export const HOSTS = new Set(["claude", "codex", "grok"]);
export const TIERS = new Set(["public", "internal", "restricted"]);
export const REQUIRED_RESOURCES = ["board", "card_history", "memory", "docs", "code", "production_read"];
export const TEAM_RULES = new Set([
  "board-is-for-humans",
  "pickup-is-a-claim",
  "worktree-per-lane",
  "result-header",
  "refuse-not-empty",
  "stand-down",
  "handoff-ritual",
  "injection-stop-the-line",
]);
export const TOOL_MAP_KEYS = ["Task", "SendMessage", "TodoWrite"];

/** What a holder must be able to do to own a resource: one of the package
 *  capabilities AND at least one of the (effective) tools. */
export const WRITE_REQUIREMENTS = {
  board: { capability: "board_write", tools: ["Bash"] },
  card_history: { capability: "queue_write", tools: ["Write", "Edit"] },
  memory: { capability: "memory_write", tools: ["Write", "Bash"] },
  docs: { capability: "docs_write", tools: ["Write", "Edit"] },
  code: { capability: "code_write", tools: ["Write", "Edit"] },
  production_read: { capability: "read_only_measurement", tools: ["Bash", "Read"] },
};

const SLUG = /^[a-z][a-z0-9-]*$/;

export function workspaceVersion() {
  const text = readFileSync(join(ROOT, "Cargo.toml"), "utf8");
  const wsIdx = text.indexOf("[workspace.package]");
  if (wsIdx === -1) throw new Error("Cargo.toml has no [workspace.package] section");
  const m = /\nversion\s*=\s*"([^"]+)"/.exec(text.slice(wsIdx));
  if (!m) throw new Error("Cargo.toml [workspace.package] has no version");
  return m[1];
}

/** Split `id@version`. */
export function splitRef(ref) {
  const s = String(ref);
  const at = s.indexOf("@");
  return at === -1 ? { id: s, pin: null } : { id: s.slice(0, at), pin: s.slice(at + 1) };
}

const isMap = (v) => v && typeof v === "object" && !Array.isArray(v);
const strList = (v) => Array.isArray(v) && v.every((x) => typeof x === "string" && x !== "");

/**
 * @param team parsed team.yaml
 * @param ctx  { packages: Map<id, {kind, capabilities: string[], tools: string[]|null}>, version: string }
 * @returns string[] violations (empty = clean)
 */
export function validateTeamV1(team, ctx) {
  const errs = [];
  const fail = (m) => errs.push(m);

  if (team.schema !== 1) fail(`schema must be 1, got ${JSON.stringify(team.schema)}`);

  const rawMembers = Array.isArray(team.members) ? team.members : [];
  const members = new Map(); // package id -> member
  for (const m of rawMembers) {
    if (!isMap(m) || typeof m.agent !== "string") {
      fail(`member entry is malformed: ${JSON.stringify(m)}`);
      continue;
    }
    const { id, pin } = splitRef(m.agent);
    const pkg = ctx.packages.get(id);
    if (!pkg || pkg.kind === "skill" || pkg.kind === "template") {
      fail(`member "${m.agent}" does not name a spawnable agent package`);
      continue;
    }
    if (members.has(id)) {
      fail(`member "${id}" is listed twice`);
      continue;
    }
    if (pin !== null && pin !== ctx.version) {
      fail(`member "${id}" pins @${pin} but the catalog version is ${ctx.version}`);
    }
    members.set(id, { ...m, id, pkg });

    if (typeof m.one_job !== "string" || m.one_job.trim() === "") {
      fail(`member "${id}" needs a one_job`);
    } else if (/\sand\s/i.test(m.one_job)) {
      fail(`member "${id}" one_job contains " and " (one job only): ${m.one_job}`);
    }
    if (!MODEL_TIERS.has(m.model_tier)) fail(`member "${id}" model_tier must be one of ${[...MODEL_TIERS]}`);
    if (!AUTHORITIES.has(m.authority)) fail(`member "${id}" authority must be one of ${[...AUTHORITIES]}`);
    if (typeof m.owner !== "string" || !SLUG.test(m.owner)) {
      fail(`member "${id}" owner must be a human role slug (e.g. docs-owner), got ${JSON.stringify(m.owner)}`);
    }
    if (!strList(m.must_not) || m.must_not.length === 0) fail(`member "${id}" needs a non-empty must_not list`);
    if (!strList(m.tiers) || m.tiers.length === 0) {
      fail(`member "${id}" needs a non-empty tiers list`);
    } else {
      for (const t of m.tiers) if (!TIERS.has(t)) fail(`member "${id}" tier "${t}" is not one of ${[...TIERS]}`);
    }
    if (typeof m.active !== "boolean") fail(`member "${id}" active must be true or false`);
    if (m.active === false && !Number.isInteger(m.phase)) {
      fail(`member "${id}" is inactive and must declare the phase that activates it`);
    }
    if ("tools" in m) {
      if (!strList(m.tools)) {
        fail(`member "${id}" tools must be a list of tool names`);
      } else if (pkg.tools) {
        const wider = m.tools.filter((t) => !pkg.tools.includes(t));
        if (wider.length) fail(`member "${id}" tools widen the package (not in AGENT.md tools): ${wider.join(", ")}`);
      }
    }
  }

  const effectiveTools = (mem) => (Array.isArray(mem.tools) ? mem.tools : mem.pkg.tools ?? []);

  // lead
  if (team.lead !== null && team.lead !== undefined) {
    const { id } = splitRef(team.lead);
    const leadPkg = ctx.packages.get(id);
    if (members.has(id) || (leadPkg && leadPkg.kind !== "skill")) {
      fail(`lead "${team.lead}" is a spawnable member; the lead must be null or a doctrine skill`);
    }
  }
  const shared = Array.isArray(team.shared_skills) ? team.shared_skills : [];
  for (const s of shared) {
    if (!ctx.packages.has(s)) fail(`shared_skills entry "${s}" does not match any known package`);
  }

  // edges
  const edges = Array.isArray(team.edges) ? team.edges : [];
  edges.forEach((e, i) => {
    if (!isMap(e)) return fail(`edge #${i} is malformed`);
    if (!EDGE_KINDS.has(e.kind)) fail(`edge #${i} kind "${e.kind}" is not one of ${[...EDGE_KINDS]}`);
    for (const end of ["from", "to"]) {
      if (!members.has(e[end])) fail(`edge #${i} ${end} "${e[end]}" is not a member of this team`);
    }
    if (e.from === e.to) fail(`edge #${i} points a member at itself`);
  });

  // write authority
  const wa = team.write_authority;
  const holders = new Map();
  if (!isMap(wa)) {
    fail("write_authority must be a map of resource -> member");
  } else {
    for (const r of REQUIRED_RESOURCES) {
      if (!(r in wa)) fail(`write_authority has no entry for "${r}" (zero holders)`);
    }
    for (const [res, holder] of Object.entries(wa)) {
      if (Array.isArray(holder)) {
        fail(`write_authority.${res} has ${holder.length} holders; exactly one is allowed`);
        continue;
      }
      if (holder === null || holder === undefined || holder === "") {
        fail(`write_authority.${res} has no holder`);
        continue;
      }
      const req = WRITE_REQUIREMENTS[res];
      if (!req) {
        fail(`write_authority.${res} is not a known resource (${Object.keys(WRITE_REQUIREMENTS)})`);
        continue;
      }
      const mem = members.get(holder);
      if (!mem) {
        fail(`write_authority.${res} holder "${holder}" is not a member of this team`);
        continue;
      }
      holders.set(res, holder);
      if (!mem.pkg.capabilities.includes(req.capability)) {
        fail(`write_authority.${res} holder "${holder}" lacks capability ${req.capability}`);
      }
      if (!effectiveTools(mem).some((t) => req.tools.includes(t))) {
        fail(`write_authority.${res} holder "${holder}" has none of the tools ${req.tools.join("|")} needed to write it`);
      }
    }
  }

  // team rules
  const rules = Array.isArray(team.team_rules) ? team.team_rules : [];
  const seenRules = new Set();
  for (const r of rules) {
    if (!isMap(r) || !TEAM_RULES.has(r.name)) {
      fail(`team_rules entry is not a known named rule: ${JSON.stringify(r)}`);
      continue;
    }
    if (seenRules.has(r.name)) fail(`team_rules "${r.name}" is declared twice`);
    seenRules.add(r.name);
    if (typeof r.statement !== "string" || r.statement.trim() === "" || r.statement.includes("\n")) {
      fail(`team_rules "${r.name}" needs a one-line statement`);
    }
  }
  for (const name of TEAM_RULES) if (!seenRules.has(name)) fail(`team_rules is missing "${name}"`);

  // hosts
  const hosts = isMap(team.hosts) ? team.hosts : null;
  if (!hosts) {
    fail("hosts must declare the claude, codex and grok targets");
  } else {
    for (const h of HOSTS) {
      const tm = hosts[h]?.tool_map;
      if (!isMap(tm)) {
        fail(`hosts.${h} needs a tool_map`);
        continue;
      }
      for (const k of TOOL_MAP_KEYS) {
        if (typeof tm[k] !== "string" || tm[k] === "") fail(`hosts.${h}.tool_map is missing ${k}`);
      }
    }
    for (const h of Object.keys(hosts)) if (!HOSTS.has(h)) fail(`hosts.${h} is not a known host`);
  }

  // presets
  const presets = isMap(team.presets) ? team.presets : {};
  for (const [name, p] of Object.entries(presets)) {
    if (!isMap(p)) {
      fail(`preset "${name}" is malformed`);
      continue;
    }
    for (const key of Object.keys(p)) {
      if (key !== "active" && key !== "hosts") fail(`preset "${name}" has undeclared key "${key}"`);
    }
    const active = isMap(p.active) ? p.active : {};
    const phosts = isMap(p.hosts) ? p.hosts : {};
    for (const [mid, v] of Object.entries(active)) {
      if (!members.has(mid)) fail(`preset "${name}" names unknown member "${mid}"`);
      if (typeof v !== "boolean") fail(`preset "${name}" active.${mid} must be true or false`);
    }
    for (const [mid, h] of Object.entries(phosts)) {
      if (!members.has(mid)) fail(`preset "${name}" names unknown member "${mid}"`);
      if (!HOSTS.has(h) || !(hosts && h in hosts)) fail(`preset "${name}" names unknown host "${h}" for "${mid}"`);
    }
    for (const [res, holder] of holders) {
      const mem = members.get(holder);
      const on = holder in active ? active[holder] : mem.active;
      if (on === false && mem.active !== false) {
        fail(`preset "${name}" deactivates "${holder}", the only holder of write_authority.${res}`);
      }
    }
  }

  return errs;
}
