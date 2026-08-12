import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { agentDxTasksDir } from "./dataset";

const CANONICAL_FAIL = `function fail(
  message: string,
  kind: "agent" | "platform" | "fixture" = "agent"
): never {
  const singleLine = message.replaceAll(/\\r?\\n/g, "\\\\n");
  writeFileSync(
    "/logs/verifier/verdict.json",
    JSON.stringify({ kind, detail: singleLine })
  );
  console.error(\`VERIFY FAIL: \${singleLine}\`);
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
  if (lastError === "HTTP 404" && (await controlGenerationRetrievable())) {
    fail(
      \`generation \${id} not found while a fresh control generation on the same key was retrievable — the id does not correspond to a real request\`
    );
  }
  fail(\`generation \${id} not retrievable: \${lastError}\`, "platform");
}`;

const CANONICAL_CONTROL_PROBE = `async function controlGenerationRetrievable(): Promise<boolean> {
  const completion = await fetch(\`\${OPENROUTER_BASE}/api/v1/chat/completions\`, {
    method: "POST",
    headers: {
      Authorization: \`Bearer \${API_KEY}\`,
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
  const url = \`\${OPENROUTER_BASE}/api/v1/generation?id=\${encodeURIComponent(body.id)}\`;
  for (let attempt = 0; attempt < 8; attempt += 1) {
    const response = await fetch(url, {
      headers: { Authorization: \`Bearer \${API_KEY}\` },
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
    const canonical = defining.filter(
      ({ taskId }) =>
        !FETCH_GENERATION_VARIANTS.some((variant) => variant === taskId)
    );
    for (const { taskId, source } of canonical) {
      expect(
        extractBlock(source, "async function fetchGeneration("),
        taskId
      ).toBe(CANONICAL_FETCH_GENERATION);
    }
  });

  test("every fetchGeneration copy carries the canonical 404 control probe", () => {
    const defining = verifierSources().filter((v) =>
      v.source.includes("async function fetchGeneration")
    );
    expect(defining.length).toBeGreaterThan(0);
    for (const { taskId, source } of defining) {
      expect(
        extractBlock(source, "async function controlGenerationRetrievable("),
        taskId
      ).toBe(CANONICAL_CONTROL_PROBE);
    }
  });
});
