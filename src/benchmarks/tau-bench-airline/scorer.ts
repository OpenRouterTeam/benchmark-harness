import type { Effect } from "effect/Effect";
import { succeed } from "effect/Effect";

import type {
  ModelMessage,
  Score,
  Target,
  TaskState,
} from "../../harness/core";
import { MessageRole, ScoreValue } from "../../harness/core";
import type { ScorerService } from "../../harness/scorer";
import { Either } from "../../internal/either";
import { isDefinedAndNotNull, isRecord } from "../../internal/guards";
import type { PredictedToolCall } from "./evaluator";
import { evaluateSimulation } from "./evaluator";
import type { AirlineData, Tau2Task } from "./types";

function isAirlineData(val: unknown): val is AirlineData {
  if (typeof val !== "object" || val === null) {
    return false;
  }
  return "flights" in val && "reservations" in val && "users" in val;
}

function isTau2Task(val: unknown): val is Tau2Task {
  if (typeof val !== "object" || val === null) {
    return false;
  }
  return "id" in val && "user_scenario" in val;
}

function collectToolCalls(
  messages: readonly ModelMessage[]
): PredictedToolCall[] {
  const calls: PredictedToolCall[] = [];
  for (const m of messages) {
    for (const tc of m.toolCalls ?? []) {
      const parsed = Either.try((): unknown =>
        JSON.parse(tc.function.arguments)
      );
      const args =
        Either.isRight(parsed) && isRecord(parsed.right) ? parsed.right : {};
      calls.push({ name: tc.function.name, arguments: args });
    }
  }
  return calls;
}

export const airlineScorer: ScorerService = (
  state: TaskState,
  _target: Target
): Effect<Score, never> => {
  const meta: Readonly<Record<string, unknown>> = state.sample.metadata ?? {};
  const agentData = meta["agentData"];
  const task = meta["task"];
  const terminationReason =
    typeof meta["terminationReason"] === "string"
      ? meta["terminationReason"]
      : "MAX_STEPS";
  if (!isAirlineData(agentData)) {
    return succeed({
      value: ScoreValue.Incorrect,
      answer: null,
      explanation: "Missing agentData in TaskState metadata",
    });
  }
  if (isDefinedAndNotNull(task) && !isTau2Task(task)) {
    return succeed({
      value: ScoreValue.Incorrect,
      answer: null,
      explanation: "Malformed task in TaskState metadata",
    });
  }
  const messages = state.messages;
  const assistantTexts = messages
    .filter((m) => m.role === MessageRole.Assistant)
    .map((m) => m.content);
  const toolCalls = collectToolCalls(messages);
  const result = evaluateSimulation({
    task: isTau2Task(task) ? task : undefined,
    agentData,
    assistantTexts,
    toolCalls,
    terminationReason,
  });
  return succeed({
    value: result.reward >= 1 ? ScoreValue.Correct : ScoreValue.Incorrect,
    answer: String(result.reward),
    explanation:
      result.note ??
      `db_match=${result.dbMatch}, communicate_met=${result.communicateMet}, termination=${result.terminationReason}`,
  });
};
