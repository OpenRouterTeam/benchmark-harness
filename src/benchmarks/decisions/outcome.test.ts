import { describe, expect, it } from "bun:test";

import { InvariantViolation } from "./invariants";
import {
  answerDistribution,
  decisionOutcomeFromResponse,
  extractLlmLabel,
  isHardViolation,
  llmOutcomeFromCompletion,
  OutcomeViolation,
  parseDecisionOutcome,
  verbalizedDistribution,
} from "./outcome";
import { LLM_SYSTEM_PROMPT, renderLlmPrompt } from "./prompt";
import type { DecisionTaskSpec } from "./schema";
import { DecisionArm, DecisionTaskId, DecisionVariant } from "./schema";

const CHOICE_SPEC: DecisionTaskSpec = {
  task: DecisionTaskId.Xnli,
  variant: DecisionVariant.Base,
  language: "en",
  questionId: "relation",
  state: "Premise: p\n\nHypothesis: h",
  stateText: "Premise: p\n\nHypothesis: h",
  question: {
    type: "choice",
    instructions: "What is the relation?",
    criteria: { entailment: "must", neutral: null, contradiction: "cannot" },
  },
  labels: ["entailment", "neutral", "contradiction"],
  gold: "neutral",
  goldKnowable: true,
};

const NOUL_SPEC: DecisionTaskSpec = {
  ...CHOICE_SPEC,
  task: DecisionTaskId.BoolQ,
  questionId: "answer_is_yes",
  question: { type: "noul", instructions: "Yes?" },
  labels: ["true", "false"],
  gold: "true",
};

const SCORE_SPEC: DecisionTaskSpec = {
  ...CHOICE_SPEC,
  task: DecisionTaskId.Sst5,
  questionId: "sentiment",
  question: {
    type: "score",
    instructions: "How positive?",
    criteria: ["negative", "neutral", "positive"],
  },
  labels: ["0", "1", "2"],
  gold: "2",
};

describe("answerDistribution", () => {
  it("maps a noul probability onto true/false labels", () => {
    expect(
      answerDistribution({ type: "noul", noul: 0.3 }, ["true", "false"])
    ).toEqual({ true: 0.3, false: 0.7 });
  });
  it("projects choice probabilities onto the label set with zero for missing keys", () => {
    expect(
      answerDistribution(
        { type: "choice", choice: "a", probabilities: { a: 0.6, b: 0.4 } },
        ["a", "b", "c"]
      )
    ).toEqual({ a: 0.6, b: 0.4, c: 0 });
  });
});

describe("decisionOutcomeFromResponse", () => {
  it("records a missing answer as a hard violation with no distribution", () => {
    const outcome = decisionOutcomeFromResponse({
      spec: CHOICE_SPEC,
      response: { model: "typesafe/jev-latest", answers: {} },
      latencyMs: 12,
    });
    expect(outcome.arm).toBe(DecisionArm.Decision);
    expect(outcome.violations).toEqual([OutcomeViolation.MissingAnswer]);
    expect(outcome.distribution).toEqual({});
    expect(outcome.argmax).toBeNull();
    expect(outcome.cost).toBeNull();
  });
  it("converts a conforming choice answer and carries usage, provider, and latency", () => {
    const outcome = decisionOutcomeFromResponse({
      spec: CHOICE_SPEC,
      response: {
        id: "gen-1",
        model: "typesafe/jev-1.13.0",
        provider: "typesafe",
        answers: {
          relation: {
            type: "choice",
            choice: "neutral",
            probabilities: {
              entailment: 0.1,
              neutral: 0.8,
              contradiction: 0.1,
            },
            confidence: 0.8,
          },
        },
        usage: { input_tokens: 50, output_tokens: 0, cost: 0.0000021 },
      },
      latencyMs: 90,
    });
    expect(outcome.violations).toEqual([]);
    expect(outcome.argmax).toBe("neutral");
    expect(outcome.confidence).toBe(0.8);
    expect(outcome.scoreValue).toBeNull();
    expect(outcome.provider).toBe("typesafe");
    expect(outcome.model).toBe("typesafe/jev-1.13.0");
    expect(outcome.inputTokens).toBe(50);
    expect(outcome.outputTokens).toBe(0);
    expect(outcome.cost).toBeCloseTo(0.0000021, 12);
    expect(outcome.latencyMs).toBe(90);
  });
  it("keeps invariant violations alongside the projected distribution", () => {
    const outcome = decisionOutcomeFromResponse({
      spec: CHOICE_SPEC,
      response: {
        model: "m",
        answers: {
          relation: {
            type: "choice",
            choice: "neutral",
            probabilities: {
              entailment: 0.6,
              neutral: 0.3,
              contradiction: 0.1,
            },
          },
        },
      },
      latencyMs: 1,
    });
    expect(outcome.violations).toEqual([InvariantViolation.ArgmaxMismatch]);
    expect(outcome.argmax).toBe("entailment");
  });
  it("exposes the score value for score answers", () => {
    const outcome = decisionOutcomeFromResponse({
      spec: SCORE_SPEC,
      response: {
        model: "m",
        answers: {
          sentiment: {
            type: "score",
            score: 1.5,
            legend: { "0": "negative", "1": "neutral", "2": "positive" },
            probabilities: { "0": 0, "1": 0.5, "2": 0.5 },
          },
        },
      },
      latencyMs: 1,
    });
    expect(outcome.scoreValue).toBe(1.5);
    expect(outcome.violations).toEqual([]);
  });
});

