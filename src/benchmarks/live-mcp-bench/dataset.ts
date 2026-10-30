import { map as mapEffect } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { effect as layerEffect, provide as layerProvide } from "effect/Layer";
import * as Result from "effect/Result";
import type { Stream } from "effect/Stream";
import {
  drop as streamDrop,
  filter as streamFilter,
  runCount,
  take as streamTake,
} from "effect/Stream";

import { makeHfDatasetLayer } from "../../datasets/huggingface";
import type { DatasetError, Sample } from "../../harness/core";
import type { DatasetStreamOptions } from "../../harness/dataset";
import { Dataset } from "../../harness/dataset";
import { definedValues } from "../../internal/guards";
import { parseSchema, z } from "../../internal/zod";
import type { RetryConfig } from "../../runtime/retry";
import type { LiveMcpBenchCategory } from "./schema";
import { LIVE_MCP_BENCH_CATEGORIES, LIVE_MCP_BENCH_ID } from "./schema";

export const LIVE_MCP_BENCH_DATASET = {
  dataset: "ICIP/LiveMCPBench",
  config: "default",
  split: "test",
} as const;

export const LIVE_MCP_BENCH_TASK_METADATA_KEY = "task";

export const LiveMcpBenchTaskSchema = z.object({
  taskId: z.string(),
  category: z.enum(LIVE_MCP_BENCH_CATEGORIES),
  answers: z.string(),
  fileName: z.string(),
  steps: z.string(),
  annotatedTools: z.array(z.string()).readonly(),
});

export type LiveMcpBenchTask = z.infer<typeof LiveMcpBenchTaskSchema>;

export interface LiveMcpBenchDatasetOptions {
  readonly categories?: readonly LiveMcpBenchCategory[];
}

const AnnotatorMetadataSchema = z.object({
  Steps: z.string(),
  Tools: z.string(),
});

const RecordSchema = z.object({
  task_id: z.string(),
  Question: z.string(),
  answers: z.string(),
  category: z.enum(LIVE_MCP_BENCH_CATEGORIES),
  file_name: z.string(),
  "Annotator Metadata": AnnotatorMetadataSchema,
});

const NUMBERED_LINE = /^\s*\d+\.\s*/;

export function parseNumberedList(text: string): readonly string[] {
  return text
    .split("\n")
    .map((line) => line.replace(NUMBERED_LINE, "").trim())
    .filter((line) => line !== "");
}

export function recordToSample(
  record: Readonly<Record<string, unknown>>,
  index: number
): Sample {
  const parsed = parseSchema(RecordSchema, record);
  if (Result.isFailure(parsed)) {
    throw new Error(
      `LiveMCPBench record ${index} is malformed: ${parsed.failure.message}`
    );
  }
  const row = parsed.success;
  const task: LiveMcpBenchTask = {
    taskId: row.task_id,
    category: row.category,
    answers: row.answers,
    fileName: row.file_name,
    steps: row["Annotator Metadata"].Steps,
    annotatedTools: parseNumberedList(row["Annotator Metadata"].Tools),
  };
  return {
    id: `${LIVE_MCP_BENCH_ID}-${index}`,
    input: row.Question,
    target: { text: row.answers },
    metadata: { [LIVE_MCP_BENCH_TASK_METADATA_KEY]: task },
  };
}

export function sampleTask(sample: Sample): LiveMcpBenchTask | undefined {
  const parsed = parseSchema(
    LiveMcpBenchTaskSchema,
    sample.metadata?.[LIVE_MCP_BENCH_TASK_METADATA_KEY]
  );
  return Result.isSuccess(parsed) ? parsed.success : undefined;
}

function sliceStream(
  source: Stream<Sample, DatasetError>,
  opts: DatasetStreamOptions | undefined
): Stream<Sample, DatasetError> {
  const start = opts?.start ?? 0;
  const dropped = start > 0 ? source.pipe(streamDrop(start)) : source;
  return opts?.end === undefined
    ? dropped
    : dropped.pipe(streamTake(Math.max(0, opts.end - start)));
}

export function keepSamplesInCategories(
  layer: Layer<Dataset>,
  categories: readonly LiveMcpBenchCategory[]
): Layer<Dataset> {
  const allowed = new Set<string>(categories);
  const isAllowed = (sample: Sample): boolean => {
    const task = sampleTask(sample);
    return task !== undefined && allowed.has(task.category);
  };
  return layerEffect(Dataset)(
    Dataset.pipe(
      mapEffect((dataset) => {
        const filtered = dataset.stream().pipe(streamFilter(isAllowed));
        const stream: Dataset["Service"]["stream"] = (opts) =>
          sliceStream(filtered, opts);
        return { size: runCount(filtered), stream };
      })
    )
  ).pipe(layerProvide(layer));
}

export function makeLiveMcpBenchDatasetLayer(
  retryConfig?: RetryConfig,
  options: LiveMcpBenchDatasetOptions = {}
): Layer<Dataset> {
  const base = makeHfDatasetLayer({
    ...LIVE_MCP_BENCH_DATASET,
    recordToSample,
    ...definedValues({ retry: retryConfig }),
  });
  return options.categories === undefined
    ? base
    : keepSamplesInCategories(base, options.categories);
}
