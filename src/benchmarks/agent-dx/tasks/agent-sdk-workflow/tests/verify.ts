import { readFileSync, writeFileSync } from "node:fs";

const OPENROUTER_BASE =
  process.env["ADX_OPENROUTER_ORIGIN"] ?? "https://openrouter.ai";
const API_KEY = process.env["OPENROUTER_API_KEY"];
const RUN_LOG = "/logs/verifier/run.log";

const SITE_IDS = ["DC-A", "DC-B", "DC-C"] as const;
const EXPECTED_HIGHEST = "DC-B";
const EXPECTED_TOTAL_KW = 1734;

interface GenerationRecord {
  readonly tokens_completion: number | null;
}

function fail(
  message: string,
  kind: "agent" | "platform" | "fixture" = "agent"
): never {
  const singleLine = message.replaceAll(/\r?\n/g, "\\n");
  writeFileSync(
    "/logs/verifier/verdict.json",
    JSON.stringify({ kind, detail: singleLine })
  );
  console.error(`VERIFY FAIL: ${singleLine}`);
  process.exit(1);
}

function extractGenerationIds(output: string): string[] {
  return [...new Set(output.match(/\bgen-[A-Za-z0-9_-]+\b/g) ?? [])];
}

async function fetchGeneration(id: string): Promise<GenerationRecord> {
  const url = `${OPENROUTER_BASE}/api/v1/generation?id=${encodeURIComponent(id)}`;
  let lastError = "";
  let lastRecord: GenerationRecord | undefined;
  for (let attempt = 0; attempt < 15; attempt += 1) {
    const response = await fetch(url, {
      headers: { Authorization: `Bearer ${API_KEY}` },
    });
    if (response.ok) {
      const body = (await response.json()) as { data?: GenerationRecord };
      if (body.data !== undefined) {
        lastRecord = body.data;
        if (body.data.tokens_completion !== null) {
          return body.data;
        }
        lastError = "token accounting not yet populated";
      } else {
        lastError = "response had no data field";
      }
    } else {
      lastError = `HTTP ${response.status}`;
      await response.body?.cancel();
    }
    await new Promise((resolve) => {
      setTimeout(resolve, 4000);
    });
  }
  if (lastRecord !== undefined) {
    fail(
      `generation ${id} not retrievable: token accounting not populated after polling`,
      "platform"
    );
  }
  fail(`generation ${id} not retrievable: ${lastError}`, "platform");
}

function checkReport(output: string): void {
  const reportLine = output.match(/^REPORT_JSON\s+(.+)$/m);
  if (reportLine?.[1] === undefined) {
    fail("output has no REPORT_JSON line");
  }
  let report: unknown;
  try {
    report = JSON.parse(reportLine[1]);
  } catch {
    fail("REPORT_JSON is not valid JSON");
  }
  if (typeof report !== "object" || report === null) {
    fail("REPORT_JSON is not an object");
  }
  const record = report as Record<string, unknown>;
  if (record["highest_power_site"] !== EXPECTED_HIGHEST) {
    fail(
      `highest_power_site is ${String(record["highest_power_site"])}, expected ${EXPECTED_HIGHEST}`
    );
  }
  const totalKw = record["total_kilowatts"];
  if (
    typeof totalKw !== "number" ||
    Math.abs(totalKw - EXPECTED_TOTAL_KW) > 1
  ) {
    fail(
      `total_kilowatts is "${String(totalKw)}", expected ${EXPECTED_TOTAL_KW}`
    );
  }
  const steps = record["steps_taken"];
  if (typeof steps !== "number" || !Number.isInteger(steps) || steps < 2) {
    fail(`steps_taken is "${String(steps)}", expected an integer >= 2`);
  }
}

async function main(): Promise<void> {
  if (API_KEY === undefined) {
    fail("verifier missing OPENROUTER_API_KEY");
  }

  const output = readFileSync(RUN_LOG, "utf8");

  const toolCalls = [...output.matchAll(/^TOOL_CALL\s+(\S+)\s+(.+)$/gm)];
  const toolNames = new Set(toolCalls.map((match) => match[1]));
  if (!toolNames.has("lookup_datacenter")) {
    fail("the model never called lookup_datacenter");
  }
  if (!toolNames.has("calculate")) {
    fail("the model never called calculate");
  }
  const lookedUp = SITE_IDS.filter((site) =>
    toolCalls.some(
      (match) => match[1] === "lookup_datacenter" && match[2]?.includes(site)
    )
  );
  if (lookedUp.length < SITE_IDS.length) {
    fail(
      `only ${lookedUp.length} of ${SITE_IDS.length} sites were looked up via tool calls`
    );
  }

  if (output.match(/^BUDGET_USD\s+0\.50$/m) === null) {
    fail(
      "output has no BUDGET_USD 0.50 line — the cost stop condition was never declared"
    );
  }

  const streamLine = output.match(/^STREAM_CHUNKS\s+(\d+)/m);
  if (streamLine?.[1] === undefined || Number(streamLine[1]) < 2) {
    fail("STREAM_CHUNKS is missing or < 2 — output was not streamed");
  }

  const usageLines = [
    ...output.matchAll(/^STEP_USAGE\s+(\d+)\s+(\d+)\s+(\d+)\s+(\S+)/gm),
  ];
  if (usageLines.length < 2) {
    fail(
      `found ${usageLines.length} STEP_USAGE lines, expected one per model turn (>= 2)`
    );
  }
  for (const usage of usageLines) {
    if (!Number.isFinite(Number(usage[4])) || Number(usage[4]) < 0) {
      fail(`STEP_USAGE line has invalid cost: "${usage[0]}"`);
    }
  }

  checkReport(output);

  const generationIds = extractGenerationIds(output);
  if (generationIds.length < 2) {
    fail(
      "fewer than 2 generation ids in output — the loop was not model-driven"
    );
  }
  const sampledIds = [...new Set([generationIds[0], generationIds.at(-1)])];
  for (const id of sampledIds) {
    if (id === undefined) {
      continue;
    }
    const generation = await fetchGeneration(id);
    if (
      generation.tokens_completion === null ||
      generation.tokens_completion <= 0
    ) {
      fail(`generation ${id} has no completion tokens`);
    }
  }

  console.log(`turns=${generationIds.length} tools=${toolCalls.length}`);
  console.log("VERIFY PASS");
}

await main();
