import { formatIso, unsafeNow } from "effect/DateTime";

import type { Citation } from "../../harness/core";
import { wLog } from "../../internal/log";
import type { ResponsesResult } from "../../providers/responses-client";
import {
  extractCitations,
  findOutputItems,
} from "../../providers/responses-client";
import { detectContamination } from "./contamination";
import type { DracoPanelConfig, GenerationResult } from "./schemas";
import {
  isContentRefusal,
  isFusionPanelRefusal,
  summarizeToolInvocations,
  verifyFusion,
} from "./verification";

export function buildGenerationResult(opts: {
  taskId: string;
  config: DracoPanelConfig;
  result: ResponsesResult;
  latencyMs?: number;
}): GenerationResult {
  const { taskId, config, result } = opts;
  const content = result.text;
  const toolInvocations = summarizeToolInvocations(result.output);
  const citations = extractCitations(result.output);
  const fusionItem =
    config.type === "fusion"
      ? (findOutputItems(result.output, "openrouter:fusion")[0] ?? null)
      : null;
  if (config.type === "fusion") {
    const [, fusionErr] = verifyFusion(result.output, config);
    if (fusionErr) {
      if (isFusionPanelRefusal(fusionItem)) {
        return generation(taskId, config, {
          status: "refused",
          content: null,
          error: `Fusion panel refusal: ${fusionErr}`,
          fusionItem,
          result,
          latencyMs: opts.latencyMs,
        });
      }
      return generation(taskId, config, {
        status: "failed",
        content: null,
        error: `Fusion verification failed: ${fusionErr}`,
        fusionItem,
        result,
        latencyMs: opts.latencyMs,
      });
    }
  }
  if (result.status !== "completed") {
    return generation(taskId, config, {
      status: "failed",
      content: null,
      error: `Response status ${result.status ?? "null"} (expected 'completed')`,
      fusionItem,
      result,
      latencyMs: opts.latencyMs,
    });
  }
  if (isContentRefusal(content, result.status, toolInvocations)) {
    return generation(taskId, config, {
      status: "refused",
      content: null,
      error: "Content refusal (empty completed response, no tool invocations)",
      fusionItem,
      result,
      latencyMs: opts.latencyMs,
    });
  }
  if (content.trim().length === 0) {
    return generation(taskId, config, {
      status: "failed",
      content: null,
      error: "Empty response content",
      fusionItem,
      result,
      latencyMs: opts.latencyMs,
    });
  }
  return generation(taskId, config, {
    status: "ok",
    content,
    fusionItem,
    result,
    latencyMs: opts.latencyMs,
    toolInvocations,
    citations,
  });
}

interface GenerationInput {
  readonly status: "ok" | "failed" | "refused";
  readonly content: string | null;
  readonly error?: string;
  readonly fusionItem?: Record<string, unknown> | null;
  readonly result: ResponsesResult;
  readonly toolInvocations?: Record<string, unknown>[];
  readonly citations?: readonly Citation[];
  readonly latencyMs?: number;
}

function generation(
  taskId: string,
  config: DracoPanelConfig,
  input: GenerationInput
): GenerationResult {
  const contaminationSignals = detectContamination({
    content: input.content,
    fusionAnalysis: input.fusionItem ?? null,
  });
  if (contaminationSignals.length > 0) {
    wLog("draco contamination signals", {
      task_id: taskId,
      experiment: config.name,
      signals: contaminationSignals,
    });
  }
  return {
    taskId,
    experimentName: config.name,
    status: input.status,
    content: input.content,
    model:
      input.result.model ??
      (config.type === "fusion"
        ? (config.synthesisModel ?? "openrouter/fusion")
        : (config.model ?? "unknown")),
    generationId: input.result.id,
    usage: input.result.usage,
    cost: extractCost(input.result.usage),
    latencyMs: input.latencyMs ?? null,
    error: input.error ?? null,
    fusionAnalysis: input.fusionItem ?? null,
    toolInvocations: input.toolInvocations ?? [],
    contaminationSignals,
    citations: [...(input.citations ?? [])],
    timestamp: formatIso(unsafeNow()),
  };
}

export function extractCost(
  usage: Record<string, unknown> | null
): number | null {
  if (!usage) {
    return null;
  }
  const cost = usage["cost"];
  return typeof cost === "number" ? cost : null;
}
