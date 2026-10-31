#!/usr/bin/env bun
import { join } from "node:path";

import { Presets, SingleBar } from "cli-progress";
import { gen, promise, runSync, sync } from "effect/Effect";
import { getOrNull } from "effect/Option";
import * as Result from "effect/Result";
import type { ZodError } from "zod";

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
  ADAPTIVE_REASONING_EFFORT,
  ADAPTIVE_REASONING_EFFORT_MODELS,
  COST_TIERS,
  defaultReasoningEffortFor,
  ImageDetail,
  IMAGE_DETAIL_VALUES,
  REASONING_EFFORTS,
  supportsAdaptiveReasoningEffort,
} from "../harness/constants";
import { makeProgressReporter } from "../harness/progress";
import { runHarnessPromise } from "../internal/effect-logger";
import { optionalEnv } from "../internal/env-config";
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

export type CliError =
  | { readonly kind: "invalid_image_detail"; readonly raw: string }
  | { readonly kind: "invalid_cost_tier"; readonly raw: string }
  | { readonly kind: "invalid_reasoning_effort"; readonly raw: string }
  | {
      readonly kind: "adaptive_reasoning_effort_unsupported";
      readonly model: string | undefined;
    }
  | { readonly kind: "unknown_benchmark"; readonly benchmarkId: string }
  | { readonly kind: "api_key_missing" }
  | { readonly kind: "unsafe_session_id"; readonly sessionId: string }
  | { readonly kind: "invalid_solver_config_json"; readonly cause: unknown }
  | { readonly kind: "model_required"; readonly benchmarkId: string }
  | {
      readonly kind: "unknown_solver_config_options";
      readonly benchmarkId: string;
      readonly options: readonly string[];
    }
  | {
      readonly kind: "invalid_benchmark_config";
      readonly benchmarkId: string;
      readonly error: ZodError;
    }
  | { readonly kind: "invalid_draco_panel_config"; readonly error: ZodError }
  | { readonly kind: "unsupported_benchmark"; readonly benchmarkId: string };

export function formatCliError(error: CliError): string {
  switch (error.kind) {
    case "invalid_image_detail": {
      return `--image-detail must be one of: ${IMAGE_DETAIL_VALUES.join(", ")} (got "${error.raw}")`;
    }
    case "invalid_cost_tier": {
      return `--cost-tier must be one of: ${COST_TIERS.join(", ")} (got "${error.raw}")`;
    }
    case "invalid_reasoning_effort": {
      return `--reasoning-effort must be one of: ${REASONING_EFFORTS.join(", ")} (got "${error.raw}")`;
    }
    case "adaptive_reasoning_effort_unsupported": {
      return `--reasoning-effort ${ADAPTIVE_REASONING_EFFORT} is only supported for: ${ADAPTIVE_REASONING_EFFORT_MODELS.join(", ")} (got model "${error.model ?? ""}")`;
    }
    case "unknown_benchmark": {
      return `Unknown benchmark "${error.benchmarkId}". Available: ${benchmarkIds().join(", ")}`;
    }
    case "api_key_missing": {
      return "Set OPENROUTER_API_KEY (or BENCHMARKING_OPENROUTER_API_KEY) in the environment.";
    }
    case "unsafe_session_id": {
      return `BENCH_CHILD_WORKFLOW_ID contains a control character, which ori replaces with a fresh UUID and silently detaches the run from its generations (got ${JSON.stringify(error.sessionId)}).`;
    }
    case "invalid_solver_config_json": {
      return `--solver-config is not valid JSON: ${String(error.cause)}`;
    }
    case "model_required": {
      return `${error.benchmarkId} requires --model`;
    }
    case "unknown_solver_config_options": {
      return `Unknown ${error.benchmarkId} solver-config option(s): ${error.options.join(", ")}`;
    }
    case "invalid_benchmark_config": {
      return `Invalid ${error.benchmarkId} config: ${error.error.message}`;
    }
    case "invalid_draco_panel_config": {
      return `Invalid DRACO panel config: ${error.error.message}`;
    }
    case "unsupported_benchmark": {
      return `Unsupported benchmark: ${error.benchmarkId}`;
    }
    default: {
      return error satisfies never;
    }
  }
}

