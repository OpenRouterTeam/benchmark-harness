import { isRecord } from "../../internal/guards";
import { findOutputItems } from "../../providers/responses-client";
import type { DracoPanelConfig } from "./schemas";

const FUSION_REFUSAL_ERROR_MARKER = "without producing any text";

export function isContentRefusal(
  content: string,
  status: string | null,
  toolInvocations: unknown[]
): boolean {
  if (status !== "completed") {
    return false;
  }
  if (content.trim().length > 0) {
    return false;
  }
  return toolInvocations.length === 0;
}

export function isFusionPanelRefusal(
  fusionItem: Record<string, unknown> | null
): boolean {
  if (!fusionItem) {
    return false;
  }
  const failed = fusionItem["failed_models"];
  if (!Array.isArray(failed) || failed.length === 0) {
    return false;
  }
  return failed.every((fm) => {
    if (!isRecord(fm)) {
      return false;
    }
    const model = String(fm["model"] ?? "").toLowerCase();
    const error = String(fm["error"] ?? "");
    return (
      model.includes("fable") && error.includes(FUSION_REFUSAL_ERROR_MARKER)
    );
  });
}

export function verifyFusion(
  output: readonly Record<string, unknown>[],
  config: DracoPanelConfig
): [Record<string, unknown> | null, string | null] {
  const fusionItems = findOutputItems(output, "openrouter:fusion");
  if (fusionItems.length === 0) {
    return [
      null,
      `No openrouter:fusion output item in response — fusion did not run. Output types: ${JSON.stringify(output.map((i) => i["type"]))}`,
    ];
  }
  const completed = fusionItems.filter((i) => i["status"] === "completed");
  if (completed.length === 0) {
    return [
      fusionItems.at(-1)!,
      `No openrouter:fusion item completed (statuses: ${JSON.stringify(fusionItems.map((i) => i["status"]))})`,
    ];
  }
  if (config.analysisModels.length === 0) {
    return [completed.at(-1)!, null];
  }
  const expected = counter(config.analysisModels);
  const matching = completed.filter((item) => {
    const responses = item["responses"];
    if (!Array.isArray(responses)) {
      return false;
    }
    const observed = counter(
      responses
        .map((r) => (isRecord(r) ? r["model"] : null))
        .filter((m): m is string => typeof m === "string")
    );
    return mapEquals(expected, observed);
  });
  if (matching.length > 0) {
    return [matching.at(-1)!, null];
  }
  const observedPanels = completed.map((i) => {
    const responses = i["responses"];
    const models = Array.isArray(responses)
      ? responses.map((r) => (isRecord(r) ? r["model"] : "?"))
      : [];
    return [
      ...counter(
        models.filter((m): m is string => typeof m === "string")
      ).entries(),
    ].sort();
  });
  return [
    completed.at(-1)!,
    `Panel mismatch: configured analysis_models=${[...expected.entries()].sort()} but no openrouter:fusion item has the full panel (observed panels: ${JSON.stringify(observedPanels)})`,
  ];
}

function counter(items: string[]): Map<string, number> {
  const m = new Map<string, number>();
  for (const item of items) {
    m.set(item, (m.get(item) ?? 0) + 1);
  }
  return m;
}

function mapEquals(a: Map<string, number>, b: Map<string, number>): boolean {
  if (a.size !== b.size) {
    return false;
  }
  for (const [k, v] of a) {
    if (b.get(k) !== v) {
      return false;
    }
  }
  return true;
}

export function summarizeToolInvocations(
  output: readonly Record<string, unknown>[]
): Record<string, unknown>[] {
  return output
    .filter((item) => {
      const t = item["type"];
      return t !== "message" && t !== "reasoning" && t !== "openrouter:fusion";
    })
    .map((item) => ({
      type: item["type"],
      status: item["status"] ?? null,
      action: item["action"] ?? null,
    }));
}
