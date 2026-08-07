import { readFileSync, writeFileSync } from "node:fs";

const OPENROUTER_BASE =
  process.env["ADX_OPENROUTER_ORIGIN"] ?? "https://openrouter.ai";
const API_KEY = process.env["OPENROUTER_API_KEY"];
const ANSWER_PATH = "/app/ANSWER.md";
const MIN_CONTEXT = 200_000;

interface ModelInfo {
  readonly id: string;
  readonly context_length: number | null;
}

function fail(
  message: string,
  kind: "agent" | "platform" | "fixture" = "agent"
): never {
  writeFileSync(
    "/logs/verifier/verdict.json",
    JSON.stringify({ kind, detail: message })
  );
  console.error(`VERIFY FAIL: ${message}`);
  process.exit(1);
}

function extractModelIds(text: string): string[] {
  return [
    ...new Set(
      text.match(/\b[a-z][a-z0-9-]+\/[a-z0-9][\w.-]*(?::[\w-]+)?\b/gi) ?? []
    ),
  ];
}

async function fetchCatalog(): Promise<readonly ModelInfo[] | undefined> {
  const response = await fetch(`${OPENROUTER_BASE}/api/v1/models`, {
    headers: { Authorization: `Bearer ${API_KEY}` },
  });
  if (!response.ok) {
    await response.body?.cancel();
    return undefined;
  }
  const body = (await response.json()) as { data?: readonly ModelInfo[] };
  return body.data;
}

async function main(): Promise<void> {
  const answer = readFileSync(ANSWER_PATH, "utf8");

  const candidates = extractModelIds(answer);
  if (candidates.length === 0) {
    fail("answer names no exact model id (vendor/name form)");
  }

  const dollarAmounts = [
    ...answer.matchAll(/(?:\$\s?|USD\s?)([\d,]+(?:\.\d+)?)/gi),
  ].map((match) => Number(match[1]?.replaceAll(",", "")));
  if (
    !dollarAmounts.some((amount) => Number.isFinite(amount) && amount >= 100)
  ) {
    fail("answer has no plausible monthly cost figure");
  }
  if (
    answer.match(
      /\$\s?\d+(?:\.\d+)?\s*(?:\/|per\s+)?\s*(?:1M|million|MTok|token)/i
    ) === null
  ) {
    fail("answer does not state the per-token prices the estimate is based on");
  }

  const catalog = API_KEY === undefined ? undefined : await fetchCatalog();
  if (catalog !== undefined) {
    const resolved = candidates
      .map((id) =>
        catalog.find((model) => model.id.toLowerCase() === id.toLowerCase())
      )
      .filter((model) => model !== undefined);
    const qualifying = resolved.filter(
      (model) => (model.context_length ?? 0) >= MIN_CONTEXT
    );
    if (resolved.length > 0 && qualifying.length === 0) {
      fail(
        `no named model that resolves in the catalog meets the ${MIN_CONTEXT} context requirement`
      );
    }
  }

  console.log(`candidates: ${candidates.join(", ")}`);
  console.log("VERIFY PASS");
}

await main();
