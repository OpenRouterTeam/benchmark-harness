import { sync } from "effect/Effect";

import type { Score, TaskState, ToolCall } from "../../harness/core";
import { ScoreValue } from "../../harness/core";
import type { ScorerService } from "../../harness/scorer";
import { Either } from "../../internal/either";
import { isRecord } from "../../internal/guards";
import type { ExpectedCall, ToolCallFormatCase } from "./cases";
import { getToolCallFormatCase } from "./cases";
import type { JsonSchema } from "./json-schema";
import { validateJsonSchema } from "./json-schema";

export const ToolCallFailure = {
  NoToolCall: "no_tool_call",
  UnknownTool: "unknown_tool",
  InvalidJson: "invalid_json",
  SchemaViolation: "schema_violation",
  CallCount: "call_count",
  NullString: "null_string",
  WrongValue: "wrong_value",
} as const;

const NUMBER_TOLERANCE = 1e-9;

export function valuesMatch(actual: unknown, expected: unknown): boolean {
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
    return typeof actual === "string" && actual.trim() === expected.trim();
  }
  if (Array.isArray(expected)) {
    if (!Array.isArray(actual) || actual.length !== expected.length) {
      return false;
    }
    const unused = [...actual];
    return expected.every((item: unknown) => {
      const index = unused.findIndex((candidate) =>
        valuesMatch(candidate, item)
      );
      if (index === -1) {
        return false;
      }
      unused.splice(index, 1);
      return true;
    });
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

interface ParsedCall {
  readonly name: string;
  readonly args: Readonly<Record<string, unknown>>;
}

function fail(category: string, detail: string, answer: string | null): Score {
  return {
    value: ScoreValue.Incorrect,
    answer,
    explanation: `${category}: ${detail}`,
  };
}

function parseCall(
  call: ToolCall,
  entry: ToolCallFormatCase
): Either.Either<ParsedCall, { category: string; detail: string }> {
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
  const parameters: JsonSchema = tool.function.parameters ?? {};
  const violations = validateJsonSchema(parsed.right, parameters);
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

function nullStringPath(
  actual: unknown,
  expected: unknown,
  path: string
): string | undefined {
  if (expected === null) {
    return actual === "null" ? path : undefined;
  }
  if (isRecord(expected) && isRecord(actual)) {
    for (const [key, value] of Object.entries(expected)) {
      const found = nullStringPath(actual[key], value, `${path}.${key}`);
      if (found !== undefined) {
        return found;
      }
    }
  }
  return undefined;
}

function unmatchedExpectation(
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

export function scoreToolCalls(
  entry: ToolCallFormatCase,
  toolCalls: readonly ToolCall[]
): Score {
  const answer =
    toolCalls.length > 0
      ? JSON.stringify(
          toolCalls.map((call) => ({
            name: call.function.name,
            arguments: call.function.arguments,
          }))
        )
      : null;
  if (toolCalls.length === 0) {
    return fail(ToolCallFailure.NoToolCall, "response had no tool calls", null);
  }
  const parsed: ParsedCall[] = [];
  for (const call of toolCalls) {
    const result = parseCall(call, entry);
    if (Either.isLeft(result)) {
      return fail(result.left.category, result.left.detail, answer);
    }
    parsed.push(result.right);
  }
  if (parsed.length !== entry.calls.length) {
    return fail(
      ToolCallFailure.CallCount,
      `expected ${entry.calls.length} calls, got ${parsed.length}`,
      answer
    );
  }
  const missing = unmatchedExpectation(parsed, entry.calls);
  if (missing !== undefined) {
    const nullString = parsed
      .filter((call) => call.name === missing.tool)
      .map((call) => nullStringPath(call.args, missing.args, "$"))
      .find((found) => found !== undefined);
    if (nullString !== undefined) {
      return fail(
        ToolCallFailure.NullString,
        `${missing.tool} ${nullString} is the string "null" instead of null`,
        answer
      );
    }
    return fail(
      ToolCallFailure.WrongValue,
      `no ${missing.tool} call matched ${JSON.stringify(missing.args)}`,
      answer
    );
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
        "unknown_case",
        `no toolcall_formats case for sample ${state.sample.id}`,
        null
      );
    }
    return scoreToolCalls(entry, state.output?.message.toolCalls ?? []);
  });
