import { sync } from "effect/Effect";

import type { Score, TaskState, ToolCall } from "../../harness/core";
import { ScoreValue } from "../../harness/core";
import type { ScorerService } from "../../harness/scorer";
import { Either } from "../../internal/either";
import { isRecord } from "../../internal/guards";
import type { CaseLookup } from "./cases";
import { validateJsonSchema } from "./json-schema";
import type { ExpectedCall, ToolCallSchemaFuzzCase } from "./tool-case";

const FuzzFailure = {
  NoToolCall: "no_tool_call",
  CallCount: "call_count",
  UnknownTool: "unknown_tool",
  WrongTool: "wrong_tool",
  InvalidJson: "invalid_json",
  SchemaViolation: "schema_violation",
  ValueMismatch: "value_mismatch",
  UnknownCase: "unknown_case",
  Truncated: "truncated",
} as const;

interface Failure {
  readonly category: string;
  readonly detail: string;
}

function describe(value: unknown): string {
  return value === undefined ? "missing" : JSON.stringify(value);
}

interface Comparison {
  readonly actual: unknown;
  readonly expected: unknown;
}

function firstDifference(
  actual: unknown,
  expected: unknown
): string | undefined {
  return differenceAt({ actual, expected }, "$");
}

function arrayDifference(
  { actual, expected }: Comparison,
  path: string
): string | undefined {
  if (
    !Array.isArray(actual) ||
    !Array.isArray(expected) ||
    actual.length !== expected.length
  ) {
    return `${path} expected ${describe(expected)}, got ${describe(actual)}`;
  }
  return expected
    .map((item: unknown, index) =>
      differenceAt(
        { actual: actual[index], expected: item },
        `${path}[${index}]`
      )
    )
    .find((difference) => difference !== undefined);
}

function recordDifference(
  {
    actual,
    expected,
  }: {
    readonly actual: unknown;
    readonly expected: Readonly<Record<string, unknown>>;
  },
  path: string
): string | undefined {
  if (!isRecord(actual) || Array.isArray(actual)) {
    return `${path} expected ${describe(expected)}, got ${describe(actual)}`;
  }
  const extra = Object.keys(actual).find(
    (key) => !Object.hasOwn(expected, key)
  );
  if (extra !== undefined) {
    return `${path}.${extra} unexpected key with ${describe(actual[extra])}`;
  }
  return Object.entries(expected)
    .map(([key, value]) =>
      differenceAt({ actual: actual[key], expected: value }, `${path}.${key}`)
    )
    .find((difference) => difference !== undefined);
}

function differenceAt(
  comparison: Comparison,
  path: string
): string | undefined {
  const { actual, expected } = comparison;
  if (Array.isArray(expected)) {
    return arrayDifference(comparison, path);
  }
  if (isRecord(expected)) {
    return recordDifference({ actual, expected }, path);
  }
  return Object.is(actual, expected) ||
    (typeof actual === "number" && actual === expected)
    ? undefined
    : `${path} expected ${describe(expected)}, got ${describe(actual)}`;
}

function callFailure(
  entry: ToolCallSchemaFuzzCase,
  {
    call,
    expected,
  }: { readonly call: ToolCall; readonly expected: ExpectedCall }
): Failure | undefined {
  const tool = entry.tools.find(
    (candidate) => candidate.function.name === call.function.name
  );
  if (tool === undefined) {
    return {
      category: FuzzFailure.UnknownTool,
      detail: `called ${JSON.stringify(call.function.name)}`,
    };
  }
  if (call.function.name !== expected.name) {
    return {
      category: FuzzFailure.WrongTool,
      detail: `called ${JSON.stringify(call.function.name)}, expected ${JSON.stringify(expected.name)}`,
    };
  }
  const parsed = Either.try((): unknown => JSON.parse(call.function.arguments));
  if (
    Either.isLeft(parsed) ||
    !isRecord(parsed.right) ||
    Array.isArray(parsed.right)
  ) {
    return {
      category: FuzzFailure.InvalidJson,
      detail: `arguments ${JSON.stringify(call.function.arguments)}`,
    };
  }
  const violations = validateJsonSchema(parsed.right, tool.function.parameters);
  if (violations.length > 0) {
    return {
      category: FuzzFailure.SchemaViolation,
      detail: violations
        .map((violation) => `${violation.path} ${violation.message}`)
        .join("; "),
    };
  }
  const difference = firstDifference(parsed.right, expected.args);
  return difference === undefined
    ? undefined
    : { category: FuzzFailure.ValueMismatch, detail: difference };
}

