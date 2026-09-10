#!/usr/bin/env bun
import { join } from "node:path";

import { Presets, SingleBar } from "cli-progress";
import { option, string } from "effect/Config";
import { gen, promise, runSync, sync } from "effect/Effect";
import { getOrNull } from "effect/Option";

import { isSafeOriSessionId } from "../benchmarks/agent-cli/runner";
import type { BenchmarkRunConfig } from "../benchmarks/benchmark-config";
import {
  BENCHMARK_OPTIONS_SCHEMAS,
  BenchmarkRunConfigSchema,
  isModelBenchmarkId,
  knownBenchmarkOptionKeys,
} from "../benchmarks/benchmark-config";
import { DracoPanelConfigSchema } from "../benchmarks/draco/schemas";
import { benchmarkIds, getBenchmark } from "../benchmarks/registry";
import type { CostTier, ReasoningEffort } from "../harness/constants";
import {
  COST_TIERS,
  DEFAULT_REASONING_EFFORT,
  ImageDetail,
  IMAGE_DETAIL_VALUES,
  REASONING_EFFORTS,
} from "../harness/constants";
import { makeProgressReporter } from "../harness/progress";
import { runHarnessPromise } from "../internal/effect-logger";
import { Either } from "../internal/either";
import { definedValues, isMember } from "../internal/guards";
import { parseSchema } from "../internal/zod";
import { makeLocalResultStore } from "../results/result-store";
import { datasetSizeById, runBenchmarkById } from "../runner/run-by-id";

interface CliArgs {
  readonly benchmark: string;
  readonly model: string | undefined;
  readonly limit?: number;
  readonly start?: number;
  readonly end?: number;
  readonly epochs?: number;
  readonly concurrency: number;
  readonly endpointId?: string;
  readonly solverConfig?: string;
  readonly artifactDir?: string;
  readonly resumeId?: string;
  readonly imageDetail?: ImageDetail;
  readonly costTier?: CostTier;
  readonly reasoningEffort: ReasoningEffort;
}

export function parseArgs(argv: readonly string[]): CliArgs {
  const get = (flag: string): string | undefined => {
    const idx = argv.indexOf(flag);
    return idx !== -1 ? argv[idx + 1] : undefined;
  };
  const num = (flag: string): number | undefined => {
    const raw = get(flag);
    return raw !== undefined ? Number(raw) : undefined;
  };
  return {
    benchmark: get("--benchmark") ?? "gpqa_diamond",
    model: get("--model"),
    limit: num("--limit"),
    start: num("--start"),
    end: num("--end"),
    epochs: num("--epochs"),
    concurrency: num("--concurrency") ?? 8,
    endpointId: get("--endpoint-id"),
    solverConfig: get("--solver-config"),
    artifactDir: get("--artifact-dir"),
    resumeId: get("--resume-id"),
    imageDetail: validateImageDetail(get("--image-detail")),
    costTier: validateCostTier(get("--cost-tier")),
    reasoningEffort: validateReasoningEffort(get("--reasoning-effort")),
  };
}

function resolveRange(args: CliArgs):
  | {
      start?: number;
      end?: number;
    }
  | undefined {
  const { start } = args;
  const end =
    args.end ??
    (args.limit !== undefined ? (start ?? 0) + args.limit : undefined);
  if (start === undefined && end === undefined) {
    return undefined;
  }
  return definedValues({
    start,
    end,
  });
}

function resolveTotalEvaluations(
  benchmarkId: string,
  range:
    | {
        start?: number;
        end?: number;
      }
    | undefined,
  epochs: number
): Promise<number | undefined> {
  return datasetSizeById(benchmarkId).then((sizeResult) => {
    if (Either.isLeft(sizeResult)) {
      return undefined;
    }
    const size = sizeResult.right;
    const start = Math.min(range?.start ?? 0, size);
    const end = Math.min(range?.end ?? size, size);
    return Math.max(0, end - start) * epochs;
  });
}

function resolveSessionId(): string {
  const envOpt = runSync(string("BENCH_CHILD_WORKFLOW_ID").pipe(option));
  const raw = getOrNull(envOpt);
  const fromEnv = raw === null || raw.length === 0 ? null : raw;
  if (fromEnv !== null && !isSafeOriSessionId(fromEnv)) {
    throw new Error(
      `BENCH_CHILD_WORKFLOW_ID contains a control character, which ori replaces with a fresh UUID and silently detaches the run from its generations (got ${JSON.stringify(fromEnv)}).`
    );
  }
  return fromEnv ?? runSync(sync(() => crypto.randomUUID()));
}

