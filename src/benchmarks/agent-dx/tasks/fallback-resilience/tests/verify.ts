import { readFileSync, writeFileSync } from "node:fs";

const OPENROUTER_BASE =
  process.env["ADX_OPENROUTER_ORIGIN"] ?? "https://openrouter.ai";
const API_KEY = process.env["OPENROUTER_API_KEY"];
const FALLBACK_MODEL = process.env["FALLBACK_MODEL"];
const RUN_LOG = "/logs/verifier/run.log";

interface GenerationRecord {
  readonly model: string;
  readonly tokens_completion: number | null;
}

interface CatalogModel {
  readonly id: string;
  readonly canonical_slug?: string | null;
}

function normalizeModelId(id: string): string {
  const variantIndex = id.indexOf(":");
  const base = variantIndex === -1 ? id : id.slice(0, variantIndex);
  const variant = variantIndex === -1 ? "" : id.slice(variantIndex);
  return `${base.replace(/-\d{8}$/, "")}${variant}`;
}

function variantOf(id: string): string {
  const variantIndex = id.indexOf(":");
  return variantIndex === -1 ? "" : id.slice(variantIndex);
}

async function fallbackModelAliases(
  fallbackModel: string
): Promise<{ aliases: Set<string>; isInCatalog: boolean | undefined }> {
  const aliases = new Set([normalizeModelId(fallbackModel)]);
  const response = await fetch(`${OPENROUTER_BASE}/api/v1/models`, {
    headers: { Authorization: `Bearer ${API_KEY}` },
  });
  if (!response.ok) {
    await response.body?.cancel();
    return { aliases, isInCatalog: undefined };
  }
  const body = (await response.json()) as { data?: readonly CatalogModel[] };
  const entry = (body.data ?? []).find((model) => model.id === fallbackModel);
  if (
    typeof entry?.canonical_slug === "string" &&
    entry.canonical_slug !== ""
  ) {
    aliases.add(
      `${normalizeModelId(entry.canonical_slug)}${variantOf(fallbackModel)}`
    );
  }
  return { aliases, isInCatalog: entry !== undefined };
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

async function main(): Promise<void> {
  if (API_KEY === undefined) {
    fail("verifier missing OPENROUTER_API_KEY");
  }
  if (FALLBACK_MODEL === undefined) {
    fail("verifier missing FALLBACK_MODEL task env");
  }

  const { aliases, isInCatalog } = await fallbackModelAliases(FALLBACK_MODEL);
  if (isInCatalog === false) {
    fail(
      `FIXTURE STALE: pinned fallback model ${FALLBACK_MODEL} is no longer in the live catalog — update FALLBACK_MODEL in task.toml (this is a benchmark fixture problem, not an agent failure)`,
      "fixture"
    );
  }

  const output = readFileSync(RUN_LOG, "utf8");
  const generationIds = extractGenerationIds(output);
  if (generationIds.length === 0) {
    fail(
      "npm start output contained no OpenRouter generation id — expected the raw API response to be printed"
    );
  }
  if (!/paris/i.test(output)) {
    fail("npm start output does not contain the answer (Paris)");
  }
  if (
    !/(fallback|fall back|fell back|primary.*(fail|error|unavailable|not found))/is.test(
      output
    )
  ) {
    fail(
      "npm start output does not report that the primary model failed and the fallback was used"
    );
  }

  let sawFallbackGeneration = false;
  for (const id of generationIds) {
    const generation = await fetchGeneration(id);
    if (
      generation.tokens_completion === null ||
      generation.tokens_completion <= 0
    ) {
      fail(`generation ${id} has no completion tokens`);
    }
    console.log(`generation ${id}: model=${generation.model}`);
    if (aliases.has(normalizeModelId(generation.model))) {
      sawFallbackGeneration = true;
    }
  }
  if (!sawFallbackGeneration) {
    fail(`no generation was produced by the fallback model ${FALLBACK_MODEL}`);
  }

  console.log("VERIFY PASS");
}

await main();
