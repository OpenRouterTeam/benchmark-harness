import { sync } from "effect/Effect";

import type { Score, TaskState, ToolCall } from "../../harness/core";
import { ScoreValue } from "../../harness/core";
import type { ScorerService } from "../../harness/scorer";
import { Either } from "../../internal/either";
import { isRecord } from "../../internal/guards";
import type { ExpectedCall, ToolCallFormatCase } from "./cases";
import {
  anyOrderItems,
  getToolCallFormatCase,
  isAnyOrderValues,
  isOneOfValues,
  oneOfCandidates,
} from "./cases";
import { validateJsonSchema } from "./json-schema";

const ToolCallFailure = {
  NoToolCall: "no_tool_call",
  UnknownTool: "unknown_tool",
  InvalidJson: "invalid_json",
  SchemaViolation: "schema_violation",
  CallCount: "call_count",
  WrongValue: "wrong_value",
  UnknownCase: "unknown_case",
} as const;

const NUMBER_TOLERANCE = 1e-9;

interface ParsedCall {
  readonly name: string;
  readonly args: Readonly<Record<string, unknown>>;
}

interface CallFailure {
  readonly category: string;
  readonly detail: string;
}

function unorderedMatch(
  actual: readonly unknown[],
  expected: readonly unknown[]
): boolean {
  if (actual.length !== expected.length) {
    return false;
  }
  const unused = [...actual];
  return expected.every((item) => {
    const index = unused.findIndex((candidate) => valuesMatch(candidate, item));
    if (index === -1) {
      return false;
    }
    unused.splice(index, 1);
    return true;
  });
}

export function valuesMatch(actual: unknown, expected: unknown): boolean {
  if (isOneOfValues(expected)) {
    return oneOfCandidates(expected).some((candidate) =>
      valuesMatch(actual, candidate)
    );
  }
  if (isAnyOrderValues(expected)) {
    return (
      Array.isArray(actual) && unorderedMatch(actual, anyOrderItems(expected))
    );
  }
  if (expected === null) {
    return actual === null || actual === undefined;
  }
  if (typeof expected === "number") {
    return (
      typeof actual === "number" &&
      Math.abs(actual - expected) <= NUMBER_TOLERANCE
    );
  }
  if (typeof expected === "string") {
    return actual === expected;
  }
  if (Array.isArray(expected)) {
    if (!Array.isArray(actual) || actual.length !== expected.length) {
      return false;
    }
    return expected.every((item: unknown, index) =>
      valuesMatch(actual[index], item)
    );
  }
  if (isRecord(expected)) {
    return (
      isRecord(actual) &&
      Object.entries(expected).every(([key, value]) =>
        valuesMatch(actual[key], value)
      )
    );
  }
  return actual === expected;
}

function fail(failure: CallFailure, answer: string | null): Score {
  return {
    value: ScoreValue.Incorrect,
    answer,
    explanation: `${failure.category}: ${failure.detail}`,
  };
}

function parseCall(
  call: ToolCall,
  entry: ToolCallFormatCase
): Either.Either<ParsedCall, CallFailure> {
  const tool = entry.tools.find(
    (candidate) => candidate.function.name === call.function.name
  );
  if (tool === undefined) {
    return Either.left({
      category: ToolCallFailure.UnknownTool,
      detail: `called ${JSON.stringify(call.function.name)}`,
    });
  }
  const parsed = Either.try((): unknown => JSON.parse(call.function.arguments));
  if (Either.isLeft(parsed) || !isRecord(parsed.right)) {
    return Either.left({
      category: ToolCallFailure.InvalidJson,
      detail: `${call.function.name} arguments ${JSON.stringify(call.function.arguments)}`,
    });
  }
  const violations = validateJsonSchema(parsed.right, tool.function.parameters);
  if (violations.length > 0) {
    return Either.left({
      category: ToolCallFailure.SchemaViolation,
      detail: `${call.function.name} ${violations
        .map((violation) => `${violation.path} ${violation.message}`)
        .join("; ")}`,
    });
  }
  return Either.right({ name: call.function.name, args: parsed.right });
}

function findUnmatchedExpectation(
  calls: readonly ParsedCall[],
  expected: readonly ExpectedCall[]
): ExpectedCall | undefined {
  const unused = [...calls];
  return expected.find((expectation) => {
    const index = unused.findIndex(
      (call) =>
        call.name === expectation.tool &&
        valuesMatch(call.args, expectation.args)
    );
    if (index === -1) {
      return true;
    }
    unused.splice(index, 1);
    return false;
  });
}

function mismatchFailure(missing: ExpectedCall): CallFailure {
  return {
    category: ToolCallFailure.WrongValue,
    detail: `no ${missing.tool} call matched ${JSON.stringify(missing.args)}`,
  };
}

export function scoreToolCalls(
  entry: ToolCallFormatCase,
  toolCalls: readonly ToolCall[]
): Score {
  if (toolCalls.length === 0) {
    return fail(
      {
        category: ToolCallFailure.NoToolCall,
        detail: "response had no tool calls",
      },
      null
    );
  }
  const answer = JSON.stringify(
    toolCalls.map((call) => ({
      name: call.function.name,
      arguments: call.function.arguments,
    }))
  );
  const results = toolCalls.map((call) => parseCall(call, entry));
  const firstFailure = results.find(Either.isLeft);
  if (firstFailure !== undefined) {
    return fail(firstFailure.left, answer);
  }
  const parsed = results.filter(Either.isRight).map((result) => result.right);
  if (parsed.length !== entry.calls.length) {
    return fail(
      {
        category: ToolCallFailure.CallCount,
        detail: `expected ${entry.calls.length} calls, got ${parsed.length}`,
      },
      answer
    );
  }
  const missing = findUnmatchedExpectation(parsed, entry.calls);
  if (missing !== undefined) {
    return fail(mismatchFailure(missing), answer);
  }
  return {
    value: ScoreValue.Correct,
    answer,
    explanation: `${parsed.length} tool call(s) valid and matched`,
  };
}

export const toolCallFormatsScorer: ScorerService = (state: TaskState) =>
  sync(() => {
    const entry = getToolCallFormatCase(state.sample.id);
    if (entry === undefined) {
      return fail(
        {
          category: ToolCallFailure.UnknownCase,
          detail: `no toolcall_formats case for sample ${state.sample.id}`,
        },
        null
      );
    }
    return scoreToolCalls(entry, state.output?.message.toolCalls ?? []);
  });
