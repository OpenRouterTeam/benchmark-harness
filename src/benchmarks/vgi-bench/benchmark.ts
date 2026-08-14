import type { Layer } from "effect/Layer";

import type { HfDatasetConfig } from "../../datasets/huggingface";
import { makeHfDatasetLayer } from "../../datasets/huggingface";
import type { ContentPart, Sample } from "../../harness/core";
import type { Dataset as DatasetTag } from "../../harness/dataset";
import type { SampleScore } from "../../harness/metric";
import { aggregateScores } from "../../harness/metric";
import type { GenerateConfig, ModelService } from "../../harness/model";
import type { RunResult } from "../../harness/run";
import type { SolverService } from "../../harness/solver";
import { generate } from "../../harness/solver";
import { definedValues } from "../../internal/guards";
import type { RetryConfig } from "../../runtime/retry";
import type {
  FixedTemperatureInferenceOverride,
  VgiBenchmarkConfig,
} from "../benchmark-config";
import { VGI_BENCH_META } from "../benchmark-meta";
import { defineChatBenchmark } from "../define-chat-benchmark";
import { mcqScorer } from "../scorers/mcq/scorer";
import type { Benchmark } from "../types";

export const VGI_BENCH_DATASET_PATH = "Seldon-Technologies/VGIBench";

const VGI_BENCH_CONFIG = "default";
const VGI_BENCH_SPLIT = "train";

export const VGI_BENCH_DEFAULT_REVISION = "v1.0.1";

const PROXY_SUFFIX = "_proxy_v2";

const LETTERS = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";

export const VGI_BENCH_TEMPERATURE = 0;

export function buildVgiBenchPrompt(
  question: string,
  answers: readonly string[]
): string {
  const options = answers
    .map((answer, index) => `${LETTERS[index]!}) ${answer}`)
    .join("\n");
  const letters = answers.map((_, index) => LETTERS[index]!).join("");
  return (
    "Watch the video and answer the multiple choice question about it. " +
    `The last line of your response should be of the following format: 'Answer: $LETTER' (without quotes) where LETTER is one of ${letters}.\n\n` +
    `Question: ${question}\n\n` +
    `${options}`
  );
}

function asString(value: unknown, field: string): string {
  if (typeof value !== "string") {
    throw new TypeError(`vgi-bench record field "${field}" was not a string`);
  }
  return value;
}

function asNumber(value: unknown, field: string): number {
  if (typeof value !== "number" || !Number.isFinite(value)) {
    throw new TypeError(`vgi-bench record field "${field}" was not a number`);
  }
  return value;
}

function asStringArray(value: unknown, field: string): readonly string[] {
  if (!Array.isArray(value)) {
    throw new TypeError(`vgi-bench record field "${field}" was not an array`);
  }
  return value.map((item, index) => {
    if (typeof item !== "string") {
      throw new TypeError(
        `vgi-bench record field "${field}[${index}]" was not a string`
      );
    }
    return item;
  });
}

export function downscaledVideoUrl(url: string): string {
  const parsed = new URL(url);
  const pathname = parsed.pathname;
  const slash = pathname.lastIndexOf("/");
  const dir = slash !== -1 ? pathname.slice(0, slash + 1) : "";
  const filename = slash !== -1 ? pathname.slice(slash + 1) : pathname;
  const dot = filename.lastIndexOf(".");
  const rewritten =
    dot === -1
      ? `${filename}${PROXY_SUFFIX}`
      : `${filename.slice(0, dot)}${PROXY_SUFFIX}.${filename.slice(dot + 1)}`;
  parsed.pathname = `${dir}${rewritten}`;
  return parsed.toString();
}

export interface VgiBenchRecordToSampleOptions {
  readonly downscaledVideos?: boolean;
}

export function vgiBenchRecordToSample(
  record: Readonly<Record<string, unknown>>,
  _index: number,
  opts?: VgiBenchRecordToSampleOptions
): Sample {
  const questionId = asNumber(record["question_id"], "question_id");
  const videoId = asString(record["video_id"], "video_id");
  let videoUrl = asString(record["video_url"], "video_url");
  if (opts?.downscaledVideos === true) {
    videoUrl = downscaledVideoUrl(videoUrl);
  }
  const question = asString(record["question"], "question");
  const questionType = asString(record["question_type"], "question_type");
  const answers = asStringArray(record["answers"], "answers");
  const correctAnswer = asNumber(record["correct_answer"], "correct_answer");
  if (correctAnswer < 0 || correctAnswer >= answers.length) {
    throw new TypeError(
      `vgi-bench record question_id=${questionId} has correct_answer=${correctAnswer} outside answers range [0, ${answers.length})`
    );
  }
  const prompt = buildVgiBenchPrompt(question, answers);
  const contentParts: ContentPart[] = [
    { type: "video_url", videoUrl: { url: videoUrl } },
    { type: "text", text: prompt },
  ];
  const family = questionType.includes("/")
    ? questionType.slice(0, questionType.indexOf("/"))
    : questionType;
  return {
    id: `vgi_bench-${questionId}`,
    input: prompt,
    target: { text: LETTERS[correctAnswer]! },
    contentParts,
    metadata: {
      question_id: questionId,
      video_id: videoId,
      question_type: questionType,
      family,
      ...(opts?.downscaledVideos === true && {
        downscaled_videos: true,
      }),
    },
  };
}