describe("extractLlmLabel", () => {
  const labels = CHOICE_SPEC.labels;
  it("reads the answer and confidence from the last JSON object", () => {
    expect(
      extractLlmLabel(
        'thinking {"answer": "entailment"} final {"answer": "neutral", "confidence": 0.7}',
        labels
      )
    ).toEqual({ label: "neutral", confidence: 0.7, violation: null });
  });
  it("matches labels case-insensitively but does not repair other differences", () => {
    expect(extractLlmLabel('{"answer": "Neutral"}', labels).label).toBe(
      "neutral"
    );
    expect(extractLlmLabel('{"answer": "neutral."}', labels)).toEqual({
      label: null,
      confidence: null,
      violation: OutcomeViolation.LabelNotInOptions,
    });
  });
  it("reports unparsable output when there is no JSON object or no answer key", () => {
    expect(extractLlmLabel("neutral", labels).violation).toBe(
      OutcomeViolation.UnparsableOutput
    );
    expect(extractLlmLabel('{"label": "neutral"}', labels).violation).toBe(
      OutcomeViolation.UnparsableOutput
    );
  });
  it("accepts numeric answers for level-index labels", () => {
    expect(
      extractLlmLabel('{"answer": 2, "confidence": 0.9}', SCORE_SPEC.labels)
    ).toEqual({ label: "2", confidence: 0.9, violation: null });
  });
  it("drops confidence values outside the unit interval", () => {
    expect(
      extractLlmLabel('{"answer": "neutral", "confidence": 7}', labels)
        .confidence
    ).toBeNull();
    expect(
      extractLlmLabel('{"answer": "neutral", "confidence": "0.5"}', labels)
        .confidence
    ).toBeNull();
  });
});

describe("verbalizedDistribution", () => {
  it("gives the chosen label its confidence and splits the rest evenly", () => {
    const distribution = verbalizedDistribution(["a", "b", "c"], "b", 0.7);
    expect(distribution.b).toBe(0.7);
    expect(distribution.a).toBeCloseTo(0.15, 12);
    expect(distribution.c).toBeCloseTo(0.15, 12);
  });
  it("puts all mass on a single-label set", () => {
    expect(verbalizedDistribution(["a"], "a", 0.4)).toEqual({ a: 0.4 });
  });
});

