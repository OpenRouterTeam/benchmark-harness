import { readFileSync, writeFileSync } from "node:fs";

const OPENROUTER_BASE =
  process.env["ADX_OPENROUTER_ORIGIN"] ?? "https://openrouter.ai";
const API_KEY = process.env["OPENROUTER_API_KEY"];
const RUN_LOG = "/logs/verifier/run.log";
const MIN_CONTEXT = 128_000;

interface ModelInfo {
  readonly id: string;
  readonly canonical_slug?: string | null;
  readonly context_length: number | null;
  readonly pricing: { readonly prompt: string } | null;
  readonly supported_parameters: readonly string[] | null;
}

interface GenerationRecord {
  readonly model: string;
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

function modelIdAliases(model: ModelInfo): string[] {
  const aliases = [normalizeModelId(model.id)];
  if (typeof model.canonical_slug === "string" && model.canonical_slug !== "") {
    aliases.push(
      `${normalizeModelId(model.canonical_slug)}${variantOf(model.id)}`
    );
  }
  return [...new Set(aliases)];
}

async function fetchModels(): Promise<readonly ModelInfo[]> {
  const response = await fetch(`${OPENROUTER_BASE}/api/v1/models`, {
    headers: { Authorization: `Bearer ${API_KEY}` },
  });
  if (!response.ok) {
    await response.body?.cancel();
    fail(
      `models endpoint returned HTTP ${response.status}`,
      response.status >= 500 ? "platform" : "agent"
    );
  }
  const body = (await response.json()) as { data?: readonly ModelInfo[] };
  if (body.data === undefined || body.data.length === 0) {
    fail("models endpoint returned no models");
  }
  return body.data;
}

function qualifies(model: ModelInfo): boolean {
  return (
    (model.context_length ?? 0) >= MIN_CONTEXT &&
    (model.supported_parameters ?? []).includes("tools") &&
    model.pricing !== null &&
    Number.isFinite(Number(model.pricing.prompt)) &&
    Number(model.pricing.prompt) >= 0
  );
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

  const output = readFileSync(RUN_LOG, "utf8");
  const [generationId] = extractGenerationIds(output);
  if (generationId === undefined) {
    fail(
      "npm start output contained no OpenRouter generation id — expected the raw API response to be printed"
    );
  }

  const models = await fetchModels();
  const qualifying = models.filter(qualifies);
  if (qualifying.length === 0) {
    fail("no live model satisfies the task constraints — cannot verify");
  }
  const prices = new Map(
    qualifying.flatMap((model) =>
      modelIdAliases(model).map(
        (alias) => [alias, Number(model.pricing?.prompt)] as const
      )
    )
  );
  const minPrice = Math.min(...prices.values());

  const generation = await fetchGeneration(generationId);
  if (
    generation.tokens_completion === null ||
    generation.tokens_completion <= 0
  ) {
    fail(`generation ${generationId} has no completion tokens`);
  }

  const usedModel = normalizeModelId(generation.model);
  const usedPrice = prices.get(usedModel);
  if (usedPrice === undefined) {
    fail(
      `generation used ${generation.model}, which does not satisfy the constraints (tools + >=128k context)`
    );
  }
  if (usedPrice > minPrice) {
    fail(
      `generation used ${generation.model} at prompt price ${usedPrice}, but the cheapest qualifying price is ${minPrice}`
    );
  }

  console.log(
    `generation ${generationId}: model=${generation.model} prompt_price=${usedPrice}`
  );
  console.log("VERIFY PASS");
}

await main();
