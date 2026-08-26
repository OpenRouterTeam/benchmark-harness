import type { Layer } from "effect/Layer";

import type { HfDatasetConfig } from "../../datasets/huggingface";
import { makeHfDatasetLayer } from "../../datasets/huggingface";
import type { Sample } from "../../harness/core";
import type { Dataset } from "../../harness/dataset";
import type { GenerateConfig, ModelService } from "../../harness/model";
import type { SolverService } from "../../harness/solver";
import { generate } from "../../harness/solver";
import { Either } from "../../internal/either";
import { definedValues } from "../../internal/guards";
import { parseSchema, z } from "../../internal/zod";
import type { RetryConfig } from "../../runtime/retry";
import type {
  IfStructBenchmarkConfig,
  InferenceOverride,
} from "../benchmark-config";
import { IFSTRUCT_META } from "../benchmark-meta";
import { defineChatBenchmark } from "../define-chat-benchmark";
import type { Benchmark } from "../types";
import type {
  IfStructRequirements,
  JsonSchemaNode,
  TopLevelCount,
} from "./schema";
import {
  JsonSchemaNodeSchema,
  OutputFormat,
  TopLevelCountSchema,
} from "./schema";
import { ifStructScorer } from "./scorer";

const IfStructRecordSchema = z.object({
  doc_id: z.number().int(),
  entity_type: z.string(),
  prompt: z.string(),
  output_format: z.enum([OutputFormat.Json, OutputFormat.Yaml]),
  top_level_count: z.string(),
  top_level_key: z.string(),
  require_wrapper_key: z.boolean(),
  require_code_block: z.boolean(),
  require_no_commentary: z.boolean(),
  json_schema: z.string(),
});

export const IFSTRUCT_TEMPERATURE = 0;

export function ifStructRecordToSample(
  record: Readonly<Record<string, unknown>>
): Sample {
  const parsed = parseSchema(IfStructRecordSchema, record);
  if (Either.isLeft(parsed)) {
    throw new TypeError(
      `ifstruct record failed validation: ${parsed.left.message}`
    );
  }
  const row = parsed.right;
  const requirements: IfStructRequirements = {
    jsonSchema: parseJsonSchema(row.json_schema),
    topLevelCount: parseTopLevelCount(row.top_level_count),
    topLevelKey: row.top_level_key === "" ? null : row.top_level_key,
    requireWrapperKey: row.require_wrapper_key,
    requireCodeBlock: row.require_code_block,
    requireNoCommentary: row.require_no_commentary,
    outputFormat: row.output_format,
  };
  return {
    id: `ifstruct-${row.doc_id}`,
    input: row.prompt,
    target: { text: row.output_format },
    metadata: {
      ...requirements,
      docId: row.doc_id,
      entityType: row.entity_type,
    },
  };
}

function parseJsonSchema(raw: string): JsonSchemaNode {
  const json = Either.try((): unknown => JSON.parse(raw));
  if (Either.isLeft(json)) {
    throw new TypeError(
      `ifstruct json_schema is not valid JSON: ${String(json.left)}`
    );
  }
  const parsed = parseSchema(JsonSchemaNodeSchema, json.right);
  if (Either.isLeft(parsed)) {
    throw new TypeError(
      `ifstruct json_schema failed validation: ${parsed.left.message}`
    );
  }
  return parsed.right;
}

function parseTopLevelCount(raw: string): TopLevelCount {
  const trimmed = raw.trim();
  if (trimmed === "") {
    return null;
  }
  const json = Either.try((): unknown => JSON.parse(trimmed));
  if (Either.isLeft(json)) {
    throw new TypeError(
      `ifstruct top_level_count is not valid JSON: ${trimmed}`
    );
  }
  const parsed = parseSchema(TopLevelCountSchema, json.right);
  if (Either.isLeft(parsed)) {
    throw new TypeError(
      `ifstruct top_level_count failed validation: ${trimmed}`
    );
  }
  return parsed.right;
}

export const IFSTRUCT_DATASET = {
  dataset: "LiquidAI/ifstruct-v1.0",
  config: "default",
  split: "test",
  recordToSample: ifStructRecordToSample,
} as const satisfies Omit<HfDatasetConfig, "pageSize">;

export function ifStructSolver(
  model: ModelService,
  opts?: {
    readonly endpointId?: string;
    readonly inference?: InferenceOverride;
  }
): SolverService {
  const config: GenerateConfig = {
    temperature: IFSTRUCT_TEMPERATURE,
    ...definedValues(opts?.inference ?? {}),
    ...(opts?.endpointId !== undefined && { endpointId: opts.endpointId }),
  };
  return generate(model, config);
}

export function makeIfStructDatasetLayer(
  retryConfig?: RetryConfig
): Layer<Dataset> {
  return makeHfDatasetLayer({
    ...IFSTRUCT_DATASET,
    ...(retryConfig !== undefined && { retry: retryConfig }),
  });
}

export const IFSTRUCT_BENCHMARK: Benchmark = defineChatBenchmark({
  id: "ifstruct",
  temperature: IFSTRUCT_TEMPERATURE,
  defaultEpochs: IFSTRUCT_META.defaultEpochs,
  isConfig: (config): config is IfStructBenchmarkConfig =>
    config.benchmarkId === "ifstruct",
  makeDatasetLayer: makeIfStructDatasetLayer,
  scorer: ifStructScorer,
  makeSolver: (model, config) =>
    ifStructSolver(model, {
      ...(config.endpointId !== undefined && { endpointId: config.endpointId }),
      inference: {
        temperature: config.temperature,
        maxTokens: config.maxTokens,
        reasoningEffort: config.reasoningEffort,
        timeoutMs: config.timeoutMs,
        sort: config.sort,
        providerOnly: config.providerOnly,
        providerIgnore: config.providerIgnore,
        allowFallbacks: config.allowFallbacks,
        cloudflareVersion: config.cloudflareVersion,
        costTier: config.costTier,
        costQualityTradeoff: config.costQualityTradeoff,
      },
    }),
});