export function parseArgs(
  argv: readonly string[]
): Result.Result<CliArgs, CliError> {
  const get = (flag: string): string | undefined => {
    const idx = argv.indexOf(flag);
    return idx !== -1 ? argv[idx + 1] : undefined;
  };
  const num = (flag: string): number | undefined => {
    const raw = get(flag);
    return raw !== undefined ? Number(raw) : undefined;
  };
  const imageDetail = validateImageDetail(get("--image-detail"));
  if (Result.isFailure(imageDetail)) {
    return Result.fail(imageDetail.failure);
  }
  const costTier = validateCostTier(get("--cost-tier"));
  if (Result.isFailure(costTier)) {
    return Result.fail(costTier.failure);
  }
  const reasoningEffort = validateReasoningEffort(
    get("--reasoning-effort"),
    get("--model")
  );
  if (Result.isFailure(reasoningEffort)) {
    return Result.fail(reasoningEffort.failure);
  }
  return Result.succeed({
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
    imageDetail: imageDetail.success,
    costTier: costTier.success,
    reasoningEffort: reasoningEffort.success,
  });
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
  benchmarkConfig: BenchmarkRunConfig,
  range:
    | {
        start?: number;
        end?: number;
      }
    | undefined,
  epochs: number
): Promise<number | undefined> {
  return datasetSizeById({ benchmarkId, benchmarkConfig }).then(
    (sizeResult) => {
      if (Result.isFailure(sizeResult)) {
        return undefined;
      }
      const size = sizeResult.success;
      const start = Math.min(range?.start ?? 0, size);
      const end = Math.min(range?.end ?? size, size);
      return Math.max(0, end - start) * epochs;
    }
  );
}

function resolveSessionId(): Result.Result<string, CliError> {
  const envOpt = runSync(optionalEnv("BENCH_CHILD_WORKFLOW_ID"));
  const raw = getOrNull(envOpt);
  const fromEnv = raw === null || raw.length === 0 ? null : raw;
  if (fromEnv !== null && !isSafeOriSessionId(fromEnv)) {
    return Result.fail({ kind: "unsafe_session_id", sessionId: fromEnv });
  }
  return Result.succeed(fromEnv ?? runSync(sync(() => crypto.randomUUID())));
}

function resolveApiKey(): Result.Result<string, CliError> {
  const primaryOpt = runSync(optionalEnv("OPENROUTER_API_KEY"));
  const fallbackOpt = runSync(optionalEnv("BENCHMARKING_OPENROUTER_API_KEY"));
  const keyValue = getOrNull(primaryOpt) ?? getOrNull(fallbackOpt);
  if (keyValue === null) {
    return Result.fail({ kind: "api_key_missing" });
  }
  return Result.succeed(keyValue);
}

interface RunSetup {
  readonly args: CliArgs;
  readonly benchmark: NonNullable<ReturnType<typeof getBenchmark>>;
  readonly apiKey: string;
  readonly sessionId: string;
  readonly benchmarkConfig: unknown;
}

function prepareRun(
  argv: readonly string[]
): Result.Result<RunSetup, CliError> {
  const args = parseArgs(argv);
  if (Result.isFailure(args)) {
    return Result.fail(args.failure);
  }
  const benchmark = getBenchmark(args.success.benchmark);
  if (benchmark === undefined) {
    return Result.fail({
      kind: "unknown_benchmark",
      benchmarkId: args.success.benchmark,
    });
  }
  const apiKey = resolveApiKey();
  if (Result.isFailure(apiKey)) {
    return Result.fail(apiKey.failure);
  }
  const sessionId = resolveSessionId();
  if (Result.isFailure(sessionId)) {
    return Result.fail(sessionId.failure);
  }
  const benchmarkConfig =
    args.success.solverConfig !== undefined
      ? parseSolverConfig(args.success.solverConfig)
      : Result.succeed(undefined);
  if (Result.isFailure(benchmarkConfig)) {
    return Result.fail(benchmarkConfig.failure);
  }
  return Result.succeed({
    args: args.success,
    benchmark,
    apiKey: apiKey.success,
    sessionId: sessionId.success,
    benchmarkConfig: benchmarkConfig.success,
  });
}

