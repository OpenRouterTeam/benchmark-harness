import { describe, expect, it } from "bun:test";

import * as Result from "effect/Result";

import { MessageRole, ScoreValue } from "../../harness/core";
import type { SampleScore } from "../../harness/metric";
import {
  formatToolDescriptions,
  jevQuestions,
  jevState,
  jevVerdict,
  judgePrompt,
  liveMcpBenchRunLevelScores,
  parseVerdict,
  toolCallHistory,
  toolTranscript,
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
  it("pairs each assistant tool call with its tool output in order", () => {
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
    ).toEqual(['search({"q":"a"})\n   -> result', "fetch({})\n   -> "]);
  });
});

describe("toolTranscript", () => {
  it("caps long tool outputs and tolerates a missing output", () => {
    const entries = toolTranscript(
      [
        {
          role: MessageRole.Assistant,
          content: "",
          toolCalls: [
            {
              id: "1",
              type: "function",
              function: { name: "a", arguments: "{}" },
            },
            {
              id: "2",
              type: "function",
              function: { name: "b", arguments: "{}" },
            },
          ],
        },
        { role: MessageRole.Tool, content: "0123456789", toolCallId: "1" },
      ],
      4
    );
    expect(entries[0]?.output.startsWith("0123\n")).toBe(true);
    expect(entries[1]?.output).toBe("");
  });
});

describe("jev judge", () => {
  it("asks one step question per key point plus pass, quality and fabricated", () => {
    const questions = jevQuestions("1. look up\n\n2. write file\n");
    expect(Object.keys(questions).sort()).toEqual([
      "fabricated",
      "pass",
      "quality",
      "step_1",
      "step_2",
    ]);
    expect(questions.step_2?.instructions).toContain("2. write file");
  });

  it("puts tool outputs in the state the judge sees", () => {
    const state = jevState({
      today: "2026-10-30",
      task: undefined,
      question: "q",
      response: "r",
      transcript: [{ name: "whois", arguments: "{}", output: "registrar: x" }],
    });
    expect(JSON.stringify(state)).toContain("registrar: x");
  });

  it("passes above 0.5 and records typed fields", () => {
    const verdict = jevVerdict(
      "jev-1",
      {
        pass: { type: "noul", noul: 0.8 },
        quality: { type: "score", score: 2.4 },
        fabricated: { type: "noul", noul: 0.1 },
        step_1: { type: "noul", noul: 0.9 },
      },
      2
    );
    expect(verdict.status).toBe("success");
    expect(verdict.quality).toBe(2.4);
    expect(verdict.stepProbabilities).toEqual([0.9, 0]);
    expect(
      jevVerdict("jev-1", { pass: { type: "noul", noul: 0.5 } }, 0).status
    ).toBe("failure");
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
