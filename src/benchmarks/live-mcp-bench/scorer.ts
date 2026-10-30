import type { Effect } from "effect/Effect";
import { gen, succeed } from "effect/Effect";
import * as Result from "effect/Result";

import type {
  ModelMessage,
  Score,
  TaskState,
  Target,
} from "../../harness/core";
import { MessageRole, ScoreValue } from "../../harness/core";
import type { RunResult } from "../../harness/run";
import type { ScorerService } from "../../harness/scorer";
import type { SolverService } from "../../harness/solver";
import type { ValueOf } from "../../internal/guards";
import { definedValues } from "../../internal/guards";
import { parseSchema, z } from "../../internal/zod";
import type { JudgeCallSpec, JudgeConfig } from "../../judge/judge";
import { judgeCall } from "../../judge/judge";
import type { ResponsesService } from "../../providers/responses-client";
import { mergeModelUsages } from "../search/core/usage";
import type { LiveMcpBenchTask } from "./dataset";
import {
  LIVE_MCP_BENCH_TASK_METADATA_KEY,
  LiveMcpBenchTaskSchema,
} from "./dataset";
import { LIVE_MCP_BENCH_CATEGORIES } from "./schema";
import {
  LIVE_MCP_BENCH_RUN_METADATA_KEY,
  LiveMcpBenchRunMetadataSchema,
} from "./solver";

export const LIVE_MCP_BENCH_VERDICT_METADATA_KEY = "live_mcp_bench_verdict";

export const JudgeStatus = {
  Success: "success",
  Failure: "failure",
} as const;

export type JudgeStatus = ValueOf<typeof JudgeStatus>;

export const LiveMcpBenchVerdictSchema = z.object({
  status: z.enum([JudgeStatus.Success, JudgeStatus.Failure]),
  thoughts: z.string(),
});

export type LiveMcpBenchVerdict = z.infer<typeof LiveMcpBenchVerdictSchema>;

const VERDICT_JSON_SCHEMA = {
  type: "object",
  properties: {
    thoughts: { type: "string" },
    status: {
      type: "string",
      enum: [JudgeStatus.Success, JudgeStatus.Failure],
    },
  },
  required: ["thoughts", "status"],
  additionalProperties: false,
} as const;

const PARSE_FAILURE_VERDICT: LiveMcpBenchVerdict = {
  status: JudgeStatus.Failure,
  thoughts: "judge verdict could not be parsed",
};

const EMPTY_RESPONSE_VERDICT: LiveMcpBenchVerdict = {
  status: JudgeStatus.Failure,
  thoughts: "the model produced no final response",
};

export const LIVE_MCP_BENCH_JUDGE_INSTRUCTIONS = `You are an expert in evaluating the performance of a tool-use agent. The agent is designed to help a human user use multi-tools to complete a task. Given the user's task, the agent's final response, key points for task completion, and tool call history, your goal is to determine whether the agent has completed the task and achieved all requirements.

Your response must strictly follow the following evaluation criteria!
*Important Evaluation Criteria*:
1. You must carefully check whether the information (e.g. the coordinates of the addresses) comes from the tool call, if the agent get it from the internal knowledge, it should be considered failed.
2: Some tasks require to create files to be considered successful.

*IMPORTANT*
Respond with a JSON object with two fields:
"thoughts": your thoughts and reasoning process based on double-checking each key points and the evaluation criteria
"status": "success" or "failure"`;

const STATUS_LINE = /status"?\s*[:=]\s*"?(success|failure)/i;

export function toolCallHistory(messages: readonly ModelMessage[]): string[] {
  return messages
    .filter((message) => message.role === MessageRole.Assistant)
    .flatMap((message) => message.toolCalls ?? [])
    .map((call) => `${call.function.name}(${call.function.arguments})`);
}

export function formatToolDescriptions(
  descriptions: Readonly<Record<string, string>>
): string {
  return Object.entries(descriptions)
    .map(([name, description]) => `Tool: ${name}\nDescription: ${description}`)
    .join("\n\n");
}

export function judgePrompt(input: {
  readonly task: string;
  readonly keyPoints: string;
  readonly response: string;
  readonly toolCalls: readonly string[];
  readonly toolDescriptions: string;
}): string {
  const calls = input.toolCalls
    .map((call, index) => `${index + 1}. ${call}`)
    .join("\n");
  return `User Task: \n${input.task}\n\nKey Points: \n${input.keyPoints}\n\nFinal Response: \n${input.response}\n\nTool Call History:\n${calls}\n\nTool Descriptions:\n${input.toolDescriptions}\n`;
}

export function parseVerdict(
  text: string
): Result.Result<LiveMcpBenchVerdict, string> {
  const trimmed = text.trim();
  try {
    const parsed = parseSchema(LiveMcpBenchVerdictSchema, JSON.parse(trimmed));
    if (Result.isSuccess(parsed)) {
      return Result.succeed(parsed.success);
    }
  } catch {
    return parseStatusLine(trimmed);
  }
  return parseStatusLine(trimmed);
}

function parseStatusLine(
  trimmed: string
): Result.Result<LiveMcpBenchVerdict, string> {
  const match = STATUS_LINE.exec(trimmed);
  if (match?.[1] === undefined) {
    return Result.fail("no success/failure status in judge output");
  }
  return Result.succeed({
    status:
      match[1].toLowerCase() === JudgeStatus.Success
        ? JudgeStatus.Success
        : JudgeStatus.Failure,
    thoughts: trimmed,
  });
}