function resolveApiKey(): string {
  const primaryOpt = runSync(string("OPENROUTER_API_KEY").pipe(option));
  const fallbackOpt = runSync(
    string("BENCHMARKING_OPENROUTER_API_KEY").pipe(option)
  );
  const keyValue = getOrNull(primaryOpt) ?? getOrNull(fallbackOpt);
  if (keyValue === null) {
    throw new Error(
      "Set OPENROUTER_API_KEY (or BENCHMARKING_OPENROUTER_API_KEY) in the environment."
    );
  }
  return keyValue;
}

function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));
  const benchmark = getBenchmark(args.benchmark);
  if (benchmark === undefined) {
    throw new Error(
      `Unknown benchmark "${args.benchmark}". Available: ${benchmarkIds().join(", ")}`
    );
  }
  const apiKey = resolveApiKey();
  const baseUrl = getOrNull(
    runSync(string("OPENROUTER_BASE_URL").pipe(option))
  );
  const epochs = args.epochs ?? benchmark.defaultEpochs;
  const range = resolveRange(args);
  const sessionId = resolveSessionId();
  const benchmarkConfig =
    args.solverConfig !== undefined
      ? parseSolverConfig(args.solverConfig)
      : undefined;
  return runHarnessPromise(
    gen(function* () {
      let effectivePanelConfig: unknown = benchmarkConfig;
      let artifactDir: string | undefined = args.artifactDir ?? args.resumeId;
      if (benchmark.cli !== undefined) {
        const resolved = yield* promise(() =>
          benchmark.cli!.resolve(
            definedValues({
              argv: process.argv.slice(2),
              benchmarkConfig,
              artifactDir: args.artifactDir,
              resumeId: args.resumeId,
            })
          )
        );
        effectivePanelConfig = resolved.benchmarkConfig;
        ({ artifactDir } = resolved);
      }
      const benchmarkRunConfig = buildBenchmarkConfig({
        benchmarkId: args.benchmark,
        model: args.model,
        panelConfig: effectivePanelConfig,
        artifactDir,
        endpointId: args.endpointId,
        imageDetail: args.imageDetail,
        costTier: args.costTier,
        reasoningEffort: args.reasoningEffort,
      });
      process.stderr.write(
        `Running ${args.benchmark}${args.model !== undefined ? ` on ${args.model}` : ""}${args.solverConfig !== undefined ? ` (solver-config=${args.solverConfig})` : ""}${artifactDir !== undefined ? ` (artifact-dir=${artifactDir})` : ""} (epochs=${epochs}, concurrency=${args.concurrency}, reasoning-effort=${args.reasoningEffort}${range !== undefined ? `, range=${range.start ?? 0}..${range.end ?? "end"}` : ""}, session=${sessionId})...\n`
      );
      const total = yield* promise(() =>
        resolveTotalEvaluations(args.benchmark, range, epochs)
      );
      const bar = new SingleBar(
        {
          format:
            " {bar} {percentage}% | {value}/{total} evals | {sample} | {duration_formatted}",
        },
        Presets.shades_classic
      );
      let currentSample = "";
      if (total !== undefined) {
        bar.start(total, 0, { sample: "" });
      }
      const result = yield* promise(() =>
        runBenchmarkById(
          definedValues({
            benchmarkId: args.benchmark,
            apiKey,
            benchmarkConfig: benchmarkRunConfig,
            epochs,
            maxConcurrency: args.concurrency,
            baseUrl: baseUrl ? baseUrl : undefined,
            range,
            sessionId,
            resultStore: makeLocalResultStore({
              dir: join(process.cwd(), "bench-results"),
            }),
            progressReporter: makeProgressReporter({
              onSampleComplete: (completed) =>
                bar.update(completed, { sample: currentSample }),
              onSampleStart: (event) => {
                currentSample = `#${event.sampleIndex}`;
                bar.update({ sample: currentSample });
              },
              onSampleEnd: () => {
                currentSample = "";
                bar.update({ sample: currentSample });
              },
            }),
          })
        )
      );
      bar.stop();
      if (Either.isLeft(result)) {
        process.stderr.write(`Benchmark failed: ${result.left}\n`);
        process.exitCode = 1;
        return;
      }
      const { metrics, usage } = result.right.result;
      const bench = getBenchmark(args.benchmark);
      const runLevelScores = bench?.runLevelScores?.(result.right.result);
      if (result.right.resultsPath !== null) {
        process.stderr.write(
          `Results written to ${result.right.resultsPath}\n`
        );
      }
      process.stdout.write(
        `${JSON.stringify(
          definedValues({
            benchmark: args.benchmark,
            model: args.model,
            sessionId,
            solverConfig: args.solverConfig,
            accuracy: metrics.accuracy,
            totalQuestions: metrics.totalQuestions,
            correctAnswers: metrics.correctAnswers,
            usage,
            runLevelScores,
            sampleScores: result.right.result.sampleScores.map((s) =>
              definedValues({
                sampleId: s.sampleId,
                epoch: s.epoch,
                value: s.score.value,
                answer: s.score.answer,
                explanation: s.score.explanation,
                metadata: s.metadata,
              })
            ),
          }),
          null,
          2
        )}\n`
      );
    })
  );
}

