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
import type {
  TypesafeJudgeService,
  TypesafeQuestion,
} from "../../judge/typesafe";
import { noulOf, scoreOf } from "../../judge/typesafe";
import type { ResponsesService } from "../../providers/responses-client";
import { capToolOutput } from "../mcp-sandbox/tool-loop";
import { mergeModelUsages } from "../search/core/usage";
import type { LiveMcpBenchTask } from "./dataset";
import {
  LIVE_MCP_BENCH_TASK_METADATA_KEY,
  LiveMcpBenchTaskSchema,
} from "./dataset";
import {
  LIVE_MCP_BENCH_CATEGORIES,
  LIVE_MCP_BENCH_JUDGE_OUTPUT_CAP,
} from "./schema";
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
  judge: z.string().optional(),
  passProbability: z.number().optional(),
  quality: z.number().optional(),
  fabricationProbability: z.number().optional(),
  stepProbabilities: z.array(z.number()).readonly().optional(),
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

export interface ToolTranscriptEntry {
  readonly name: string;
  readonly arguments: string;
  readonly output: string;
}

export function toolTranscript(
  messages: readonly ModelMessage[],
  outputCap: number = LIVE_MCP_BENCH_JUDGE_OUTPUT_CAP
): ToolTranscriptEntry[] {
  const outputs = new Map<string, string>();
  for (const message of messages) {
    if (message.role === MessageRole.Tool && message.toolCallId !== undefined) {
      outputs.set(message.toolCallId, messageText(message.content));
    }
  }
  return messages
    .filter((message) => message.role === MessageRole.Assistant)
    .flatMap((message) => message.toolCalls ?? [])
    .map((call) => ({
      name: call.function.name,
      arguments: call.function.arguments,
      output: capToolOutput(outputs.get(call.id) ?? "", outputCap),
    }));
}

function messageText(content: ModelMessage["content"]): string {
  return typeof content === "string" ? content : JSON.stringify(content);
}

export function toolCallHistory(messages: readonly ModelMessage[]): string[] {
  return toolTranscript(messages).map(
    (entry) => `${entry.name}(${entry.arguments})\n   -> ${entry.output}`
  );
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

const JEV_PASS_THRESHOLD = 0.5;

export const JEV_QUALITY_CRITERIA = [
  "Wrong, empty, or refuses without cause",
  "Partially addresses the task; key parts missing or unsupported by tool outputs",
  "Addresses the task with minor gaps or unverified details",
  "Fully addresses the task, grounded in tool outputs, clear",
] as const;

export const JEV_RULES =
  "Information in the final answer must come from tool outputs, not the agent's own knowledge. Dates and identifiers later than the agent's training data are not evidence of fabrication; judge against `today`. A step counts as done if a tool call in `agent.tool_calls` plausibly performs it, even under a different tool name. Some tasks require a file to be created; the tool output must confirm the write.";

export function keyPointList(keyPoints: string): string[] {
  return keyPoints
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "");
}

export function jevState(input: {
  readonly today: string;
  readonly task: LiveMcpBenchTask | undefined;
  readonly question: string;
  readonly response: string;
  readonly transcript: readonly ToolTranscriptEntry[];
}): Record<string, unknown> {
  return {
    today: input.today,
    task: {
      category: input.task?.category ?? "unknown",
      question: input.question,
      expected_answer_hint: input.task?.answers ?? "",
      required_steps: input.task?.steps ?? "",
    },
    agent: {
      final_answer: input.response,
      tool_calls: input.transcript.map((entry, index) => ({
        index: index + 1,
        tool: entry.name,
        arguments: entry.arguments,
        output: entry.output,
      })),
    },
    rules: JEV_RULES,
  };
}

export function jevQuestions(
  keyPoints: string
): Record<string, TypesafeQuestion> {
  const questions: Record<string, TypesafeQuestion> = {
    pass: {
      type: "noul",
      instructions:
        "Did the agent complete the task in `task.question` according to `task.required_steps`, with a final answer that satisfies the request and is grounded in the tool outputs in `agent.tool_calls` (see `rules`)?",
    },
    quality: {
      type: "score",
      instructions:
        "How good is `agent.final_answer` as a response to `task.question`, given the evidence in `agent.tool_calls`?",
      criteria: [...JEV_QUALITY_CRITERIA],
    },
    fabricated: {
      type: "noul",
      instructions:
        "Does `agent.final_answer` present specific facts (numbers, names, URLs, IDs) that do not appear in any tool output in `agent.tool_calls`?",
    },
  };
  keyPointList(keyPoints).forEach((step, index) => {
    questions[`step_${index + 1}`] = {
      type: "noul",
      instructions: `Was this required step completed, judging from the tool calls, their outputs and the final answer: ${step}`,
    };
  });
  return questions;
}

export function jevVerdict(
  model: string,
  answers: Parameters<typeof noulOf>[0],
  stepCount: number
): LiveMcpBenchVerdict {
  const passProbability = noulOf(answers, "pass") ?? 0;
  const stepProbabilities = Array.from(
    { length: stepCount },
    (_, index) => noulOf(answers, `step_${index + 1}`) ?? 0
  );
  return {
    status:
      passProbability > JEV_PASS_THRESHOLD
        ? JudgeStatus.Success
        : JudgeStatus.Failure,
    thoughts: `jev pass=${passProbability.toFixed(2)} quality=${(scoreOf(answers, "quality") ?? 0).toFixed(2)} steps=[${stepProbabilities.map((p) => p.toFixed(2)).join(", ")}]`,
    judge: model,
    passProbability,
    ...definedValues({
      quality: scoreOf(answers, "quality"),
      fabricationProbability: noulOf(answers, "fabricated"),
    }),
    stepProbabilities,
  };
}

export function liveMcpBenchJevJudgeStage(
  judge: TypesafeJudgeService,
  today: () => string = () => new Date().toISOString().slice(0, 10)
): SolverService {
  return (state) =>
    gen(function* () {
      const response = state.output?.completion ?? "";
      if (response.trim() === "") {
        return withVerdict(state, EMPTY_RESPONSE_VERDICT, undefined);
      }
      const task = sampleTaskOf(state);
      const keyPoints = task?.steps ?? "";
      const answered = yield* judge.ask(
        jevState({
          today: today(),
          task,
          question:
            typeof state.sample.input === "string" ? state.sample.input : "",
          response,
          transcript: toolTranscript(state.messages),
        }),
        jevQuestions(keyPoints)
      );
      return withVerdict(
        state,
        jevVerdict(
          answered.model,
          answered.answers,
          keyPointList(keyPoints).length
        ),
        state.output?.usage
      );
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