describe("llmOutcomeFromCompletion", () => {
  const base = {
    model: "openai/gpt-x",
    latencyMs: 400,
    cost: 0.001,
    inputTokens: 100,
    outputTokens: 12,
  };
  it("builds a confidence-weighted distribution for a valid answer", () => {
    const outcome = llmOutcomeFromCompletion({
      ...base,
      spec: CHOICE_SPEC,
      completion: '{"answer": "neutral", "confidence": 0.8}',
    });
    expect(outcome.arm).toBe(DecisionArm.Llm);
    expect(outcome.argmax).toBe("neutral");
    expect(outcome.confidence).toBe(0.8);
    expect(outcome.distribution.neutral).toBeCloseTo(0.8, 12);
    expect(outcome.distribution.entailment).toBeCloseTo(0.1, 12);
    expect(outcome.violations).toEqual([]);
    expect(outcome.rawAnswer).toBe('{"answer": "neutral", "confidence": 0.8}');
  });
  it("records a soft violation and a point mass when confidence is omitted", () => {
    const outcome = llmOutcomeFromCompletion({
      ...base,
      spec: CHOICE_SPEC,
      completion: '{"answer": "neutral"}',
    });
    expect(outcome.violations).toEqual([OutcomeViolation.MissingConfidence]);
    expect(outcome.confidence).toBeNull();
    expect(outcome.distribution).toEqual({
      entailment: 0,
      neutral: 1,
      contradiction: 0,
    });
    expect(isHardViolation(OutcomeViolation.MissingConfidence)).toBe(false);
  });
  it("flags confidence below uniform as incompatible with the chosen label", () => {
    const outcome = llmOutcomeFromCompletion({
      ...base,
      spec: CHOICE_SPEC,
      completion: '{"answer": "neutral", "confidence": 0.2}',
    });
    expect(outcome.violations).toEqual([
      OutcomeViolation.ConfidenceBelowUniform,
    ]);
    expect(isHardViolation(OutcomeViolation.ConfidenceBelowUniform)).toBe(
      false
    );
  });
  it("leaves the distribution empty for malformed or out-of-option output", () => {
    const outcome = llmOutcomeFromCompletion({
      ...base,
      spec: CHOICE_SPEC,
      completion: '{"answer": "maybe", "confidence": 0.9}',
    });
    expect(outcome.distribution).toEqual({});
    expect(outcome.argmax).toBeNull();
    expect(outcome.confidence).toBeNull();
    expect(outcome.violations).toEqual([OutcomeViolation.LabelNotInOptions]);
    expect(isHardViolation(OutcomeViolation.LabelNotInOptions)).toBe(true);
  });
  it("derives the score value from the chosen level for score questions", () => {
    const outcome = llmOutcomeFromCompletion({
      ...base,
      spec: SCORE_SPEC,
      completion: '{"answer": "2", "confidence": 0.6}',
    });
    expect(outcome.scoreValue).toBe(2);
  });
  it("round-trips through the outcome schema", () => {
    const outcome = llmOutcomeFromCompletion({
      ...base,
      spec: NOUL_SPEC,
      completion: '{"answer": "true", "confidence": 0.55}',
    });
    expect(parseDecisionOutcome(JSON.parse(JSON.stringify(outcome)))).toEqual(
      outcome
    );
    expect(parseDecisionOutcome({ arm: "decision" })).toBeNull();
  });
});

describe("renderLlmPrompt", () => {
  it("renders state, instructions, and labelled options with criteria", () => {
    const prompt = renderLlmPrompt(CHOICE_SPEC);
    expect(prompt).toContain("STATE:\nPremise: p\n\nHypothesis: h");
    expect(prompt).toContain("QUESTION:\nWhat is the relation?");
    expect(prompt).toContain(
      "- entailment: must\n- neutral\n- contradiction: cannot"
    );
    expect(prompt).toContain('{"answer": "<option label>"');
  });
  it("renders object state as JSON", () => {
    const prompt = renderLlmPrompt({
      ...CHOICE_SPEC,
      state: { text: "p", item_index: 1 },
    });
    expect(prompt).toContain('STATE:\n{"text":"p","item_index":1}');
  });
  it("renders score levels by index with their rubric text", () => {
    const prompt = renderLlmPrompt(SCORE_SPEC);
    expect(prompt).toContain("- 0: negative\n- 1: neutral\n- 2: positive");
  });
  it("renders noul labels with and without criteria", () => {
    expect(renderLlmPrompt(NOUL_SPEC)).toContain("- true\n- false");
    const withCriteria = renderLlmPrompt({
      ...NOUL_SPEC,
      question: {
        type: "noul",
        instructions: "Yes?",
        criteria: { true: "It is yes.", false: "It is no." },
      },
    });
    expect(withCriteria).toContain("- true: It is yes.\n- false: It is no.");
  });
  it("system prompt demands the closed-set JSON contract", () => {
    expect(LLM_SYSTEM_PROMPT).toContain("confidence");
    expect(LLM_SYSTEM_PROMPT).toContain("Do not invent labels");
  });
});
