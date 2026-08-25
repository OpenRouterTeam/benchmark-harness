import type { Layer } from "effect/Layer";

import { HfImageSchema, makeHfDatasetLayer } from "../datasets/huggingface";
import type { ImageDetail } from "../harness/constants";
import type { ContentPart, Sample } from "../harness/core";
import type { Dataset as DatasetTag } from "../harness/dataset";
import type { GenerateConfig, ModelService } from "../harness/model";
import type { SolverService } from "../harness/solver";
import { chain, generate, systemMessage } from "../harness/solver";
import { Either } from "../internal/either";
import { definedValues, isDefinedAndNotNull } from "../internal/guards";
import { parseSchema } from "../internal/zod";
import type { RetryConfig } from "../runtime/retry";
import type {
  InferenceOverride,
  GeminiMediaResolution,
  MmmuProVisionBenchmarkConfig,
} from "./benchmark-config";
import { MMMU_PRO_VISION_META } from "./benchmark-meta";
import { defineSingleTurnBenchmark } from "./define-single-turn-benchmark";
import { MMMU_SYSTEM_MESSAGE, parseOptions } from "./mmmu-shared";
import { buildDynamicMcqPrompt } from "./scorers/mcq/dynamic-prompt";
import { mcqScorer } from "./scorers/mcq/scorer";
import type { Benchmark } from "./types";

const MMMU_PRO_DATASET_PATH = "MMMU/MMMU_Pro";

const MMMU_PRO_VISION_SUBSET = "vision";

const MMMU_PRO_SPLIT = "test";

const DEFAULT_QUESTION =
  "Use the image to answer the question. Choose the best option.";

function asString(value: unknown, fallback: string): string {
  return typeof value === "string" ? value : fallback;
}

export function mmmuProVisionRecordToSample(
  record: Readonly<Record<string, unknown>>,
  index: number,
  imageDetail?: ImageDetail
): Sample {
  const id = asString(record["id"], `mmmu-pro-vision-${index}`);
  const questionRaw = record["question"];
  const question =
    typeof questionRaw === "string" && questionRaw.trim().length > 0
      ? questionRaw.trim()
      : DEFAULT_QUESTION;
  const answer = asString(record["answer"], "");
  const options = parseOptions(record["options"]);
  const input =
    options.length > 0
      ? buildDynamicMcqPrompt(question, options)
      : `${question}\n\nAnswer succinctly.`;
  const contentParts: ContentPart[] = [{ type: "text", text: input }];
  let numImages = 0;
  const imageField = record["image"];
  if (isDefinedAndNotNull(imageField)) {
    const parsed = parseSchema(HfImageSchema, imageField);
    if (Either.isRight(parsed)) {
      contentParts.push({
        type: "image_url",
        imageUrl: {
          url: parsed.right.src,
          ...(imageDetail !== undefined && { detail: imageDetail }),
        },
      });
      numImages++;
    }
  }
  for (let i = 1; i <= 7; i++) {
    const imgField = record[`image_${i}`];
    if (!isDefinedAndNotNull(imgField)) {
      continue;
    }
    const parsed = parseSchema(HfImageSchema, imgField);
    if (Either.isLeft(parsed)) {
      continue;
    }
    contentParts.push({
      type: "image_url",
      imageUrl: {
        url: parsed.right.src,
        ...(imageDetail !== undefined && { detail: imageDetail }),
      },
    });
    numImages++;
  }
  return {
    id,
    input,
    target: { text: answer },
    contentParts: numImages > 0 ? contentParts : undefined,
    metadata: {
      question_type: "multiple-choice",
      subject: record["subject"] ?? "",
      topic_difficulty: record["topic_difficulty"] ?? "",
      answer,
      num_images: numImages,
    },
  };
}

interface MmmuProVisionDatasetOpts {
  readonly imageDetail?: ImageDetail;
  readonly retry?: RetryConfig;
}

export function makeMmmuProVisionDatasetLayer(
  opts?: MmmuProVisionDatasetOpts
): Layer<DatasetTag> {
  return makeHfDatasetLayer({
    dataset: MMMU_PRO_DATASET_PATH,
    config: MMMU_PRO_VISION_SUBSET,
    split: MMMU_PRO_SPLIT,
    recordToSample: (record, idx) =>
      mmmuProVisionRecordToSample(record, idx, opts?.imageDetail),
    ...(opts?.retry !== undefined && { retry: opts.retry }),
  });
}

export function mmmuProVisionSolver(
  model: ModelService,
  opts?: {
    readonly endpointId?: string;
    readonly inference?: InferenceOverride;
    readonly mediaResolution?: GeminiMediaResolution;
  }
): SolverService {
  const config: GenerateConfig = {
    temperature: 0,
    ...definedValues(opts?.inference ?? {}),
    ...(opts?.endpointId !== undefined && { endpointId: opts.endpointId }),
    ...(opts?.mediaResolution !== undefined && {
      extraBody: { media_resolution: opts.mediaResolution },
    }),
  };
  return chain(systemMessage(MMMU_SYSTEM_MESSAGE), generate(model, config));
}

export const MMMU_PRO_VISION_BENCHMARK: Benchmark = defineSingleTurnBenchmark({
  id: "mmmu_pro_vision",
  temperature: 0,
  defaultEpochs: MMMU_PRO_VISION_META.defaultEpochs,
  isConfig: (config): config is MmmuProVisionBenchmarkConfig =>
    config.benchmarkId === "mmmu_pro_vision",
  makeDatasetLayer: (retryConfig) =>
    makeMmmuProVisionDatasetLayer(
      retryConfig !== undefined ? { retry: retryConfig } : undefined
    ),
  makeDatasetLayerForConfig: (config, retryConfig) =>
    makeMmmuProVisionDatasetLayer({
      imageDetail: config.imageDetail,
      ...(retryConfig !== undefined && { retry: retryConfig }),
    }),
  scorer: mcqScorer,
  makeSolver: (model, config) =>
    mmmuProVisionSolver(model, {
      ...(config.endpointId !== undefined && { endpointId: config.endpointId }),
      ...(config.mediaResolution !== undefined && {
        mediaResolution: config.mediaResolution,
      }),
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
