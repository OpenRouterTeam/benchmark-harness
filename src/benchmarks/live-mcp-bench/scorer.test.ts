import { describe, expect, it } from "bun:test";

import * as Result from "effect/Result";

import { MessageRole, ScoreValue } from "../../harness/core";
import type { SampleScore } from "../../harness/metric";
import {
  formatToolDescriptions,
  judgePrompt,
  liveMcpBenchRunLevelScores,
  parseVerdict,
  toolCallHistory,
} from "./scorer";

describe("parseVerdict", () => {
  it("reads the JSON verdict", () => {
    const verdict = parseVerdict('{"thoughts":"ok","status":"success"}');
    expect(Result.isSuccess(verdict) && verdict.success).toEqual({
      thoughts: "ok",
      status: "success",
    });
  });

  it("falls back to the upstream Status line", () => {
    const verdict = parseVerdict(
      'Thoughts: the file was never written\nStatus: "failure"'
    );
    expect(Result.isSuccess(verdict) && verdict.success.status).toBe("failure");
  });

  it("fails without a status", () => {
    expect(Result.isFailure(parseVerdict("no verdict here"))).toBe(true);
  });

  it("fails on JSON with an unknown status", () => {
    expect(
      Result.isFailure(parseVerdict('{"thoughts":"x","status":"maybe"}'))
    ).toBe(true);
  });
});

describe("toolCallHistory", () => {
  it("lists assistant tool calls as name(arguments) in order", () => {
    expect(
      toolCallHistory([
        { role: MessageRole.User, content: "task" },
        {
          role: MessageRole.Assistant,
          content: "",
          toolCalls: [
            {
              id: "1",
              type: "function",
              function: { name: "search", arguments: '{"q":"a"}' },
            },
          ],
        },
        { role: MessageRole.Tool, content: "result", toolCallId: "1" },
        {
          role: MessageRole.Assistant,
          content: "",
          toolCalls: [
            {
              id: "2",
              type: "function",
              function: { name: "fetch", arguments: "{}" },
            },
          ],
        },
      ])
    ).toEqual(['search({"q":"a"})', "fetch({})"]);
  });
});

describe("judgePrompt", () => {
  it("numbers the tool calls and carries every section", () => {
    const prompt = judgePrompt({
      task: "Find a flight",
      keyPoints: "1. search\n2. answer",
      response: "Flight found",
      toolCalls: ["search({})", "fetch({})"],
      toolDescriptions: formatToolDescriptions({ search: "Search flights" }),
    });
    expect(prompt).toContain("User Task: \nFind a flight");
    expect(prompt).toContain("1. search({})\n2. fetch({})");
    expect(prompt).toContain("Tool: search\nDescription: Search flights");
  });
});

describe("liveMcpBenchRunLevelScores", () => {
  const sampleScore = (
    value: ScoreValue,
    run: Record<string, unknown> | undefined
  ): SampleScore => ({
    sampleId: "s",
    epoch: 0,
    score: {
      value,
      answer: null,
      explanation: "",
      ...(run === undefined
        ? {}
        : { trajectory: { kind: "judge_runs" as const, runs: [run] } }),
    },
  });

  it("reports the overall and per-category success rates", () => {
    const scores = liveMcpBenchRunLevelScores({
      sampleScores: [
        sampleScore(ScoreValue.Correct, {
          status: "success",
          thoughts: "",
          category: "Office",
        }),
        sampleScore(ScoreValue.Incorrect, {
          status: "failure",
          thoughts: "",
          category: "Office",
        }),
        sampleScore(ScoreValue.Correct, {
          status: "success",
          thoughts: "",
          category: "Travel",
        }),
        sampleScore(ScoreValue.Skipped, undefined),
      ],
    });
    expect(scores).toEqual([
      {
        name: "live_mcp_bench_success",
        metrics: {
          success_rate: { value: 2 / 3 },
          success_rate_office: { value: 0.5 },
          success_rate_travel: { value: 1 },
        },
      },
    ]);
  });

  it("returns nothing when every sample was skipped", () => {
    expect(
      liveMcpBenchRunLevelScores({
        sampleScores: [sampleScore(ScoreValue.Skipped, undefined)],
      })
    ).toEqual([]);
  });
});