export function judgeSpec(
  userInput: string
): JudgeCallSpec<LiveMcpBenchVerdict> {
  return {
    instructions: LIVE_MCP_BENCH_JUDGE_INSTRUCTIONS,
    userInput,
    schemaName: "live_mcp_bench_verdict",
    jsonSchema: VERDICT_JSON_SCHEMA,
    parseVerdict,
    parseFailureFallback: PARSE_FAILURE_VERDICT,
  };
}

function sampleTaskOf(state: TaskState): LiveMcpBenchTask | undefined {
  const parsed = parseSchema(
    LiveMcpBenchTaskSchema,
    state.sample.metadata?.[LIVE_MCP_BENCH_TASK_METADATA_KEY]
  );
  return Result.isSuccess(parsed) ? parsed.success : undefined;
}

function calledToolDescriptionsOf(
  state: TaskState
): Readonly<Record<string, string>> {
  const parsed = parseSchema(
    LiveMcpBenchRunMetadataSchema,
    state.sample.metadata?.[LIVE_MCP_BENCH_RUN_METADATA_KEY]
  );
  return Result.isSuccess(parsed) ? parsed.success.calledToolDescriptions : {};
}

export function liveMcpBenchJudgeStage(
  responses: ResponsesService,
  judgeConfig: JudgeConfig
): SolverService {
  return (state) =>
    gen(function* () {
      const response = state.output?.completion ?? "";
      if (response.trim() === "") {
        return withVerdict(state, EMPTY_RESPONSE_VERDICT, undefined);
      }
      const task = sampleTaskOf(state);
      const prompt = judgePrompt({
        task: typeof state.sample.input === "string" ? state.sample.input : "",
        keyPoints: task?.steps ?? "",
        response,
        toolCalls: toolCallHistory(state.messages),
        toolDescriptions: formatToolDescriptions(
          calledToolDescriptionsOf(state)
        ),
      });
      const judged = yield* judgeCall(
        responses,
        judgeConfig,
        judgeSpec(prompt)
      );
      const usage = mergeModelUsages([state.output?.usage, judged.usage]);
      return withVerdict(state, judged.verdict, usage);
    });
}

function withVerdict(
  state: TaskState,
  verdict: LiveMcpBenchVerdict,
  usage: ReturnType<typeof mergeModelUsages>
): TaskState {
  return {
    ...state,
    ...definedValues({
      output:
        state.output !== undefined
          ? { ...state.output, ...definedValues({ usage }) }
          : undefined,
    }),
    sample: {
      ...state.sample,
      metadata: {
        ...state.sample.metadata,
        [LIVE_MCP_BENCH_VERDICT_METADATA_KEY]: verdict,
      },
    },
  };
}

export function sampleVerdict(
  state: TaskState
): LiveMcpBenchVerdict | undefined {
  const parsed = parseSchema(
    LiveMcpBenchVerdictSchema,
    state.sample.metadata?.[LIVE_MCP_BENCH_VERDICT_METADATA_KEY]
  );
  return Result.isSuccess(parsed) ? parsed.success : undefined;
}

export function makeLiveMcpBenchScorer(): ScorerService {
  return (state: TaskState, _target: Target): Effect<Score, never> => {
    const verdict = sampleVerdict(state);
    const answer = state.output?.completion ?? null;
    if (verdict === undefined) {
      return succeed({
        value: ScoreValue.Incorrect,
        answer,
        explanation: "no judge verdict recorded",
      });
    }
    const task = sampleTaskOf(state);
    return succeed({
      value:
        verdict.status === JudgeStatus.Success
          ? ScoreValue.Correct
          : ScoreValue.Incorrect,
      answer,
      explanation: `judge: ${verdict.status}; ${verdict.thoughts}`,
      trajectory: {
        kind: "judge_runs",
        runs: [{ ...verdict, category: task?.category ?? "unknown" }],
      },
    });
  };
}

const CategoryRunSchema = z.object({
  status: z.enum([JudgeStatus.Success, JudgeStatus.Failure]),
  category: z.string(),
});

export function liveMcpBenchRunLevelScores(
  result: Pick<RunResult, "sampleScores">
): readonly {
  readonly name: string;
  readonly metrics: Readonly<Record<string, { readonly value: number }>>;
}[] {
  const runs = result.sampleScores.flatMap((sample) => {
    if (sample.score.value === ScoreValue.Skipped) {
      return [];
    }
    const trajectory = sample.score.trajectory;
    if (trajectory?.kind !== "judge_runs") {
      return [{ status: JudgeStatus.Failure, category: "unknown" }];
    }
    const parsed = parseSchema(CategoryRunSchema, trajectory.runs[0]);
    return [
      Result.isSuccess(parsed)
        ? parsed.success
        : { status: JudgeStatus.Failure, category: "unknown" },
    ];
  });
  if (runs.length === 0) {
    return [];
  }
  const successRate = (items: readonly { readonly status: JudgeStatus }[]) =>
    items.filter((item) => item.status === JudgeStatus.Success).length /
    items.length;
  const perCategory = Object.fromEntries(
    LIVE_MCP_BENCH_CATEGORIES.flatMap((category) => {
      const items = runs.filter((run) => run.category === category);
      return items.length === 0
        ? []
        : [
            [
              `success_rate_${category.toLowerCase()}`,
              { value: successRate(items) },
            ],
          ];
    })
  );
  return [
    {
      name: "live_mcp_bench_success",
      metrics: { success_rate: { value: successRate(runs) }, ...perCategory },
    },
  ];
}