function validateImageDetail(raw: string | undefined): ImageDetail | undefined {
  if (raw === undefined) {
    return undefined;
  }
  if (!isMember(raw, ImageDetail)) {
    throw new Error(
      `--image-detail must be one of: ${IMAGE_DETAIL_VALUES.join(", ")} (got "${raw}")`
    );
  }
  return raw;
}

function validateCostTier(raw: string | undefined): CostTier | undefined {
  if (raw === undefined) {
    return undefined;
  }
  if (!isMember(raw, COST_TIERS)) {
    throw new Error(
      `--cost-tier must be one of: ${COST_TIERS.join(", ")} (got "${raw}")`
    );
  }
  return raw;
}

function validateReasoningEffort(raw: string | undefined): ReasoningEffort {
  if (raw === undefined) {
    return DEFAULT_REASONING_EFFORT;
  }
  if (!isMember(raw, REASONING_EFFORTS)) {
    throw new Error(
      `--reasoning-effort must be one of: ${REASONING_EFFORTS.join(", ")} (got "${raw}")`
    );
  }
  return raw;
}

function requireModel(benchmarkId: string, model: string | undefined): string {
  if (model === undefined) {
    throw new Error(`${benchmarkId} requires --model`);
  }
  return model;
}

function buildSchemaValidatedConfig(opts: {
  benchmarkId: string;
  model: string;
  endpointId: string | undefined;
  panelConfig: unknown;
  costTier?: CostTier;
  reasoningEffort: ReasoningEffort;
}): BenchmarkRunConfig {
  const {
    benchmarkId,
    model,
    endpointId,
    panelConfig,
    costTier,
    reasoningEffort,
  } = opts;
  const merged: Record<string, unknown> = definedValues({
    benchmarkId,
    model,
    endpointId,
    costTier,
    reasoningEffort,
  });
  if (typeof panelConfig === "object" && panelConfig !== null) {
    const known = isModelBenchmarkId(benchmarkId)
      ? knownBenchmarkOptionKeys(benchmarkId)
      : undefined;
    const unknown = known
      ? Object.keys(panelConfig).filter((k) => !known.has(k))
      : [];
    if (unknown.length > 0) {
      throw new Error(
        `Unknown ${benchmarkId} solver-config option(s): ${unknown.sort().join(", ")}`
      );
    }
    for (const [k, v] of Object.entries(panelConfig)) {
      if (k !== "benchmarkId" && k !== "model") {
        merged[k] = v;
      }
    }
  }
  const optionsSchema = isModelBenchmarkId(benchmarkId)
    ? BENCHMARK_OPTIONS_SCHEMAS[benchmarkId]
    : undefined;
  if (
    optionsSchema !== undefined &&
    Object.hasOwn(optionsSchema.shape, "agentReasoningEffort") &&
    !(
      typeof panelConfig === "object" &&
      panelConfig !== null &&
      Object.hasOwn(panelConfig, "agentReasoningEffort")
    )
  ) {
    merged.agentReasoningEffort = reasoningEffort;
  }
  const parsed = parseSchema(BenchmarkRunConfigSchema, merged);
  if (Either.isLeft(parsed)) {
    throw new Error(`Invalid ${benchmarkId} config: ${parsed.left.message}`);
  }
  return parsed.right;
}

