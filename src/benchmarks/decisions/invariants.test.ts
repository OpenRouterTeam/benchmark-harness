import { describe, expect, it } from "bun:test";

import {
  checkAnswerInvariants,
  checkResponseInvariants,
  InvariantViolation,
} from "./invariants";
import type {
  ChoiceQuestion,
  DecisionsResponse,
  NoulQuestion,
  ScoreQuestion,
} from "./schema";

const NOUL: NoulQuestion = { type: "noul", instructions: "Is it sunny?" };

const CHOICE: ChoiceQuestion = {
  type: "choice",
  instructions: "Pick the intent.",
  criteria: {
    refund: "Wants money back",
    cancel: "Wants to cancel",
    other: null,
  },
};

const SCORE: ScoreQuestion = {
  type: "score",
  instructions: "Rate the tone.",
  criteria: ["hostile", "neutral", "friendly"],
};

describe("checkAnswerInvariants", () => {
  it("accepts a well-formed noul answer", () => {
    expect(checkAnswerInvariants(NOUL, { type: "noul", noul: 0.42 })).toEqual(
      []
    );
  });
  it("rejects a noul probability outside the unit interval", () => {
    expect(checkAnswerInvariants(NOUL, { type: "noul", noul: 1.5 })).toEqual([
      InvariantViolation.ProbabilityOutOfRange,
    ]);
    expect(
      checkAnswerInvariants(NOUL, { type: "noul", noul: Number.NaN })
    ).toEqual([InvariantViolation.ProbabilityOutOfRange]);
  });
  it("reports a type mismatch before any other check", () => {
    expect(
      checkAnswerInvariants(NOUL, {
        type: "choice",
        choice: "refund",
        probabilities: { refund: 1 },
      })
    ).toEqual([InvariantViolation.TypeMismatch]);
  });

  it("accepts a well-formed choice answer", () => {
    expect(
      checkAnswerInvariants(CHOICE, {
        type: "choice",
        choice: "refund",
        probabilities: { refund: 0.7, cancel: 0.2, other: 0.1 },
        confidence: 0.7,
      })
    ).toEqual([]);
  });
  it("flags a choice outside the caller-supplied options", () => {
    const violations = checkAnswerInvariants(CHOICE, {
      type: "choice",
      choice: "upgrade",
      probabilities: { refund: 0.7, cancel: 0.2, other: 0.1 },
    });
    expect(violations).toContain(InvariantViolation.ChoiceNotInOptions);
    expect(violations).toContain(InvariantViolation.ArgmaxMismatch);
  });
  it("flags probability keys that do not match the option set", () => {
    const violations = checkAnswerInvariants(CHOICE, {
      type: "choice",
      choice: "refund",
      probabilities: { refund: 0.7, cancel: 0.3 },
    });
    expect(violations).toEqual([InvariantViolation.ProbabilityKeysMismatch]);
  });
  it("flags probabilities that do not sum to one", () => {
    const violations = checkAnswerInvariants(CHOICE, {
      type: "choice",
      choice: "refund",
      probabilities: { refund: 0.7, cancel: 0.7, other: 0.1 },
    });
    expect(violations).toEqual([InvariantViolation.ProbabilitiesDoNotSum]);
  });
  it("flags a choice that is not the argmax of its own distribution", () => {
    const violations = checkAnswerInvariants(CHOICE, {
      type: "choice",
      choice: "other",
      probabilities: { refund: 0.7, cancel: 0.2, other: 0.1 },
    });
    expect(violations).toEqual([InvariantViolation.ArgmaxMismatch]);
  });
  it("tolerates an argmax tie within the sum tolerance", () => {
    expect(
      checkAnswerInvariants(CHOICE, {
        type: "choice",
        choice: "cancel",
        probabilities: { refund: 0.5, cancel: 0.49, other: 0.01 },
      })
    ).toEqual([]);
  });
  it("flags a confidence outside the unit interval", () => {
    expect(
      checkAnswerInvariants(CHOICE, {
        type: "choice",
        choice: "refund",
        probabilities: { refund: 1, cancel: 0, other: 0 },
        confidence: 1.2,
      })
    ).toEqual([InvariantViolation.ConfidenceOutOfRange]);
  });

  it("accepts a well-formed score answer", () => {
    expect(
      checkAnswerInvariants(SCORE, {
        type: "score",
        score: 1.5,
        legend: { "0": "hostile", "1": "neutral", "2": "friendly" },
        probabilities: { "0": 0, "1": 0.5, "2": 0.5 },
      })
    ).toEqual([]);
  });
  it("flags a legend that does not enumerate every level", () => {
    const violations = checkAnswerInvariants(SCORE, {
      type: "score",
      score: 1,
      legend: { "0": "hostile", "1": "neutral" },
      probabilities: { "0": 0, "1": 1, "2": 0 },
    });
    expect(violations).toEqual([InvariantViolation.LegendMismatch]);
  });
  it("flags a score that is not the probability-weighted level", () => {
    const violations = checkAnswerInvariants(SCORE, {
      type: "score",
      score: 2,
      legend: { "0": "hostile", "1": "neutral", "2": "friendly" },
      probabilities: { "0": 0, "1": 1, "2": 0 },
    });
    expect(violations).toEqual([InvariantViolation.ScoreNotWeightedMean]);
  });
  it("flags a score outside the level range", () => {
    const violations = checkAnswerInvariants(SCORE, {
      type: "score",
      score: 3,
      legend: { "0": "hostile", "1": "neutral", "2": "friendly" },
      probabilities: { "0": 0, "1": 0, "2": 1 },
    });
    expect(violations).toContain(InvariantViolation.ScoreOutOfRange);
    expect(violations).toContain(InvariantViolation.ScoreNotWeightedMean);
  });
});

describe("checkResponseInvariants", () => {
  const questions = { q1: NOUL, q2: CHOICE };
  it("maps each question id to its violations and reports missing and extra answers", () => {
    const response: DecisionsResponse = {
      model: "typesafe/jev-latest",
      answers: {
        q1: { type: "noul", noul: 0.2 },
        q3: { type: "noul", noul: 0.9 },
      },
    };
    expect(checkResponseInvariants(questions, response)).toEqual({
      q1: [],
      q2: [InvariantViolation.MissingAnswer],
      q3: [InvariantViolation.ExtraAnswer],
    });
  });
  it("returns an empty violation list per question for a conforming response", () => {
    const response: DecisionsResponse = {
      model: "typesafe/jev-latest",
      answers: {
        q1: { type: "noul", noul: 0.2 },
        q2: {
          type: "choice",
          choice: "cancel",
          probabilities: { refund: 0.1, cancel: 0.8, other: 0.1 },
        },
      },
    };
    expect(checkResponseInvariants(questions, response)).toEqual({
      q1: [],
      q2: [],
    });
  });
});
