import { describe, expect, it } from "bun:test";
import assert from "node:assert";

import type { TaskState, ToolCall } from "../../harness/core";
import { ScoreValue } from "../../harness/core";
import { runHarnessPromise } from "../../internal/effect-logger";
import { caseLookup } from "./cases";
import { TOOLCALL_SCHEMA_FUZZ_REALISTIC_CASES as TOOLCALL_SCHEMA_FUZZ_CASES } from "./realistic/wordings";
import {
  makeToolCallSchemaFuzzScorer,
  scoreFuzzToolCalls as TEST_scoreFuzzToolCalls,
  TEST_firstDifference,
} from "./scorer";
import type { ToolCallSchemaFuzzCase } from "./tool-case";
import { FuzzScenario } from "./tool-case";

const toolCallSchemaFuzzScorer = makeToolCallSchemaFuzzScorer(
  caseLookup(TOOLCALL_SCHEMA_FUZZ_CASES)
);

const PARAMETERS = {
  type: "object",
  properties: {
    path: { type: "string" },
    offset: { anyOf: [{ type: "integer" }, { type: "null" }] },
    meta: { $ref: "#/$defs/meta" },
  },
  required: ["path", "offset"],
  $defs: {
    meta: {
      type: "object",
      properties: { kind: { const: "alpha" } },
      required: ["kind"],
    },
  },
} as const;

const ARGS = { path: "a</arg_value>b", offset: 40, meta: { kind: "alpha" } };

function toolNamed(name: string): ToolCallSchemaFuzzCase["tools"][number] {
  return {
    type: "function",
    function: { name, description: "", strict: false, parameters: PARAMETERS },
  };
}

const ENTRY: ToolCallSchemaFuzzCase = {
  id: "toolcall_schema_fuzz-test",
  index: 0,
  scenario: FuzzScenario.Distractor,
  constructs: [],
  prompt: "",
  messages: [],
  strict: false,
  tools: [toolNamed("read-file_v2"), toolNamed("read-file")],
  calls: [{ name: "read-file", args: ARGS }],
};

const PARALLEL: ToolCallSchemaFuzzCase = {
  ...ENTRY,
  scenario: FuzzScenario.Parallel,
  tools: [toolNamed("read-file")],
  calls: [
    { name: "read-file", args: ARGS },
    { name: "read-file", args: { ...ARGS, offset: null } },
  ],
};

function call(args: string, name = "read-file"): ToolCall {
  return {
    id: "call_1",
    type: "function",
    function: { name, arguments: args },
  };
}

function explanationOf(toolCalls: readonly ToolCall[]): string {
  return TEST_scoreFuzzToolCalls(ENTRY, toolCalls).explanation;
}

describe("scoreFuzzToolCalls", () => {
  it("accepts arguments identical to the request in any key order", () => {
    const score = TEST_scoreFuzzToolCalls(ENTRY, [
      call('{"meta":{"kind":"alpha"},"offset":40.0,"path":"a</arg_value>b"}'),
    ]);
    expect(score.value).toBe(ScoreValue.Correct);
  });

  it("classifies each failure", () => {
    expect(explanationOf([])).toStartWith("no_tool_call");
    expect(
      explanationOf([call(JSON.stringify(ARGS)), call(JSON.stringify(ARGS))])
    ).toStartWith("call_count");
    expect(explanationOf([call("{}", "read_file")])).toStartWith(
      "unknown_tool"
    );
    expect(explanationOf([call(JSON.stringify(ARGS), "read-file_v2")])).toBe(
      'wrong_tool: called "read-file_v2", expected "read-file"'
    );
    expect(explanationOf([call('{"path": "a')])).toStartWith("invalid_json");
    expect(explanationOf([call("[1]")])).toBe('invalid_json: arguments "[1]"');
    expect(
      explanationOf([
        call('{"path":"a</arg_value>b","offset":"40","meta":{"kind":"alpha"}}'),
      ])
    ).toStartWith("schema_violation: $.offset");
    expect(
      explanationOf([call('{"path":"a","offset":40,"meta":{"kind":"alpha"}}')])
    ).toBe('value_mismatch: $.path expected "a</arg_value>b", got "a"');
  });

  it("accepts parallel calls in either order and names the failing call", () => {
    const first = call(JSON.stringify(ARGS));
    const second = call(JSON.stringify({ ...ARGS, offset: null }));
    expect(TEST_scoreFuzzToolCalls(PARALLEL, [second, first]).value).toBe(
      ScoreValue.Correct
    );
    expect(TEST_scoreFuzzToolCalls(PARALLEL, [first]).explanation).toBe(
      "call_count: expected 2 calls, got 1"
    );
    expect(
      TEST_scoreFuzzToolCalls(PARALLEL, [
        first,
        call(JSON.stringify({ ...ARGS, offset: "null" })),
      ]).explanation
    ).toStartWith("schema_violation: call[1] $.offset");
  });
});