function parseSolverConfig(raw: string): unknown {
  const trimmed = raw.trim();
  if (trimmed.startsWith("{")) {
    const result = Either.try(() => JSON.parse(trimmed));
    if (Either.isLeft(result)) {
      throw new Error(
        `--solver-config is not valid JSON: ${String(result.left)}`
      );
    }
    return result.right;
  }
  return raw;
}

export function buildBenchmarkConfig(opts: {
  benchmarkId: string;
  model: string | undefined;
  panelConfig: unknown;
  artifactDir: string | undefined;
  endpointId: string | undefined;
  imageDetail: ImageDetail | undefined;
  costTier?: CostTier;
  reasoningEffort: ReasoningEffort;
}): BenchmarkRunConfig {
  const {
    benchmarkId,
    model,
    panelConfig,
    artifactDir,
    endpointId,
    costTier,
    reasoningEffort,
  } = opts;
  switch (benchmarkId) {
    case "gpqa_diamond": {
      return {
        benchmarkId: "gpqa_diamond",
        model: requireModel("gpqa_diamond", model),
        ...definedValues({
          endpointId,
          costTier,
        }),
        reasoningEffort,
      };
    }
    case "mmlu_pro": {
      return {
        benchmarkId: "mmlu_pro",
        model: requireModel("mmlu_pro", model),
        ...definedValues({
          endpointId,
          costTier,
        }),
        reasoningEffort,
      };
    }
    case "tau_bench_verified_airline": {
      return buildSchemaValidatedConfig({
        benchmarkId: "tau_bench_verified_airline",
        model: requireModel("tau_bench_verified_airline", model),
        endpointId,
        panelConfig,
        costTier,
        reasoningEffort,
      });
    }
    case "tau3_bench_banking": {
      return buildSchemaValidatedConfig({
        benchmarkId: "tau3_bench_banking",
        model: requireModel("tau3_bench_banking", model),
        endpointId,
        panelConfig,
        costTier,
        reasoningEffort,
      });
    }
    case "terminal_bench": {
      return buildSchemaValidatedConfig({
        benchmarkId: "terminal_bench",
        model: requireModel("terminal_bench", model),
        endpointId,
        panelConfig,
        costTier,
        reasoningEffort,
      });
    }
    case "terminal_bench_4": {
      return buildSchemaValidatedConfig({
        benchmarkId: "terminal_bench_4",
        model: requireModel("terminal_bench_4", model),
        endpointId,
        panelConfig,
        costTier,
        reasoningEffort,
      });
    }
    case "draco": {
      const panel = parseSchema(DracoPanelConfigSchema, panelConfig);
      if (Either.isLeft(panel)) {
        throw new Error(`Invalid DRACO panel config: ${panel.left.message}`);
      }
      return {
        benchmarkId: "draco",
        panelConfig: panel.right,
        ...definedValues({
          artifactDir,
        }),
      };
    }
    case "mmmu_pro_vision": {
      return {
        benchmarkId: "mmmu_pro_vision",
        model: requireModel("mmmu_pro_vision", model),
        ...definedValues({
          endpointId,
          imageDetail: opts.imageDetail,
          costTier,
        }),
        reasoningEffort,
      };
    }
    case "ifstruct": {
      return {
        benchmarkId: "ifstruct",
        model: requireModel("ifstruct", model),
        ...definedValues({
          endpointId,
          costTier,
        }),
        reasoningEffort,
      };
    }
    case "swe_atlas_qa":
    case "swe_atlas_tw":
    case "swe_atlas_rf":
    case "deep_swe":
    case "wandr":
    case "search_browsecomp":
    case "search_hle":
    case "search_dsqa":
    case "search_widesearch":
    case "vgi_bench": {
      return buildSchemaValidatedConfig({
        benchmarkId,
        model: requireModel(benchmarkId, model),
        endpointId,
        panelConfig,
        costTier,
        reasoningEffort,
      });
    }
    default: {
      throw new Error(`Unsupported benchmark: ${benchmarkId}`);
    }
  }
}
if (import.meta.main) {
  await main();
}
