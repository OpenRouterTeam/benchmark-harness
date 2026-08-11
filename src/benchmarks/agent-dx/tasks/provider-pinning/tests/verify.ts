import { readFileSync, writeFileSync } from "node:fs";

const OPENROUTER_BASE =
  process.env["ADX_OPENROUTER_ORIGIN"] ?? "https://openrouter.ai";
const API_KEY = process.env["OPENROUTER_API_KEY"];
const PINNED_PROVIDER = process.env["PINNED_PROVIDER"];
const PIN_MODEL = process.env["PIN_MODEL"];
const RUN_LOG = "/logs/verifier/run.log";

interface GenerationRecord {
  readonly provider_name: string | null;
  readonly tokens_completion: number | null;
}

async function isModelInCatalog(modelId: string): Promise<boolean | undefined> {
  const response = await fetch(`${OPENROUTER_BASE}/api/v1/models`, {
    headers: { Authorization: `Bearer ${API_KEY}` },
  });
  if (!response.ok) {
    await response.body?.cancel();
    return undefined;
  }
  const body = (await response.json()) as { data?: readonly { id: string }[] };
  return (body.data ?? []).some((model) => model.id === modelId);
}

async function isProviderServingModel(
  modelId: string,
  providerName: string
): Promise<boolean | undefined> {
  const response = await fetch(
    `${OPENROUTER_BASE}/api/v1/models/${modelId}/endpoints`,
    {
      headers: { Authorization: `Bearer ${API_KEY}` },
    }
  );
  if (!response.ok) {
    await response.body?.cancel();
    return undefined;
  }
  const body = (await response.json()) as {
    data?: { endpoints?: readonly { provider_name?: string }[] };
  };
  return (body.data?.endpoints ?? []).some((endpoint) =>
    (endpoint.provider_name ?? "")
      .toLowerCase()
      .includes(providerName.toLowerCase())
  );
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

async function main(): Promise<void> {
  if (API_KEY === undefined) {
    fail("verifier missing OPENROUTER_API_KEY");
  }
  if (PINNED_PROVIDER === undefined) {
    fail("verifier missing PINNED_PROVIDER");
  }
  if (
    PIN_MODEL !== undefined &&
    (await isModelInCatalog(PIN_MODEL)) === false
  ) {
    fail(
      `FIXTURE STALE: pinned model ${PIN_MODEL} is no longer in the live catalog — update PIN_MODEL in task.toml (this is a benchmark fixture problem, not an agent failure)`,
      "fixture"
    );
  }
  if (
    PIN_MODEL !== undefined &&
    (await isProviderServingModel(PIN_MODEL, PINNED_PROVIDER)) === false
  ) {
    fail(
      `FIXTURE STALE: pinned provider ${PINNED_PROVIDER} no longer serves ${PIN_MODEL} — update PINNED_PROVIDER or PIN_MODEL in task.toml (this is a benchmark fixture problem, not an agent failure)`,
      "fixture"
    );
  }

  const output = readFileSync(RUN_LOG, "utf8");
  if (!/100/.test(output)) {
    fail("output does not contain the expected answer (100)");
  }
  if (!/PIN_FAILED/.test(output)) {
    fail(
      "no PIN_FAILED line — the bad-provider request must fail closed and be reported"
    );
  }

  const generationIds = extractGenerationIds(output);
  if (generationIds.length === 0) {
    fail("npm start output contained no OpenRouter generation id");
  }
  if (generationIds.length > 1) {
    fail(
      `found ${generationIds.length} generations — the bad-provider request must not produce one`
    );
  }

  const generationId = generationIds[0]!;
  const record = await fetchGeneration(generationId);
  if (record.tokens_completion === null || record.tokens_completion <= 0) {
    fail(`generation ${generationId} has no completion tokens`);
  }
  const provider = (record.provider_name ?? "").toLowerCase();
  if (!provider.includes(PINNED_PROVIDER.toLowerCase())) {
    fail(
      `generation was served by provider "${record.provider_name}", expected pinned provider "${PINNED_PROVIDER}"`
    );
  }

  console.log(
    `generation ${generationId}: served by ${record.provider_name}; bad pin failed closed`
  );
  console.log("VERIFY PASS");
}

await main();
