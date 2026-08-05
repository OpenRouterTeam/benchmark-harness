import { createHash } from "node:crypto";

import { isRecord } from "../../internal/guards";
import type { StageKey } from "./artifact-store";
import type { DracoPanelConfig } from "./schemas";

const LEGACY_PRODUCTION_FUSION_MODE = "production";

export function stableStringify(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map(stableStringify).join(",")}]`;
  }
  if (isRecord(value)) {
    return `{${Object.keys(value)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${stableStringify(value[k])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function hash(inputs: Record<string, unknown>): string {
  return createHash("sha256")
    .update(stableStringify(inputs))
    .digest("hex")
    .slice(0, 32);
}

export function promptSha(prompt: string): string {
  return createHash("sha256").update(prompt).digest("hex").slice(0, 16);
}

export function toolSurfaceSha(tools: readonly unknown[]): string {
  return createHash("sha256")
    .update(stableStringify([...tools]))
    .digest("hex")
    .slice(0, 16);
}

export function generationConfigSha(config: DracoPanelConfig): string {
  const {
    judgeModel,
    judgeRuns,
    judgeTemperature,
    judgeReasoningEffort,
    criterionConcurrency,
    name,
    description,
    timeout,
    concurrency,
    cacheNamespace,
    ...generationFields
  } = config;
  return createHash("sha256")
    .update(
      stableStringify({
        ...generationFields,
        fusionMode: LEGACY_PRODUCTION_FUSION_MODE,
      })
    )
    .digest("hex")
    .slice(0, 32);
}

export function soloGenKey(opts: {
  taskId: string;
  config: DracoPanelConfig;
  prompt: string;
  tools: readonly unknown[];
}): StageKey {
  const { taskId, config, prompt, tools } = opts;
  return {
    stage: "generation",
    key: hash({
      stage: "solo-gen",
      task_id: taskId,
      model: config.model,
      prompt: promptSha(prompt),
      tools: toolSurfaceSha(tools),
      version: config.versionOverride ?? null,
      provider: config.provider ?? null,
    }),
  };
}

export function productionFusionGenKey(opts: {
  taskId: string;
  config: DracoPanelConfig;
  prompt: string;
}): StageKey {
  const { taskId, config, prompt } = opts;
  return {
    stage: "generation",
    key: hash({
      stage: "production-fusion-gen",
      task_id: taskId,
      config: generationConfigSha(config),
      prompt: promptSha(prompt),
      version: config.versionOverride ?? null,
    }),
  };
}

export function judgeKey(opts: {
  generationKey: string;
  judgeModel: string;
  criterionId: string;
  runNum: number;
  judgePrompt: string;
  judgeTemperature: number | undefined;
  judgeReasoningEffort: string | undefined;
  versionOverride: string | undefined;
}): StageKey {
  const {
    generationKey,
    judgeModel,
    criterionId,
    runNum,
    judgePrompt,
    judgeTemperature,
    judgeReasoningEffort,
    versionOverride,
  } = opts;
  return {
    stage: "judge",
    key: hash({
      stage: "judge",
      generation_key: generationKey,
      judge_model: judgeModel,
      criterion_id: criterionId,
      run: runNum,
      judge_prompt: promptSha(judgePrompt),
      judge_temperature: judgeTemperature ?? null,
      judge_reasoning_effort: judgeReasoningEffort ?? null,
      version: versionOverride ?? null,
    }),
  };
}
