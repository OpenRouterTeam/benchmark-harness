import type { ResponsesRequest } from "@openrouter/sdk/models";
import { currentTimeMillis } from "effect/Clock";
import { formatIso, unsafeNow } from "effect/DateTime";
import type { Effect } from "effect/Effect";
import {
  catchAll,
  fail,
  flatMap,
  forEach,
  gen,
  logError,
  logWarning,
  mapError,
  provideService,
  succeed,
} from "effect/Effect";

import type {
  ModelMessage,
  ModelError,
  ModelOutput,
  ModelUsage,
  TaskState,
} from "../../harness/core";
import { MessageRole, SolverError } from "../../harness/core";
import type { SolverService } from "../../harness/solver";
import { Either } from "../../internal/either";
import { isDefinedAndNotNull } from "../../internal/guards";
import { parseSchema, z } from "../../internal/zod";
import type { JudgeConfig } from "../../judge/judge";
import { judgeCall } from "../../judge/judge";
import type {
  ResponsesResult,
  ResponsesSendOptions,
  ResponsesService,
} from "../../providers/responses-client";
import {
  Responses,
  ResponsesResultSchema,
  toModelError,
  usageFromResponses,
} from "../../providers/responses-client";
import { withAuxiliaryUsage } from "../../runtime/generation-ids";
import { withCallCacheSalt } from "../../runtime/response-cache";
import type { StageKey, ArtifactStore } from "./artifact-store";
import { stageGet, stagePut, ArtifactStoreService } from "./artifact-store";
import { buildGenerationResult } from "./generation";
import { parseSingleVerdict } from "./parse-verdict";
import {
  AGENT_SYSTEM_PROMPT,
  FUSION_CLASSIFIER_DIRECTIVE,
  JUDGE_SYSTEM_PROMPT,
  buildJudgeUserInput,
} from "./prompts";
import {
  buildFusionBody,
  buildSoloBody,
  experimentTools,
} from "./request-body";
import type {
  Criterion,
  CriterionVerdict,
  DracoPanelConfig,
  GenerationResult,
  JudgeRun,
} from "./schemas";
import {
  CriterionSchema,
  CriterionVerdictSchema,
  JudgeRunStatus,
  VerdictValue,
} from "./schemas";
import { judgeKey, productionFusionGenKey, soloGenKey } from "./stage-key";

export interface DracoSolverOpts {
  readonly config: DracoPanelConfig;
}

export function dracoSolver(
  responses: ResponsesService,
  artifactStore: ArtifactStore,
  opts: DracoSolverOpts
): SolverService {
  const { config } = opts;
  return (state) =>
    gen(function* () {
      const sample = state.sample;
      const problem = sample.input;
      const criteriaResult = readCriteria(sample.metadata?.["criteria"]);
      if (Either.isLeft(criteriaResult)) {
        return yield* fail(new SolverError({ message: criteriaResult.left }));
      }
      const criteria = criteriaResult.right;
      const { generation, generationKey } = yield* runGeneration({
        config,
        taskId: sample.id,
        problem,
      });
      if (generation.status !== "ok") {
        yield* logWarning("draco generation failed/refused", {
          task_id: sample.id,
          experiment: config.name,
          status: generation.status,
          error: generation.error,
        });
        return completedState(state, generation, []);
      }
      const verdicts = yield* runJudge({
        config,
        generationKey,
        problem,
        responseText: generation.content ?? "",
        criteria,
      });
      return completedState(state, generation, verdicts);
    }).pipe(
      provideService(Responses, responses),
      provideService(ArtifactStoreService, artifactStore)
    );
}

function runGeneration(opts: {
  config: DracoPanelConfig;
  taskId: string;
  problem: string;
}): Effect<
  {
    readonly generation: GenerationResult;
    readonly generationKey: string;
  },
  ModelError | SolverError,
  ArtifactStoreService | Responses
> {
  const { config, taskId, problem } = opts;
  return gen(function* () {
    const isFusion = config.type === "fusion";
    const tools = experimentTools(config);
    let prompt: string;
    if (isFusion) {
      prompt = FUSION_CLASSIFIER_DIRECTIVE + AGENT_SYSTEM_PROMPT;
    } else if (tools.length > 0) {
      prompt = AGENT_SYSTEM_PROMPT;
    } else {
      prompt = "";
    }
    const body = isFusion
      ? buildFusionBody(problem, config)
      : buildSoloBody(problem, config);
    const stageKey = isFusion
      ? productionFusionGenKey({ taskId, config, prompt })
      : soloGenKey({ taskId, config, prompt, tools });
    const { result, latencyMs } = yield* cachedSend({ config, stageKey, body });
    const generation = buildGenerationResult({
      taskId,
      config,
      result,
      latencyMs,
    });
    return { generation, generationKey: stageKey.key };
  });
}

function cachedSend(opts: {
  config: DracoPanelConfig;
  stageKey: StageKey;
  body: ResponsesRequest;
}): Effect<
  {
    result: ResponsesResult;
    latencyMs: number;
  },
  ModelError | SolverError,
  ArtifactStoreService | Responses