interface VgiBenchDatasetOpts extends VgiBenchRecordToSampleOptions {
  readonly retry?: RetryConfig;
  readonly revision?: string;
}

export function makeVgiBenchDatasetLayer(
  opts?: VgiBenchDatasetOpts
): Layer<DatasetTag> {
  const config: HfDatasetConfig = {
    dataset: VGI_BENCH_DATASET_PATH,
    config: VGI_BENCH_CONFIG,
    split: VGI_BENCH_SPLIT,
    recordToSample: (record, idx) =>
      vgiBenchRecordToSample(record, idx, {
        ...(opts?.downscaledVideos !== undefined && {
          downscaledVideos: opts.downscaledVideos,
        }),
      }),
    ...(opts?.revision !== undefined && { revision: opts.revision }),
    ...(opts?.retry !== undefined && { retry: opts.retry }),
  };
  return makeHfDatasetLayer(config);
}

export function vgiBenchSolver(
  model: ModelService,
  opts?: {
    readonly endpointId?: string;
    readonly inference?: FixedTemperatureInferenceOverride;
  }
): SolverService {
  const config: GenerateConfig = {
    temperature: VGI_BENCH_TEMPERATURE,
    ...definedValues(opts?.inference ?? {}),
    ...(opts?.endpointId !== undefined && { endpointId: opts.endpointId }),
  };
  return generate(model, config);
}

function vgiBenchRunLevelScores(result: RunResult): readonly {
  name: string;
  metrics: Readonly<
    Record<
      string,
      {
        readonly value: number;
      }
    >
  >;
}[] {
  const byFamily = new Map<string, SampleScore[]>();
  const byType = new Map<string, SampleScore[]>();
  for (const sampleScore of result.sampleScores) {
    const family = sampleScore.metadata?.["family"];
    if (typeof family === "string") {
      const current = byFamily.get(family);
      if (current === undefined) {
        byFamily.set(family, [sampleScore]);
      } else {
        current.push(sampleScore);
      }
    }
    const questionType = sampleScore.metadata?.["question_type"];
    if (typeof questionType === "string") {
      const current = byType.get(questionType);
      if (current === undefined) {
        byType.set(questionType, [sampleScore]);
      } else {
        current.push(sampleScore);
      }
    }
  }
  const group = (
    name: string,
    bucket: Map<string, SampleScore[]>
  ): readonly {
    name: string;
    metrics: Readonly<Record<string, { readonly value: number }>>;
  }[] =>
    [...bucket.entries()]
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([key, sampleScores]) => {
        const metrics = aggregateScores(sampleScores);
        return {
          name: `${name}_${key}`,
          metrics: {
            accuracy: { value: metrics.accuracy },
            total_questions: { value: metrics.totalQuestions },
          },
        };
      });
  return [
    {
      name: "vgi_bench",
      metrics: {
        accuracy: { value: result.metrics.accuracy },
        total_questions: { value: result.metrics.totalQuestions },
      },
    },
    ...group("vgi_bench_family", byFamily),
    ...group("vgi_bench_type", byType),
  ];
}

const VGI_BENCH_CHAT_BENCHMARK = defineChatBenchmark({
  id: VGI_BENCH_META.id,
  temperature: VGI_BENCH_TEMPERATURE,
  defaultEpochs: VGI_BENCH_META.defaultEpochs,
  isConfig: (config): config is VgiBenchmarkConfig =>
    config.benchmarkId === "vgi_bench",
  makeDatasetLayer: (retryConfig) =>
    makeVgiBenchDatasetLayer(
      retryConfig !== undefined
        ? {
            retry: retryConfig,
            revision: VGI_BENCH_DEFAULT_REVISION,
          }
        : { revision: VGI_BENCH_DEFAULT_REVISION }
    ),
  makeDatasetLayerForConfig: (config, retryConfig) =>
    makeVgiBenchDatasetLayer({
      downscaledVideos: config.downscaledVideos,
      ...(config.datasetRevision !== undefined
        ? { revision: config.datasetRevision }
        : { revision: VGI_BENCH_DEFAULT_REVISION }),
      ...(retryConfig !== undefined && { retry: retryConfig }),
    }),
  scorer: mcqScorer,
  makeSolver: (model, config) =>
    vgiBenchSolver(model, {
      ...(config.endpointId !== undefined && { endpointId: config.endpointId }),
      inference: {
        maxTokens: config.maxTokens,
        reasoningEffort: config.reasoningEffort,
        timeoutMs: config.timeoutMs,
        sort: config.sort,
        cloudflareVersion: config.cloudflareVersion,
        costTier: config.costTier,
        costQualityTradeoff: config.costQualityTradeoff,
      },
    }),
});

export const VGI_BENCH_BENCHMARK: Benchmark = {
  ...VGI_BENCH_CHAT_BENCHMARK,
  runLevelScores: vgiBenchRunLevelScores,
};
