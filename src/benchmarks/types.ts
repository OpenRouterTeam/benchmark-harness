import type { HttpClient } from "@effect/platform";
import type { Layer } from "effect/Layer";

import type { Dataset } from "../harness/dataset";
import type { Model } from "../harness/model";
import type { RunResult } from "../harness/run";
import type { Scorer } from "../harness/scorer";
import type { Solver } from "../harness/solver";
import type { ResponsesModel } from "../providers/responses-model";
import type { RetryConfig } from "../runtime/retry";
import type { BenchmarkRunConfig } from "./benchmark-config";

export interface BenchmarkRunInput {
  readonly apiKey: string;
  readonly baseUrl?: string;
  readonly benchmarkConfig: BenchmarkRunConfig;
  readonly sessionId: string;
  readonly datasetRetry?: RetryConfig;
  readonly modelRetry?: RetryConfig;
  readonly maxOutputTokensCeiling?: number;
  readonly modelLayer?: Layer<Model, Error, HttpClient.HttpClient>;
  readonly responsesModelLayer?: Layer<
    ResponsesModel,
    Error,
    HttpClient.HttpClient
  >;
}

export interface BenchmarkPrimaryScore {
  readonly value: number;
  readonly weight: number;
}

export interface Benchmark {
  readonly id: string;
  readonly makeDatasetLayer: (retryConfig?: RetryConfig) => Layer<Dataset>;
  readonly makeDatasetLayerForConfig?: (
    config: BenchmarkRunConfig,
    retryConfig?: RetryConfig
  ) => Layer<Dataset>;
  readonly makeLayer: (
    input: BenchmarkRunInput
  ) => Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient>;
  readonly temperature: number;
  readonly defaultEpochs: number;
  readonly degradeSolverErrors?: boolean;
  readonly userModel?: string;
  readonly cli?: BenchmarkCliPlugin;
  readonly runLevelScores?: (result: RunResult) => readonly {
    readonly name: string;
    readonly metrics: Readonly<
      Record<
        string,
        {
          readonly value: number;
        }
      >
    >;
  }[];
  readonly primaryScore?: (
    result: RunResult
  ) => BenchmarkPrimaryScore | undefined;
}

export interface BenchmarkCliContext {
  readonly argv: readonly string[];
  readonly benchmarkConfig?: unknown;
  readonly artifactDir?: string;
  readonly resumeId?: string;
}

export interface BenchmarkCliResolution {
  readonly benchmarkConfig: unknown;
  readonly artifactDir?: string;
}

export interface BenchmarkCliPlugin {
  readonly resolve: (
    ctx: BenchmarkCliContext
  ) => Promise<BenchmarkCliResolution>;
}
