import { arch, homedir, hostname, platform } from "node:os";
import { readFile } from "node:fs/promises";
import { join } from "node:path";

const baseUrl = process.env.WEFTOS_DASHBOARD_URL ?? "https://weftos-dashboard.vercel.app";
const nodeId = process.env.WEFTOS_DASHBOARD_NODE_ID;
const tokenFile = process.env.WEFTOS_DASHBOARD_TOKEN_FILE ?? join(homedir(), ".config/weftos/node-token");
const installationId = process.env.WEFTOS_INSTALLATION_ID ?? hostname();
const intervalMs = Number(process.env.WEFTOS_DASHBOARD_INTERVAL_MS ?? 60_000);
const repoRoot = process.env.WEFTOS_REPO_ROOT ?? process.cwd();

async function receipt(path) {
  try {
    return JSON.parse(await readFile(join(repoRoot, path), "utf8"));
  } catch {
    return null;
  }
}

async function metaharnessReport() {
  const [score, crosscut] = await Promise.all([
    receipt(".metaharness/weftos-score-latest.json"),
    receipt(".metaharness/brain/crosscut-latest.json"),
  ]);
  const report = {};
  if (typeof score?.weftosFoundationScore === "number" && typeof score.generatedAt === "string") {
    report.foundation_score = score.weftosFoundationScore;
    report.score_generated_at = score.generatedAt;
  }
  if (crosscut?.counts && typeof crosscut.generatedAt === "string") {
    const counts = Object.fromEntries(["SEE", "WIRE", "BUILD", "UPSTREAM"]
      .filter((key) => Number.isInteger(crosscut.counts[key]) && crosscut.counts[key] >= 0)
      .map((key) => [key, crosscut.counts[key]]));
    if (Object.keys(counts).length) {
      report.crosscut_counts = counts;
      report.crosscut_generated_at = crosscut.generatedAt;
    }
  }
  return Object.keys(report).length ? report : undefined;
}

if (!nodeId) {
  console.error("Set WEFTOS_DASHBOARD_NODE_ID.");
  process.exit(2);
}
if (!Number.isFinite(intervalMs) || intervalMs < 15_000) {
  console.error("WEFTOS_DASHBOARD_INTERVAL_MS must be at least 15000.");
  process.exit(2);
}

async function heartbeat() {
  try {
    const token = process.env.WEFTOS_DASHBOARD_TOKEN ?? (await readFile(tokenFile, "utf8")).trim();
    if (!/^wft_[a-f0-9]{64}$/.test(token)) throw new Error("Invalid node credential format");
    const response = await fetch(new URL("/api/nodes/heartbeat", baseUrl), {
      method: "POST",
      headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
      body: JSON.stringify({
        node_id: nodeId,
        installation_id: installationId,
        report: {
          hostname: hostname(),
          platform: platform(),
          arch: arch(),
          version: process.env.WEFTOS_VERSION ?? null,
          status: process.env.WEFTOS_NODE_STATUS ?? "ready",
          metaharness: await metaharnessReport(),
        },
      }),
      signal: AbortSignal.timeout(10_000),
    });
    if (!response.ok) throw new Error(`HTTP ${response.status}`);
    console.log("Dashboard heartbeat accepted", new Date().toISOString());
  } catch (error) {
    console.error("Dashboard heartbeat failed:", error instanceof Error ? error.message : String(error));
    if (process.argv.includes("--once")) process.exitCode = 1;
  }
}

await heartbeat();
if (!process.argv.includes("--once")) setInterval(heartbeat, intervalMs);
