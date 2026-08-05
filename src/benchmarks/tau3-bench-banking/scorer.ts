import type { Effect } from "effect/Effect";
import { succeed } from "effect/Effect";

import type { ChatMessage, Score, Target, TaskState } from "../../harness/core";
import { MessageRole, ScoreValue } from "../../harness/core";
import type { ScorerService } from "../../harness/scorer";
import { isDefinedAndNotNull, isRecord } from "../../internal/guards";
import type { PredictedToolCall } from "./evaluator";
import { evaluateSimulation } from "./evaluator";
import { parseToolArguments } from "./tools/helpers";
import type { BankingData, Tau3Task } from "./types";

function isBankingData(val: unknown): val is BankingData {
  if (typeof val !== "object" || val === null) {
    return false;
  }
  return "users" in val && "accounts" in val;
}

function isTau3Task(val: unknown): val is Tau3Task {
  if (typeof val !== "object" || val === null) {
    return false;
  }
  return "id" in val && "user_scenario" in val;
}

function collectToolCalls(
  messages: readonly ChatMessage[]
): PredictedToolCall[] {
  const calls: PredictedToolCall[] = [];
  for (const m of messages) {
    for (const tc of m.toolCalls ?? []) {
      const args = parseToolArguments(tc.function.arguments);
      calls.push({
        name: tc.function.name,
        arguments: args,
        requestor: "assistant",
      });
    }
  }
  return calls;
}

function collectUserToolCalls(value: unknown): PredictedToolCall[] {
  if (!Array.isArray(value)) {
    return [];
  }
  return value.filter(
    (call): call is PredictedToolCall =>
      isRecord(call) &&
      typeof call["name"] === "string" &&
      isRecord(call["arguments"]) &&
      call["requestor"] === "user"
  );
}

export const bankingScorer: ScorerService = (
  state: TaskState,
  _target: Target
): Effect<Score, never> => {
  const meta: Readonly<Record<string, unknown>> = state.sample.metadata ?? {};
  const agentData: unknown = meta["agentData"];
  const task: unknown = meta["task"];
  const terminationReason: string =
    typeof meta["terminationReason"] === "string"
      ? meta["terminationReason"]
      : "MAX_STEPS";
  if (!isBankingData(agentData)) {
    return succeed({
      value: ScoreValue.Incorrect,
      answer: null,
      explanation: "Missing agentData in TaskState metadata",
    });
  }
  if (isDefinedAndNotNull(task) && !isTau3Task(task)) {
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
  const toolCalls = [
    ...collectToolCalls(messages),
    ...collectUserToolCalls(meta["predictedUserToolCalls"]),
  ];
  const result = evaluateSimulation({
    task: isTau3Task(task) ? task : undefined,
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
      `db_match=${result.dbMatch}, action_met=${result.actionMet}, communicate_met=${result.communicateMet}, termination=${result.terminationReason}, breakdown=${JSON.stringify(result.breakdown)}`,
  });
};
