import { describe, expect, it } from "bun:test";
import assert from "node:assert/strict";

import type { ToolCall } from "../../harness/core";
import { ScoreValue } from "../../harness/core";
import type { ToolCallFormatCase } from "./cases";
import { getToolCallFormatCase } from "./cases";
import { scoreToolCalls, valuesMatch } from "./scorer";

function caseOf(id: string): ToolCallFormatCase {
  const entry = getToolCallFormatCase(id);
  assert(entry !== undefined, `missing case ${id}`);
  return entry;
}

function call(name: string, args: string, id = "call_1"): ToolCall {
  return { id, type: "function", function: { name, arguments: args } };
}

const READ_SLICE_STRICT = caseOf(
  "toolcall_formats-read_slice-anyof_null_strict"
);

describe("scoreToolCalls", () => {
  it("accepts well-typed arguments", () => {
    const score = scoreToolCalls(READ_SLICE_STRICT, [
      call("read", '{"path":"src/server.ts","offset":40,"limit":41}'),
    ]);
    expect(score.value).toBe(ScoreValue.Correct);
  });

  it("flags Morph's stringified numbers as a schema violation", () => {
    const score = scoreToolCalls(READ_SLICE_STRICT, [
      call("read", '{"limit":"41","offset":"40","path":"src/server.ts"}'),
    ]);
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.explanation).toBe(
      'schema_violation: read $.limit string "41" matched no anyOf branch; $.offset string "40" matched no anyOf branch'
    );
  });

  it('flags the string "null" and missing required nullable fields', () => {
    const whole = caseOf("toolcall_formats-read_whole-anyof_null_strict");
    expect(
      scoreToolCalls(whole, [
        call("read", '{"path":"README.md","offset":null,"limit":"null"}'),
      ]).explanation
    ).toStartWith("schema_violation");
    expect(
      scoreToolCalls(whole, [call("read", '{"path":"README.md"}')]).explanation
    ).toStartWith("schema_violation");
  });

  it("accepts any schema-valid value for keys the prompt leaves unset", () => {
    const ticket = caseOf("toolcall_formats-ticket_minimal-anyof_null");
    const score = scoreToolCalls(ticket, [
      call(
        "create_ticket",
        '{"title":"Typo on pricing page","priority":"low","blocking":false,"assignee":"null"}'
      ),
    ]);
    expect(score.value).toBe(ScoreValue.Correct);
  });

  it("allows omitted optionals when the schema leaves them out of required", () => {
    const whole = caseOf("toolcall_formats-read_whole-omitted");
    expect(
      scoreToolCalls(whole, [call("read", '{"path":"README.md"}')]).value
    ).toBe(ScoreValue.Correct);
  });

  it("classifies missing, malformed, unknown and wrong-valued calls", () => {
    expect(scoreToolCalls(READ_SLICE_STRICT, []).explanation).toStartWith(
      "no_tool_call"
    );
    expect(
      scoreToolCalls(READ_SLICE_STRICT, [call("read", '{"path": "src/')])
        .explanation
    ).toStartWith("invalid_json");
    expect(
      scoreToolCalls(READ_SLICE_STRICT, [call("write", "{}")]).explanation
    ).toStartWith("unknown_tool");
    expect(
      scoreToolCalls(READ_SLICE_STRICT, [
        call("read", '{"path":"src/server.ts","offset":4,"limit":null}'),
      ]).explanation
    ).toStartWith("wrong_value");
  });

  it("matches parallel calls in any order and requires the exact count", () => {
    const parallel = caseOf("toolcall_formats-parallel_reads-type_array_null");
    const reads = ["README.md", "package.json", "tsconfig.json"].map(
      (path, index) =>
        call(
          "read",
          JSON.stringify({ path, offset: null, limit: null }),
          `call_${index}`
        )
    );
    expect(scoreToolCalls(parallel, reads).value).toBe(ScoreValue.Correct);
    expect(scoreToolCalls(parallel, reads.slice(1)).explanation).toBe(
      "call_count: expected 3 calls, got 2"
    );
  });
});

describe("valuesMatch", () => {
  it("compares numbers numerically, strings exactly and arrays positionally", () => {
    expect(valuesMatch(1e-4, 0.0001)).toBe(true);
    expect(valuesMatch("40", 40)).toBe(false);
    expect(valuesMatch(["bug", "p1"], ["bug", "p1"])).toBe(true);
    expect(valuesMatch(["p1", "bug"], ["bug", "p1"])).toBe(false);
    expect(valuesMatch(" x", "x")).toBe(false);
    expect(valuesMatch(undefined, null)).toBe(true);
    expect(valuesMatch({ a: 1, b: 2 }, { a: 1 })).toBe(true);
  });

  it("rejects wrong titles, short slices, appends, padded content and swapped pairings", () => {
    const wrong = (
      id: string,
      toolCall: { readonly name: string; readonly args: unknown }
    ) =>
      scoreToolCalls(caseOf(`toolcall_formats-${id}`), [
        call(toolCall.name, JSON.stringify(toolCall.args)),
      ]).explanation;
    expect(
      wrong("ticket_full-omitted", {
        name: "create_ticket",
        args: {
          title: "Other issue",
          priority: "urgent",
          blocking: true,
          assignee: "dana",
        },
      })
    ).toStartWith("wrong_value");
    expect(
      wrong("read_slice-omitted", {
        name: "read",
        args: { path: "src/server.ts", offset: 40, limit: 1 },
      })
    ).toStartWith("wrong_value");
    const content = '{"port": 8080, "debug": true, "tags": ["x", "y"]}';
    expect(
      wrong("write_json_content-omitted", {
        name: "write_file",
        args: {
          path: "config/app.json",
          content,
          append: true,
        },
      })
    ).toStartWith("wrong_value");
    expect(
      wrong("write_json_content-omitted", {
        name: "write_file",
        args: {
          path: "config/app.json",
          content: ` ${content}`,
        },
      })
    ).toStartWith("wrong_value");
    expect(
      wrong("label_weights-omitted", {
        name: "label_issue",
        args: {
          issue_id: 77,
          labels: ["infra", "perf"],
          weights: [0.25, 1.5],
        },
      })
    ).toStartWith("wrong_value");
  });

  it("accepts unweighted labels in any order", () => {
    const labels = caseOf("toolcall_formats-label_array-omitted");
    const score = (value: readonly string[]) =>
      scoreToolCalls(labels, [
        call("label_issue", JSON.stringify({ issue_id: 4821, labels: value })),
      ]).explanation;

    expect(score(["regression", "bug", "p1"])).toEndWith("matched");
    expect(score(["regression", "bug"])).toStartWith("wrong_value");
    expect(score(["regression", "bug", "bug"])).toStartWith("wrong_value");
  });

  it("accepts false, null or omission for an overwrite", () => {
    const write = caseOf("toolcall_formats-write_json_content-anyof_null");
    const content = '{"port": 8080, "debug": true, "tags": ["x", "y"]}';
    for (const append of [false, null]) {
      expect(
        scoreToolCalls(write, [
          call(
            "write_file",
            JSON.stringify({ path: "config/app.json", content, append })
          ),
        ]).value
      ).toBe(ScoreValue.Correct);
    }
  });
});
