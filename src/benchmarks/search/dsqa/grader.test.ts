import { describe, expect, it } from "bun:test";
import { createHash } from "node:crypto";

import { assertLeft, assertRight } from "../../../internal/testing";
import {
  DSQA_GRADER_PROMPT,
  DSQA_JUDGE_CONFIG,
  dsqaJudgeSpec,
  parseDsqaVerdict,
  renderDsqaGraderPrompt,
} from "./grader";
describe("DSQA grader", () => {
  it("pins the official prompt and judge", () => {
    expect(createHash("sha256").update(DSQA_GRADER_PROMPT).digest("hex")).toBe(
      "9bdd0b9198244de8a78bf256b5332805d00c140e85b713f0e1878b3e4aa605a0"
    );
    expect(DSQA_JUDGE_CONFIG.judgeModel).toBe("google/gemini-2.5-flash");
    expect(DSQA_JUDGE_CONFIG.reasoningEffort).toBe("low");
  });
  it("renders the benchmark inputs inside the official wrappers", () => {
    const prompt = renderDsqaGraderPrompt({
      question: "Name both countries.",
      promptType: "Set Answer",
      correctAnswer: "Belgium, France",
      response: "Belgium and France",
    });
    expect(prompt).toContain("<prompt>\nName both countries.\n</prompt>");
    expect(prompt).toContain("Prompt Type: Set Answer");
    expect(prompt).toContain("<answer>\nBelgium, France\n</answer>");
    expect(prompt).toContain("<response>\nBelgium and France\n</response>");
  });
  it("parses the official nested verdict", () => {
    const result = parseDsqaVerdict(
      JSON.stringify({
        "Answer Correctness": {
          Explanation: "Belgium was found but France was not.",
          "Correctness Details": { Belgium: true, France: false },
          "Excessive Answers": ["Italy"],
        },
      })
    );
    assertRight(result);
    expect(result.right).toEqual({
      explanation: "Belgium was found but France was not.",
      correctness_details: { Belgium: true, France: false },
      excessive_answers: ["Italy"],
    });
  });
  it("parses fenced output", () => {
    const result = parseDsqaVerdict(`prefix "\`\`\`json
{"Answer Correctness":{"Explanation":"ok","Correctness Details":{"A":true},"Excessive Answers":[]}}
\`\`\`" suffix`);
    assertRight(result);
    expect(result.right.excessive_answers).toEqual([]);
  });
  it("rejects malformed verdicts", () => {
    for (const text of [
      "null",
      '{"Answer Correctness":{"Explanation":"x","Correctness Details":{"A":"true"}}}',
      '{"Answer Correctness":{"Explanation":"x","Correctness Details":[]}}',
      '{"Answer Correctness":{"Explanation":"x","Correctness Details":{"A":true},"Excessive Answers":[1]}}',
    ]) {
      assertLeft(parseDsqaVerdict(text));
    }
  });
  it("uses the official unstructured verdict contract", () => {
    const spec = dsqaJudgeSpec({
      question: "Q",
      promptType: "Single Answer",
      correctAnswer: "A",
      response: "A",
    });
    expect(spec.schemaName).toBe("dsqa_judge");
    expect(spec.jsonSchema).toBeUndefined();
    expect(spec.parseFailureFallback).toEqual({
      explanation: "Judge verdict could not be parsed.",
      correctness_details: {},
      excessive_answers: [],
    });
  });
});