function failCli(error: CliError): void {
  process.stderr.write(`error: ${formatCliError(error)}\n`);
  process.exitCode = 1;
}

function main(): Promise<void> {
  const setup = prepareRun(process.argv.slice(2));
  if (Result.isFailure(setup)) {
    failCli(setup.failure);
    return Promise.resolve();
  }
  const { args, benchmark, apiKey, sessionId, benchmarkConfig } = setup.success;
  const baseUrl = getOrNull(runSync(optionalEnv("OPENROUTER_BASE_URL")));
  const epochs = args.epochs ?? benchmark.defaultEpochs;
  const range = resolveRange(args);
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
      const built = buildBenchmarkConfig({
        benchmarkId: args.benchmark,
        model: args.model,
        panelConfig: effectivePanelConfig,
        artifactDir,
        endpointId: args.endpointId,
        imageDetail: args.imageDetail,
        costTier: args.costTier,
        reasoningEffort: args.reasoningEffort,
      });
      if (Result.isFailure(built)) {
        failCli(built.failure);
        return;
      }
      const benchmarkRunConfig = built.success;
      process.stderr.write(
        `Running ${args.benchmark}${args.model !== undefined ? ` on ${args.model}` : ""}${args.solverConfig !== undefined ? ` (solver-config=${args.solverConfig})` : ""}${artifactDir !== undefined ? ` (artifact-dir=${artifactDir})` : ""} (epochs=${epochs}, concurrency=${args.concurrency}, reasoning-effort=${args.reasoningEffort}${range !== undefined ? `, range=${range.start ?? 0}..${range.end ?? "end"}` : ""}, session=${sessionId})...\n`
      );
      const total = yield* promise(() =>
        resolveTotalEvaluations(
          args.benchmark,
          benchmarkRunConfig,
          range,
          epochs
        )
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
      if (Result.isFailure(result)) {
        process.stderr.write(`Benchmark failed: ${result.failure}\n`);
        process.exitCode = 1;
        return;
      }
      const { metrics, usage } = result.success.result;
      const bench = getBenchmark(args.benchmark);
      const runLevelScores = bench?.runLevelScores?.(result.success.result);
      if (result.success.resultsPath !== null) {
        process.stderr.write(
          `Results written to ${result.success.resultsPath}\n`
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
            sampleScores: result.success.result.sampleScores.map((s) =>
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

function validateImageDetail(
  raw: string | undefined
): Result.Result<ImageDetail | undefined, CliError> {
  if (raw === undefined) {
    return Result.succeed(undefined);
  }
  if (!isMember(raw, ImageDetail)) {
    return Result.fail({ kind: "invalid_image_detail", raw });
  }
  return Result.succeed(raw);
}

function validateCostTier(
  raw: string | undefined
): Result.Result<CostTier | undefined, CliError> {
  if (raw === undefined) {
    return Result.succeed(undefined);
  }
  if (!isMember(raw, COST_TIERS)) {
    return Result.fail({ kind: "invalid_cost_tier", raw });
  }
  return Result.succeed(raw);
}

function validateReasoningEffort(
  raw: string | undefined,
  model: string | undefined
): Result.Result<ReasoningEffort, CliError> {
  if (raw === undefined) {
    return Result.succeed(defaultReasoningEffortFor(model));
  }
  if (!isMember(raw, REASONING_EFFORTS)) {
    return Result.fail({ kind: "invalid_reasoning_effort", raw });
  }
  if (
    raw === ADAPTIVE_REASONING_EFFORT &&
    (model === undefined || !supportsAdaptiveReasoningEffort(model))
  ) {
    return Result.fail({
      kind: "adaptive_reasoning_effort_unsupported",
      model,
    });
  }
  return Result.succeed(raw);
}

function requireModel(
  benchmarkId: string,
  model: string | undefined
): Result.Result<string, CliError> {
  if (model === undefined) {
    return Result.fail({ kind: "model_required", benchmarkId });
  }
  return Result.succeed(model);
}

function buildSchemaValidatedConfig(opts: {
  benchmarkId: string;
  model: string | undefined;
  endpointId: string | undefined;
  panelConfig: unknown;
  imageDetail?: ImageDetail;
  costTier?: CostTier;
  reasoningEffort: ReasoningEffort;
}): Result.Result<BenchmarkRunConfig, CliError> {
  const {
    benchmarkId,
    endpointId,
    panelConfig,
    imageDetail,
    costTier,
    reasoningEffort,
  } = opts;
  const model = requireModel(benchmarkId, opts.model);
  if (Result.isFailure(model)) {
    return Result.fail(model.failure);
  }
  const merged: Record<string, unknown> = definedValues({
    benchmarkId,
    model: model.success,
    endpointId,
    imageDetail,
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
      return Result.fail({
        kind: "unknown_solver_config_options",
        benchmarkId,
        options: unknown.sort(),
      });
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
    reasoningEffort !== ADAPTIVE_REASONING_EFFORT &&
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
  if (Result.isFailure(parsed)) {
    return Result.fail({
      kind: "invalid_benchmark_config",
      benchmarkId,
      error: parsed.failure,
    });
  }
  return Result.succeed(parsed.success);
}

function parseSolverConfig(raw: string): Result.Result<unknown, CliError> {
  const trimmed = raw.trim();
  if (!trimmed.startsWith("{")) {
    return Result.succeed(raw);
  }
  const result = Result.try(() => JSON.parse(trimmed));
  if (Result.isFailure(result)) {
    return Result.fail({
      kind: "invalid_solver_config_json",
      cause: result.failure,
    });
  }
  return Result.succeed(result.success);
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
}): Result.Result<BenchmarkRunConfig, CliError> {
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
    case "tau_bench_verified_airline": {
      return buildSchemaValidatedConfig({
        benchmarkId: "tau_bench_verified_airline",
        model,
        endpointId,
        panelConfig,
        costTier,
        reasoningEffort,
      });
    }
    case "tau3_bench_banking": {
      return buildSchemaValidatedConfig({
        benchmarkId: "tau3_bench_banking",
        model,
        endpointId,
        panelConfig,
        costTier,
        reasoningEffort,
      });
    }
    case "terminal_bench": {
      return buildSchemaValidatedConfig({
        benchmarkId: "terminal_bench",
        model,
        endpointId,
        panelConfig,
        costTier,
        reasoningEffort,
      });
    }
    case "draco": {
      const panel = parseSchema(DracoPanelConfigSchema, panelConfig);
      if (Result.isFailure(panel)) {
        return Result.fail({
          kind: "invalid_draco_panel_config",
          error: panel.failure,
        });
      }
      return Result.succeed({
        benchmarkId: "draco",
        panelConfig: panel.success,
        ...definedValues({
          artifactDir,
        }),
      });
    }
    case "mmmu_pro_vision": {
      return buildSchemaValidatedConfig({
        benchmarkId: "mmmu_pro_vision",
        model,
        endpointId,
        panelConfig,
        imageDetail: opts.imageDetail,
        costTier,
        reasoningEffort,
      });
    }
    case "gpqa_diamond":
    case "mmlu_pro":
    case "ifstruct":
    case "toolcall_formats":
    case "toolcall_schema_fuzz":
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
        model,
        endpointId,
        panelConfig,
        costTier,
        reasoningEffort,
      });
    }
    default: {
      return Result.fail({ kind: "unsupported_benchmark", benchmarkId });
    }
  }
}
if (import.meta.main) {
  await main();
}
