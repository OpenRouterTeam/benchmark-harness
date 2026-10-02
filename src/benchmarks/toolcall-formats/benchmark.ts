import { fromIterable } from "effect/Chunk";
import { fail, succeed as effectSucceed } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { succeed as layerSucceed } from "effect/Layer";
import { fromChunk } from "effect/Stream";

import type { Sample } from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import { Dataset } from "../../harness/dataset";
import type { ModelService } from "../../harness/model";
import type { SolverService } from "../../harness/solver";
import { generate } from "../../harness/solver";
import { definedValues } from "../../internal/guards";
import type {
  FixedTemperatureInferenceOverride,
  ToolCallFormatsBenchmarkConfig,
} from "../benchmark-config";
import { TOOLCALL_FORMATS_META } from "../benchmark-meta";
import { defineSingleTurnBenchmark } from "../define-single-turn-benchmark";
import type { Benchmark } from "../types";
import type { ToolCallFormatCase } from "./cases";
import {
  getToolCallFormatCase,
  TOOLCALL_FORMAT_CASES,
  variantLabel,
} from "./cases";
import { toolCallFormatsScorer } from "./scorer";

export const TOOLCALL_FORMATS_SYSTEM_MESSAGE =
  "You are a function-calling assistant. Fulfil every request by calling the provided tools directly, without asking for confirmation. Pass values exactly as given. For any parameter the request does not give a value for, omit it, or pass null if its schema requires it and allows null; never invent defaults. When a request needs several independent calls, make all of them in this turn.";

function toolCallFormatCaseToSample(entry: ToolCallFormatCase): Sample {
  return {
    id: entry.id,
    input: entry.prompt,
    target: { text: JSON.stringify(entry.calls) },
    metadata: {
      scenario: entry.scenarioId,
      variant: variantLabel(entry.variant),
      encoding: entry.variant.encoding,
      strict: entry.variant.strict,
    },
  };
}

export const TOOLCALL_FORMATS_SAMPLES: readonly Sample[] =
  TOOLCALL_FORMAT_CASES.map(toolCallFormatCaseToSample);

export function makeToolCallFormatsDatasetLayer(): Layer<Dataset> {
  return layerSucceed(
    Dataset,
    Dataset.of({
      stream: (opts) =>
        fromChunk(
          fromIterable(TOOLCALL_FORMATS_SAMPLES.slice(opts?.start, opts?.end))
        ),
      size: effectSucceed(TOOLCALL_FORMATS_SAMPLES.length),
    })
  );
}

export function toolCallFormatsSolver(
  model: ModelService,
  opts: {
    readonly endpointId?: string;
    readonly inference: FixedTemperatureInferenceOverride;
  }
): SolverService {
  return (state) => {
    const entry = getToolCallFormatCase(state.sample.id);
    if (entry === undefined) {
      return fail(
        new SolverError({
          message: `no toolcall_formats case for sample ${state.sample.id}`,
        })
      );
    }
    return generate(model, {
      ...definedValues(opts.inference),
      ...definedValues({ endpointId: opts.endpointId }),
      temperature: TOOLCALL_FORMATS_META.temperature,
      tools: entry.tools,
    })({
      ...state,
      messages: [
        { role: MessageRole.System, content: TOOLCALL_FORMATS_SYSTEM_MESSAGE },
        ...state.messages,
      ],
    });
  };
}

export const TOOLCALL_FORMATS_BENCHMARK: Benchmark = defineSingleTurnBenchmark({
  id: TOOLCALL_FORMATS_META.id,
  temperature: TOOLCALL_FORMATS_META.temperature,
  defaultEpochs: TOOLCALL_FORMATS_META.defaultEpochs,
  isConfig: (config): config is ToolCallFormatsBenchmarkConfig =>
    config.benchmarkId === TOOLCALL_FORMATS_META.id,
  makeDatasetLayer: makeToolCallFormatsDatasetLayer,
  scorer: toolCallFormatsScorer,
  makeSolver: (model, config) =>
    toolCallFormatsSolver(
      model,
      definedValues({
        endpointId: config.endpointId,
        inference: {
          maxTokens: config.maxTokens,
          reasoningEffort: config.reasoningEffort,
          timeoutMs: config.timeoutMs,
          sort: config.sort,
          providerOnly: config.providerOnly,
          providerIgnore: config.providerIgnore,
          allowFallbacks: config.allowFallbacks,
          cloudflareVersion: config.cloudflareVersion,
          experimentIds: config.experimentIds,
          costTier: config.costTier,
          costQualityTradeoff: config.costQualityTradeoff,
          pinModel: config.pinModel,
          switchyardAlgorithm: config.switchyardAlgorithm,
        },
      })
    ),
});