> {
  const { config, stageKey, body } = opts;
  return gen(function* () {
    const cached = yield* stageGet(stageKey).pipe(
      mapError((e) => new SolverError({ message: e }))
    );
    if (cached !== undefined) {
      const jsonResult = Either.try(() => JSON.parse(cached));
      if (Either.isLeft(jsonResult)) {
        yield* logWarning(
          "draco corrupted generation cache entry (invalid JSON); treating as miss",
          {
            stage: stageKey.stage,
            key: stageKey.key,
          }
        );
      } else {
        const parsed = parseSchema(ResponsesResultSchema, jsonResult.right);
        if (Either.isLeft(parsed)) {
          yield* logWarning(
            "draco corrupted generation cache entry (schema mismatch); treating as miss",
            {
              stage: stageKey.stage,
              key: stageKey.key,
            }
          );
        } else {
          return { result: parsed.right, latencyMs: 0 };
        }
      }
    }
    const responses = yield* Responses;
    const startedAt = yield* currentTimeMillis;
    const result = yield* responses
      .send(body, sendOptions(config))
      .pipe(mapError(toModelError));
    const latencyMs = (yield* currentTimeMillis) - startedAt;
    if (result.status === "completed") {
      yield* stagePut(stageKey, JSON.stringify(result)).pipe(
        catchAll((e) => {
          return logWarning(
            "draco stagePut failed for generation; proceeding without caching",
            {
              stage: stageKey.stage,
              key: stageKey.key,
              error: String(e),
            }
          );
        })
      );
    }
    return { result, latencyMs };
  });
}

function dracoJudgeConfig(config: DracoPanelConfig): JudgeConfig {
  return {
    judgeModel: config.judgeModel,
    temperature: config.judgeTemperature ?? 0.2,
    timeoutMs: config.timeout * 1000,
    retry: { maxRetries: 0 },
    reasoningEffort: config.judgeReasoningEffort,
    ...(config.versionOverride !== undefined && {
      versionOverride: config.versionOverride,
    }),
  };
}

function sendOptions(config: DracoPanelConfig): ResponsesSendOptions {
  return {
    timeoutMs: config.timeout * 1000,
    ...(config.versionOverride !== undefined && {
      versionOverride: config.versionOverride,
    }),
  };
}

function runJudge(opts: {
  config: DracoPanelConfig;
  generationKey: string;
  problem: string;
  responseText: string;
  criteria: Criterion[];
}): Effect<
  JudgeRun[],
  ModelError | SolverError,
  ArtifactStoreService | Responses
> {
  const { config, generationKey, problem, responseText, criteria } = opts;
  const units: {
    runNum: number;
    criterion: Criterion;
  }[] = [];
  for (let runNum = 1; runNum <= config.judgeRuns; runNum++) {
    for (const criterion of criteria) {
      units.push({ runNum, criterion });
    }
  }
  return gen(function* () {
    const graded = yield* forEach(
      units,
      (unit) =>
        gradeOneCriterion({
          config,
          generationKey,
          problem,
          responseText,
          unit,
        }),
      { concurrency: Math.max(1, config.criterionConcurrency) }
    );
    const byRun = new Map<
      number,
      {
        verdicts: CriterionVerdict[];
        failed: number;
        cost: number;
      }
    >();
    for (let runNum = 1; runNum <= config.judgeRuns; runNum++) {
      byRun.set(runNum, { verdicts: [], failed: 0, cost: 0 });
    }
    for (let i = 0; i < units.length; i++) {
      const unit = units[i]!;
      const result = graded[i]!;
      const bucket = byRun.get(unit.runNum)!;
      if (result.verdict !== null) {
        bucket.verdicts.push(result.verdict);
      } else {
        bucket.failed += 1;
      }
      if (result.cost !== null) {
        bucket.cost += result.cost;
      }
    }
    const runs: JudgeRun[] = [];
    for (let runNum = 1; runNum <= config.judgeRuns; runNum++) {
      const bucket = byRun.get(runNum)!;
      const status =
        bucket.verdicts.length === 0
          ? JudgeRunStatus.Failed
          : JudgeRunStatus.Ok;
      if (status === JudgeRunStatus.Failed && bucket.failed > 0) {
        yield* logError("draco judge run produced zero verdicts", {
          run_num: runNum,
          failed_criteria: bucket.failed,
          experiment: config.name,
        });
      }
      runs.push({
        runNum,
        status,
        verdicts: bucket.verdicts,
        judgeModel: config.judgeModel,
        cost: bucket.cost > 0 ? bucket.cost : null,
        latencyMs: null,
        timestamp: formatIso(unsafeNow()),
      });
    }
    return runs;
  });
}

const DRACO_VERDICT_JSON_SCHEMA = {
  type: "object",
  additionalProperties: false,
  properties: {
    verdict: { type: "string", enum: [VerdictValue.Met, VerdictValue.Unmet] },
    justification: { type: "string" },
  },
  required: ["verdict", "justification"],
} as const;

interface GradeOneResult {
  readonly verdict: CriterionVerdict | null;
  readonly cost: number | null;
}