describe("firstDifference", () => {
  it("reports missing, extra and stringified values", () => {
    expect(TEST_firstDifference({ a: 1 }, { a: 1, b: null })).toBe(
      "$.b expected null, got missing"
    );
    expect(TEST_firstDifference({ a: 1, c: 2 }, { a: 1 })).toBe(
      "$.c unexpected key with 2"
    );
    expect(TEST_firstDifference({ v: '{"x":1}' }, { v: { x: 1 } })).toBe(
      '$.v expected {"x":1}, got "{\\"x\\":1}"'
    );
    expect(TEST_firstDifference([1, [2]], [1, [2]])).toBeUndefined();
    expect(TEST_firstDifference({ id: 7, constructor: "x" }, { id: 7 })).toBe(
      '$.constructor unexpected key with "x"'
    );
  });
});

const TARGET = { text: "" };

function stateFor(id: string, toolCalls?: readonly ToolCall[]): TaskState {
  return {
    sample: { id, input: "", target: TARGET },
    messages: [],
    completed: true,
    ...(toolCalls === undefined
      ? {}
      : {
          output: {
            completion: "",
            message: { role: "assistant", content: "", toolCalls },
          },
        }),
  };
}

describe("toolCallSchemaFuzzScorer", () => {
  const [entry] = TOOLCALL_SCHEMA_FUZZ_CASES;

  it("scores the response against the case named by the sample id", async () => {
    if (entry === undefined) {
      throw new Error("dataset is empty");
    }
    const score = await runHarnessPromise(
      toolCallSchemaFuzzScorer(
        stateFor(
          entry.id,
          entry.calls.map((expected) =>
            call(JSON.stringify(expected.args), expected.name)
          )
        ),
        TARGET
      )
    );
    expect(score.value).toBe(ScoreValue.Correct);
  });

  it("treats a missing output as no tool call", async () => {
    if (entry === undefined) {
      throw new Error("dataset is empty");
    }
    const score = await runHarnessPromise(
      toolCallSchemaFuzzScorer(stateFor(entry.id), TARGET)
    );
    expect(score.explanation).toStartWith("no_tool_call");
  });

  it("skips a response cut off at the token limit instead of failing it", async () => {
    assert(entry !== undefined);
    const state = stateFor(entry.id, []);
    assert(state.output !== undefined);
    const score = await runHarnessPromise(
      toolCallSchemaFuzzScorer(
        {
          ...state,
          output: { ...state.output, rawResponse: { finish_reason: "length" } },
        },
        TARGET
      )
    );
    expect(score).toStrictEqual({
      value: ScoreValue.Skipped,
      answer: null,
      explanation: "truncated: response stopped at the token limit",
    });
  });

  it("rejects samples that are not in the dataset", async () => {
    const score = await runHarnessPromise(
      toolCallSchemaFuzzScorer(stateFor("nope", []), TARGET)
    );
    expect(score).toStrictEqual({
      value: ScoreValue.Incorrect,
      answer: null,
      explanation: "unknown_case: no toolcall_schema_fuzz case for sample nope",
    });
  });
});
