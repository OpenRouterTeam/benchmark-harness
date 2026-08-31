import type { HttpClient } from "@effect/platform";
import { gen } from "effect/Effect";
import type { Layer } from "effect/Layer";
import { fail, effect, provide, mergeAll, succeed } from "effect/Layer";

import type { Dataset } from "../../harness/dataset";
import type { RunResult } from "../../harness/run";
import { Scorer } from "../../harness/scorer";
import { Solver } from "../../harness/solver";
import { Either } from "../../internal/either";
import { isRecord } from "../../internal/guards";
import {
  Responses,
  makeResponsesLayer,
} from "../../providers/responses-client";
import { DRACO_META } from "../benchmark-meta";
import type { Benchmark, BenchmarkRunInput } from "../types";
import type { ArtifactStore } from "./artifact-store";
import {
  ArtifactStoreService,
  FsArtifactStore,
  GcsArtifactStore,
  NoopArtifactStore,
  makeArtifactStoreLayer,
} from "./artifact-store";
import { DRACO_CLI_PLUGIN } from "./cli";
import { makeDracoDatasetLayer } from "./dataset";
import { dracoScorer } from "./scorer-draco";
import { dracoSolver } from "./solver";

export const DRACO_BENCHMARK_ID = DRACO_META.id;

function makeDracoLayer(
  input: BenchmarkRunInput
): Layer<Dataset | Solver | Scorer, Error, HttpClient.HttpClient> {
  const { benchmarkConfig } = input;
  if (benchmarkConfig.benchmarkId !== "draco") {
    return fail(new Error("draco received mismatched benchmarkConfig"));
  }
  const config = benchmarkConfig.panelConfig;
  const datasetLayer = makeDracoDatasetLayer(input.datasetRetry);
  const responsesLayer = makeResponsesLayer({
    apiKey: input.apiKey,
    ...(input.baseUrl !== undefined && { baseUrl: input.baseUrl }),
    sessionId: input.sessionId,
    ...(input.traceHeaders !== undefined && {
      traceHeaders: input.traceHeaders,
    }),
  });
  const artifactLayer = resolveArtifactLayer(
    benchmarkConfig.artifactDir,
    config.cacheNamespace
  );
  const solverLayer = effect(Solver)(
    gen(function* () {
      const responses = yield* Responses;
      const artifactStore = yield* ArtifactStoreService;
      return Solver.of(dracoSolver(responses, artifactStore, { config }));
    })
  );
  const scorerLayer = succeed(Scorer, Scorer.of(dracoScorer(config)));
  const infraLayer = mergeAll(responsesLayer, artifactLayer);
  return mergeAll(
    datasetLayer,
    solverLayer.pipe(provide(infraLayer)),
    scorerLayer
  );
}

function defaultBucketEnv(): string | null {
  return process.env["BENCHMARK_RESULTS_BUCKET"] ?? null;
}

export function resolveArtifactStore(
  artifactDir: string | undefined,
  cacheNamespace: string | undefined,
  bucketEnv: string | null = defaultBucketEnv()
): ArtifactStore {
  if (artifactDir !== undefined) {
    return new FsArtifactStore(artifactDir);
  }
  const trimmed = cacheNamespace?.trim();
  const namespace = trimmed?.length ? trimmed : undefined;
  if (bucketEnv && namespace !== undefined) {
    return new GcsArtifactStore(bucketEnv, `draco/stage-cache/${namespace}`);
  }
  return new NoopArtifactStore();
}

function resolveArtifactLayer(
  artifactDir: string | undefined,
  cacheNamespace: string | undefined
): Layer<ArtifactStoreService> {
  return makeArtifactStoreLayer(
    resolveArtifactStore(artifactDir, cacheNamespace)
  );
}

export const DRACO_BENCHMARK: Benchmark = {
  id: DRACO_BENCHMARK_ID,
  makeDatasetLayer: makeDracoDatasetLayer,
  temperature: 0,
  defaultEpochs: DRACO_META.defaultEpochs,
  cli: DRACO_CLI_PLUGIN,
  makeLayer: makeDracoLayer,
  runLevelScores: dracoRunLevelScores,
};

function dracoRunLevelScores(result: RunResult): readonly {
  name: string;
  metrics: Readonly<
    Record<
      string,
      {
        value: number;
      }
    >
  >;
}[] {
  let normSum = 0;
  let passSum = 0;
  let counted = 0;
  for (const ss of result.sampleScores) {
    const expl = ss.score.explanation;
    if (expl === undefined) {
      continue;
    }
    const parsed = Either.try(() => JSON.parse(expl));
    if (Either.isLeft(parsed)) {
      continue;
    }
    const v = parsed.right;
    if (isRecord(v)) {
      const n = v["normalized"];
      const p = v["passRate"];
      if (typeof n === "number" && typeof p === "number") {
        normSum += n;
        passSum += p;
        counted += 1;
      }
    }
  }
  if (counted === 0) {
    return [];
  }
  return [
    {
      name: "draco",
      metrics: {
        normalized: { value: Number((normSum / counted).toFixed(2)) },
        pass_rate: { value: Number((passSum / counted).toFixed(2)) },
        samples_scored: { value: counted },
      },
    },
  ];
}
