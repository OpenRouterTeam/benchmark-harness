import { readFileSync, writeFileSync } from "node:fs";

const OPENROUTER_BASE =
  process.env["ADX_OPENROUTER_ORIGIN"] ?? "https://openrouter.ai";
const API_KEY = process.env["OPENROUTER_API_KEY"];
const PRESET_SLUG = process.env["ADX_PRESET_SLUG"];
const RUN_LOG = "/logs/verifier/run.log";

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

function modelsRoughlyEqual(a: string, b: string): boolean {
  return normalizedBase(a) === normalizedBase(b);
}

function normalizedBase(id: string): string {
  return id
    .split(":")[0]!
    .toLowerCase()
    .replace(/-\d{8}$/, "");
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

interface PresetVersion {
  readonly config?: { readonly model?: unknown };
  readonly system_prompt?: unknown;
}

async function fetchPresetModel(
  slug: string
): Promise<{ model: string; version: PresetVersion }> {
  const response = await fetch(
    `${OPENROUTER_BASE}/api/v1/presets/${encodeURIComponent(slug)}`,
    {
      headers: { Authorization: `Bearer ${API_KEY}` },
    }
  );
  if (!response.ok) {
    await response.body?.cancel();
    if (response.status === 404) {
      fail(`preset "${slug}" does not exist on the account (HTTP 404)`);
    }
    fail(
      `preset "${slug}" not retrievable: HTTP ${response.status}`,
      "platform"
    );
  }
  const body = (await response.json()) as {
    data?: { designated_version?: PresetVersion | null };
  };
  const version = body.data?.designated_version;
  if (version === undefined || version === null) {
    fail(`preset "${slug}" has no designated version`);
  }
  const model = version.config?.model;
  if (typeof model !== "string" || model.length === 0) {
    fail(`preset "${slug}" has no model configured`);
  }
  return { model, version };
}

async function isModelInCatalog(modelId: string): Promise<boolean> {
  const response = await fetch(`${OPENROUTER_BASE}/api/v1/models`, {
    headers: { Authorization: `Bearer ${API_KEY}` },
  });
  if (!response.ok) {
    await response.body?.cancel();
    fail(
      `models API returned HTTP ${response.status}`,
      response.status >= 500 ? "platform" : "agent"
    );
  }
  const body = (await response.json()) as { data?: readonly { id: string }[] };
  return (body.data ?? []).some((model) =>
    modelsRoughlyEqual(model.id, modelId)
  );
}

async function main(): Promise<void> {
  if (API_KEY === undefined) {
    fail("verifier missing OPENROUTER_API_KEY");
  }
  if (PRESET_SLUG === undefined || PRESET_SLUG.length === 0) {
    fail("verifier missing ADX_PRESET_SLUG");
  }

  const output = readFileSync(RUN_LOG, "utf8");

  const presetModelLine = output.match(/PRESET_MODEL\s+(\S+)/);
  if (presetModelLine?.[1] === undefined) {
    fail("no PRESET_MODEL line naming the model configured in the preset");
  }

  const { model: configuredModel } = await fetchPresetModel(PRESET_SLUG);
  if (!modelsRoughlyEqual(configuredModel, presetModelLine[1])) {
    fail(
      `PRESET_MODEL says "${presetModelLine[1]}" but the preset actually configures "${configuredModel}"`
    );
  }
  if (!(await isModelInCatalog(configuredModel))) {
    fail(
      `preset model "${configuredModel}" is not in the live catalog — stale or invalid model id`
    );
  }

  const generationIds = extractGenerationIds(output);
  if (generationIds.length === 0) {
    fail(
      "no generation ids printed — expected the raw chat completion response"
    );
  }
  const isRouterMetaModel = configuredModel.startsWith("openrouter/");
  const presetServed = [];
  for (const id of generationIds) {
    const record = await fetchGeneration(id);
    if (record.tokens_completion === null || record.tokens_completion <= 0) {
      fail(`generation ${id} has no completion tokens`);
    }
    if (
      isRouterMetaModel ||
      modelsRoughlyEqual(record.model, configuredModel)
    ) {
      presetServed.push(id);
    }
  }
  if (presetServed.length === 0) {
    fail(
      `no generation was served by the preset's configured model ${configuredModel} — the completion must run through the preset`
    );
  }

  console.log(
    `preset ${PRESET_SLUG} configures ${configuredModel} (valid in catalog); generation served by it`
  );
  console.log("VERIFY PASS");
}

await main();
