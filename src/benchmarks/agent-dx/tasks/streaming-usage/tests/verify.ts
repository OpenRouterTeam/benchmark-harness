import { readFileSync, writeFileSync } from "node:fs";

const OPENROUTER_BASE =
  process.env["ADX_OPENROUTER_ORIGIN"] ?? "https://openrouter.ai";
const API_KEY = process.env["OPENROUTER_API_KEY"];
const RUN_LOG = "/logs/verifier/run.log";

interface GenerationRecord {
  readonly model: string;
  readonly streamed: boolean | null;
  readonly tokens_prompt: number | null;
  readonly tokens_completion: number | null;
  readonly native_tokens_prompt: number | null;
  readonly native_tokens_completion: number | null;
  readonly total_cost: number | null;
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
  if (lastError === "HTTP 404" && (await controlGenerationRetrievable())) {
    fail(
      `generation ${id} not found while a fresh control generation on the same key was retrievable — the id does not correspond to a real request`
    );
  }
  fail(`generation ${id} not retrievable: ${lastError}`, "platform");
}

async function controlGenerationRetrievable(): Promise<boolean> {
  const completion = await fetch(`${OPENROUTER_BASE}/api/v1/chat/completions`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${API_KEY}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      model: process.env["ADX_MODEL"] ?? "openrouter/auto",
      messages: [{ role: "user", content: "Control probe. Reply with: ok" }],
      max_tokens: 8,
    }),
  });
  if (!completion.ok) {
    await completion.body?.cancel();
    return false;
  }
  const body = (await completion.json()) as { id?: string };
  if (typeof body.id !== "string") {
    return false;
  }
  const url = `${OPENROUTER_BASE}/api/v1/generation?id=${encodeURIComponent(body.id)}`;
  for (let attempt = 0; attempt < 8; attempt += 1) {
    const response = await fetch(url, {
      headers: { Authorization: `Bearer ${API_KEY}` },
    });
    await response.body?.cancel();
    if (response.ok) {
      return true;
    }
    await new Promise((resolve) => {
      setTimeout(resolve, 4000);
    });
  }
  return false;
}

function didReportFigure(output: string, figure: number): boolean {
  const figurePattern = new RegExp(`(?:^|[^\\d.])${figure}(?![\\d.])`);
  return output
    .split("\n")
    .some(
      (line) =>
        /(?:^|[^"\w])(?:tokens?|usage)(?!["\w])/i.test(line) &&
        figurePattern.test(line)
    );
}

async function main(): Promise<void> {
  if (API_KEY === undefined) {
    fail("verifier missing OPENROUTER_API_KEY");
  }

  const output = readFileSync(RUN_LOG, "utf8");
  const [generationId] = extractGenerationIds(output);
  if (generationId === undefined) {
    fail(
      "npm start output contained no OpenRouter generation id — expected the response id to be printed"
    );
  }

  const generation = await fetchGeneration(generationId);
  if (
    generation.tokens_completion === null ||
    generation.tokens_completion <= 0
  ) {
    fail(`generation ${generationId} has no completion tokens`);
  }
  if (generation.streamed === false) {
    fail(
      `generation ${generationId} was not streamed — the request must use streaming`
    );
  }

  const totalTokens =
    (generation.tokens_prompt ?? 0) + generation.tokens_completion;
  const nativeTotal =
    (generation.native_tokens_prompt ?? 0) +
    (generation.native_tokens_completion ?? 0);
  const acceptedFigures = [
    totalTokens,
    generation.tokens_completion,
    nativeTotal,
    generation.native_tokens_completion ?? 0,
  ].filter((n) => n > 0);
  if (!acceptedFigures.some((n) => didReportFigure(output, n))) {
    fail(
      `npm start output does not report the token usage (expected one of: ${acceptedFigures.join(", ")})`
    );
  }
  if (!/(cost|\$|credits)/i.test(output)) {
    fail("npm start output does not report the request cost");
  }

  console.log(
    `generation ${generationId}: model=${generation.model} streamed=${generation.streamed} tokens=${totalTokens} cost=${generation.total_cost}`
  );
  console.log("VERIFY PASS");
}

await main();
