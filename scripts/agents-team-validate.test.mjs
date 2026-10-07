/**
 * Tests for team spec v1 (ADR-112 TM1). Every fixture in
 * scripts/fixtures/agents-teams/ is a seeded violation expressed as overrides on
 * base.yaml; each must fail with its `_expect` message and base must pass.
 * Run: node --test scripts/agents-team-validate.test.mjs
 */
import { test } from "node:test";
import { strict as assert } from "node:assert";
import { readFileSync, readdirSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { parseYaml } from "./lib/yaml-lite.mjs";
import { validateTeamV1, workspaceVersion } from "./agents-team-validate.mjs";
import { runValidation } from "./agents-validate.mjs";

const DIR = join(dirname(fileURLToPath(import.meta.url)), "fixtures", "agents-teams");
const load = (f) => parseYaml(readFileSync(join(DIR, f), "utf8"));

const pkg = (kind, capabilities, tools) => ({ kind, capabilities, tools });
const ctx = {
  version: workspaceVersion(),
  packages: new Map([
    ["steward", pkg("specialist", ["board_write"], ["Read", "Grep", "Glob", "Bash"])],
    ["doc-gardener", pkg("specialist", ["docs_write", "queue_write"], ["Read", "Write", "Edit", "Grep", "Glob", "Bash"])],
    ["liber", pkg("specialist", ["memory_write"], ["Read", "Write", "Grep", "Glob", "Bash"])],
    ["mo", pkg("specialist", ["voice_consult"], ["Read", "Grep", "Glob"])],
    ["developer", pkg("lane", ["code_write"], ["Read", "Write", "Edit", "Bash", "Grep", "Glob"])],
    ["reviewer", pkg("lane", ["read_only_review"], ["Read", "Bash", "Grep", "Glob"])],
    ["measurer", pkg("lane", ["read_only_measurement"], ["Read", "Bash", "Grep", "Glob"])],
    ["lead-doctrine", pkg("skill", [], null)],
  ]),
};

const isMap = (v) => v && typeof v === "object" && !Array.isArray(v);

/** Overlay a fixture onto base: maps merge, null deletes, anything else replaces. */
function overlay(base, patch) {
  const out = structuredClone(base);
  const { _expect, _member_patch, _extra_members, _extra_edges, _drop_rules, ...rest } = patch;
  const merge = (dst, src) => {
    for (const [k, v] of Object.entries(src)) {
      if (v === null) delete dst[k];
      else if (isMap(v) && isMap(dst[k])) merge(dst[k], v);
      else dst[k] = v;
    }
  };
  merge(out, rest);
  for (const [id, fields] of Object.entries(_member_patch ?? {})) {
    const m = out.members.find((x) => x.agent.split("@")[0] === id);
    merge(m, fields);
  }
  out.members.push(...(_extra_members ?? []));
  out.edges.push(...(_extra_edges ?? []));
  if (_drop_rules) out.team_rules = out.team_rules.filter((r) => !_drop_rules.includes(r.name));
  return out;
}

test("base fixture is a clean v1 team", () => {
  assert.deepEqual(validateTeamV1(load("base.yaml"), ctx), []);
});

const fixtures = readdirSync(DIR).filter((f) => f.endsWith(".yaml") && f !== "base.yaml").sort();
test("there is a seeded fixture per rule", () => {
  assert.ok(fixtures.length >= 20, `only ${fixtures.length} fixtures`);
});

for (const f of fixtures) {
  test(`fixture ${f} is refused`, () => {
    const patch = load(f);
    assert.ok(patch._expect, `${f} has no _expect`);
    const errs = validateTeamV1(overlay(load("base.yaml"), patch), ctx);
    assert.ok(errs.length > 0, `${f} produced no violation`);
    assert.ok(
      errs.some((e) => e.includes(patch._expect)),
      `${f}: expected a violation containing "${patch._expect}", got:\n${errs.join("\n")}`,
    );
  });
}

test("the real weftos-core team passes the gate", () => {
  const { errorCount, teams } = runValidation();
  assert.equal(errorCount, 0);
  assert.equal(teams.find((t) => t.id === "weftos-core").team.schema, 1);
});