function permutations<T>(items: readonly T[]): readonly (readonly T[])[] {
  if (items.length <= 1) {
    return [items];
  }
  return items.flatMap((item, index) =>
    permutations([...items.slice(0, index), ...items.slice(index + 1)]).map(
      (rest) => [item, ...rest]
    )
  );
}

function orderFailure(
  entry: ToolCallSchemaFuzzCase,
  {
    toolCalls,
    expected,
  }: {
    readonly toolCalls: readonly ToolCall[];
    readonly expected: readonly ExpectedCall[];
  }
): Failure | undefined {
  for (const [index, call] of toolCalls.entries()) {
    const want = expected[index];
    const failure =
      want === undefined
        ? undefined
        : callFailure(entry, { call, expected: want });
    if (failure !== undefined) {
      return toolCalls.length === 1
        ? failure
        : { ...failure, detail: `call[${index}] ${failure.detail}` };
    }
  }
  return undefined;
}

function scoreFuzzToolCalls(
  entry: ToolCallSchemaFuzzCase,
  toolCalls: readonly ToolCall[]
): Score {
  const answer =
    toolCalls.length === 0
      ? null
      : JSON.stringify(
          toolCalls.map(({ function: fn }) => ({
            name: fn.name,
            arguments: fn.arguments,
          }))
        );
  const failure = callsFailure(entry, toolCalls);
  return failure === undefined
    ? {
        value: ScoreValue.Correct,
        answer,
        explanation: "arguments valid and identical to the requested arguments",
      }
    : {
        value: ScoreValue.Incorrect,
        answer,
        explanation: `${failure.category}: ${failure.detail}`,
      };
}

function callsFailure(
  entry: ToolCallSchemaFuzzCase,
  toolCalls: readonly ToolCall[]
): Failure | undefined {
  if (toolCalls.length === 0) {
    return {
      category: FuzzFailure.NoToolCall,
      detail: "response had no tool calls",
    };
  }
  if (toolCalls.length !== entry.calls.length) {
    return {
      category: FuzzFailure.CallCount,
      detail: `expected ${entry.calls.length} call${entry.calls.length === 1 ? "" : "s"}, got ${toolCalls.length}`,
    };
  }
  const orders = permutations(entry.calls);
  const matched = orders.some(
    (expected) => orderFailure(entry, { toolCalls, expected }) === undefined
  );
  return matched
    ? undefined
    : orderFailure(entry, { toolCalls, expected: entry.calls });
}

export function makeToolCallSchemaFuzzScorer(cases: CaseLookup): ScorerService {
  return (state: TaskState) =>
    sync(() => {
      const entry = cases.get(state.sample.id);
      if (entry === undefined) {
        return {
          value: ScoreValue.Incorrect,
          answer: null,
          explanation: `${FuzzFailure.UnknownCase}: no toolcall_schema_fuzz case for sample ${state.sample.id}`,
        };
      }
      if (state.output?.rawResponse?.finish_reason === "length") {
        return {
          value: ScoreValue.Skipped,
          answer: null,
          explanation: `${FuzzFailure.Truncated}: response stopped at the token limit`,
        };
      }
      return scoreFuzzToolCalls(entry, state.output?.message.toolCalls ?? []);
    });
}

export { firstDifference as TEST_firstDifference, scoreFuzzToolCalls };
