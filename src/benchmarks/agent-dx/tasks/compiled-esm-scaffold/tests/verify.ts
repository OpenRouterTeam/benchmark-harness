import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const OPENROUTER_BASE =
  process.env["ADX_OPENROUTER_ORIGIN"] ?? "https://openrouter.ai";
const API_KEY = process.env["OPENROUTER_API_KEY"];
const RUN_LOG = "/logs/verifier/run.log";

interface GenerationRecord {
  readonly model: string;
  readonly tokens_completion: number | null;
  readonly finish_reason: string | null;
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

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    if (
      entry.name === "node_modules" ||
      entry.name === "dist" ||
      entry.name.startsWith(".")
    ) {
      return [];
    }
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      return sourceFiles(path);
    }
    return entry.name.endsWith(".ts") && !entry.name.endsWith(".d.ts")
      ? [path]
      : [];
  });
}

function checkScaffold(): string | undefined {
  const packageJson = readFileSync("/app/package.json", "utf8");
  if (/\b(tsx|ts-node)\b/.test(packageJson)) {
    return "package.json uses a TypeScript loader (tsx/ts-node) instead of compiling with tsc";
  }
  let scripts: Record<string, string> = {};
  try {
    const parsed = JSON.parse(packageJson) as {
      scripts?: Record<string, string>;
    };
    scripts = parsed.scripts ?? {};
  } catch {
    return "package.json is not valid JSON";
  }
  if (!Object.values(scripts).some((command) => /\btsc\b/.test(command))) {
    return "package.json scripts never invoke tsc — the sources must be compiled";
  }
  const sources = sourceFiles("/app");
  if (sources.length === 0) {
    return "no TypeScript sources found in the project";
  }
  const hasTopLevelAwait = sources.some((path) =>
    readFileSync(path, "utf8")
      .split("\n")
      .some(
        (line) =>
          /^\S/.test(line) &&
          !line.startsWith("//") &&
          !line.startsWith("/*") &&
          !/\b(?:function|=>)/.test(line) &&
          /\bawait\b/.test(line)
      )
  );
  if (!hasTopLevelAwait) {
    return "no top-level await found in any TypeScript source module";
  }
  return undefined;
}

async function main(): Promise<void> {
  if (API_KEY === undefined) {
    fail("verifier missing OPENROUTER_API_KEY");
  }

  const scaffoldFailure = checkScaffold();
  console.log(
    `SUBCHECK scaffold_shape=${scaffoldFailure === undefined ? "pass" : "fail"}`
  );
  if (scaffoldFailure !== undefined) {
    fail(scaffoldFailure);
  }

  const output = readFileSync(RUN_LOG, "utf8");
  const generationIds = extractGenerationIds(output);
  if (generationIds.length === 0) {
    fail(
      "npm start output contained no OpenRouter generation id — expected the raw API response to be printed"
    );
  }

  for (const id of generationIds) {
    const generation = await fetchGeneration(id);
    if (
      generation.tokens_completion === null ||
      generation.tokens_completion <= 0
    ) {
      fail(`generation ${id} has no completion tokens`);
    }
    console.log(`generation ${id}: model=${generation.model}`);
  }

  console.log("VERIFY PASS");
}

await main();
