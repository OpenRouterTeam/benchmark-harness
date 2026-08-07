import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { agentDxTasksDir } from "./dataset";

const CANONICAL_FAIL = `function fail(
  message: string,
  kind: "agent" | "platform" | "fixture" = "agent"
): never {
  writeFileSync(
    "/logs/verifier/verdict.json",
    JSON.stringify({ kind, detail: message })
  );
  console.error(\`VERIFY FAIL: \${message}\`);
  process.exit(1);
}`;

const CANONICAL_EXTRACT_GENERATION_IDS = `function extractGenerationIds(output: string): string[] {
  return [...new Set(output.match(/\\bgen-[A-Za-z0-9_-]+\\b/g) ?? [])];
}`;

const CANONICAL_FETCH_GENERATION = `async function fetchGeneration(id: string): Promise<GenerationRecord> {
  const url = \`\${OPENROUTER_BASE}/api/v1/generation?id=\${encodeURIComponent(id)}\`;
  let lastError = "";
  let lastRecord: GenerationRecord | undefined;
  for (let attempt = 0; attempt < 15; attempt += 1) {
    const response = await fetch(url, {
      headers: { Authorization: \`Bearer \${API_KEY}\` },
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
      lastError = \`HTTP \${response.status}\`;
      await response.body?.cancel();
    }
    await new Promise((resolve) => {
      setTimeout(resolve, 4000);
    });
  }
  if (lastRecord !== undefined) {
    fail(
      \`generation \${id} not retrievable: token accounting not populated after polling\`,
      "platform"
    );
  }
  fail(\`generation \${id} not retrievable: \${lastError}\`, "platform");
}`;

const FETCH_GENERATION_VARIANTS = ["usage-attribution"] as const;

function verifierSources(): readonly { taskId: string; source: string }[] {
  return readdirSync(agentDxTasksDir()).map((taskId) => ({
    taskId,
    source: readFileSync(
      join(agentDxTasksDir(), taskId, "tests", "verify.ts"),
      "utf8"
    ),
  }));
}

function extractBlock(source: string, header: string): string | undefined {
  const start = source.indexOf(header);
  if (start === -1) {
    return undefined;
  }
  const end = source.indexOf("\n}", start);
  return source.slice(start, end + 2);
}

describe("agent-dx verifier conformance", () => {
  test("every verify.ts fail() matches the canonical VERIFY FAIL implementation", () => {
    for (const { taskId, source } of verifierSources()) {
      expect(extractBlock(source, "function fail("), taskId).toBe(
        CANONICAL_FAIL
      );
    }
  });

  test("every extractGenerationIds copy matches the canonical implementation", () => {
    const defining = verifierSources().filter((v) =>
      v.source.includes("function extractGenerationIds")
    );
    expect(defining.length).toBeGreaterThan(0);
    for (const { taskId, source } of defining) {
      expect(
        extractBlock(source, "function extractGenerationIds("),
        taskId
      ).toBe(CANONICAL_EXTRACT_GENERATION_IDS);
    }
  });

  test("every fetchGeneration copy matches the canonical polling implementation or a listed variant", () => {
    const defining = verifierSources().filter((v) =>
      v.source.includes("async function fetchGeneration")
    );
    expect(defining.length).toBeGreaterThan(0);
    for (const { taskId, source } of defining) {
      if (FETCH_GENERATION_VARIANTS.some((variant) => variant === taskId)) {
        continue;
      }
      expect(
        extractBlock(source, "async function fetchGeneration("),
        taskId
      ).toBe(CANONICAL_FETCH_GENERATION);
    }
  });
});