const GradeOneResultSchema = z.object({
  verdict: CriterionVerdictSchema.nullable(),
  cost: z.number().nullable(),
});

function gradeOneCriterion(opts: {
  config: DracoPanelConfig;
  generationKey: string;
  problem: string;
  responseText: string;
  unit: {
    readonly runNum: number;
    readonly criterion: Criterion;
  };
}): Effect<GradeOneResult, never, ArtifactStoreService | Responses> {
  const { config, generationKey, problem, responseText, unit } = opts;
  const { runNum, criterion } = unit;
  const grade = gen(function* () {
    const judgePrompt = buildJudgeUserInput(
      problem,
      responseText,
      criterion.requirement
    );
    const key = judgeKey({
      generationKey,
      judgeModel: config.judgeModel,
      criterionId: criterion.id,
      runNum,
      judgePrompt,
      judgeTemperature: config.judgeTemperature,
      judgeReasoningEffort: config.judgeReasoningEffort,
      versionOverride: config.versionOverride,
    });
    const cached = yield* stageGet(key);
    if (cached !== undefined) {
      const jsonResult = Either.try(() => JSON.parse(cached));
      if (Either.isLeft(jsonResult)) {
        yield* logWarning(
          "draco corrupted verdict cache entry (invalid JSON); treating as miss",
          {
            stage: key.stage,
            key: key.key,
          }
        );
      } else {
        const parsed = parseSchema(GradeOneResultSchema, jsonResult.right);
        if (Either.isLeft(parsed)) {
          yield* logWarning(
            "draco corrupted verdict cache entry (schema mismatch); treating as miss",
            {
              stage: key.stage,
              key: key.key,
            }
          );
        } else {
          return parsed.right;
        }
      }
    }
    const responses = yield* Responses;
    const judged = yield* withCallCacheSalt(
      `judge-run-${runNum}`,
      withAuxiliaryUsage(
        judgeCall(responses, dracoJudgeConfig(config), {
          instructions: JUDGE_SYSTEM_PROMPT,
          userInput: judgePrompt,
          schemaName: "draco_verdict",
          jsonSchema: DRACO_VERDICT_JSON_SCHEMA,
          parseVerdict: (text) =>
            Either.right(
              text.trim().length === 0 ? null : parseSingleVerdict(text)
            ),
        })
      )
    );
    const cost = judged.usage?.totalCost ?? null;
    let gradeResult: GradeOneResult;
    if (judged.verdict === null) {
      gradeResult = { verdict: null, cost };
    } else {
      const [verdict, justification] = judged.verdict;
      gradeResult = {
        verdict: {
          criterionId: criterion.id,
          section: criterion.section,
          weight: criterion.weight,
          verdict,
          justification,
        },
        cost,
      };
    }
    yield* stagePut(key, JSON.stringify(gradeResult)).pipe(
      catchAll((e) => {
        return logWarning(
          "draco stagePut failed for judge verdict; proceeding without caching",
          {
            criterion_id: criterion.id,
            error: String(e),
          }
        );
      })
    );
    return gradeResult;
  });
  return grade.pipe(
    catchAll((e) => {
      return logWarning(
        "draco grade_one_criterion failed; omitting criterion",
        {
          criterion_id: criterion.id,
          error: String(e),
        }
      ).pipe(
        flatMap(() => succeed<GradeOneResult>({ verdict: null, cost: null }))
      );
    })
  );
}

function completedState(
  state: TaskState,
  generation: GenerationResult,
  verdicts: JudgeRun[]
): TaskState {
  const messages: ModelMessage[] = [
    { role: MessageRole.User, content: state.sample.input },
    ...(generation.content
      ? [
          {
            role: MessageRole.Assistant,
            content: generation.content,
            ...(generation.citations.length > 0 && {
              citations: generation.citations,
            }),
          } as const,
        ]
      : []),
  ];
  const output: ModelOutput = {
    completion: generation.content ?? "",
    message: { role: MessageRole.Assistant, content: generation.content ?? "" },
    usage: dracoUsageToModelUsage(generation),
    generationTimeMs: generation.latencyMs ?? 0,
  };
  return {
    sample: {
      ...state.sample,
      metadata: {
        ...state.sample.metadata,
        generation,
        verdicts,
        domain: state.sample.metadata?.["domain"],
      },
    },
    messages,
    output,
    completed: true,
  };
}

function dracoUsageToModelUsage(
  generation: GenerationResult
): ModelUsage | undefined {
  const usage = generation.usage;
  if (!usage) {
    return undefined;
  }
  const mapped = usageFromResponses(usage) ?? {};
  const { totalCost: _perCallCost, ...tokens } = mapped;
  return {
    ...tokens,
    ...(isDefinedAndNotNull(generation.cost) && { totalCost: generation.cost }),
  };
}

function readCriteria(raw: unknown): Either.Either<Criterion[], string> {
  if (raw === undefined) {
    return Either.right([]);
  }
  const parsed = parseSchema(CriterionSchema.array(), raw);
  if (Either.isLeft(parsed)) {
    return Either.left(
      `Invalid criteria in sample metadata: ${parsed.left.message}`
    );
  }
  return Either.right(parsed.right);
}
