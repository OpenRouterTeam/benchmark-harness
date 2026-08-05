import { describe, expect, it } from "bun:test";

import { extractMcqAnswer, normalizeMcqAnswer, stripMdLatex } from "./extract";

const OPENBENCH_FIXTURES: readonly {
  text: string;
  expected: string | null;
}[] = [
  { text: "Answer: C", expected: "C" },
  { text: "Answer: (B)", expected: "B" },
  { text: "The answer is **A**", expected: "A" },
  { text: "Answer:\nD", expected: "D" },
  { text: "I think Option B is correct", expected: "B" },
  { text: "Final reasoning... \\boxed{A}", expected: "A" },
  { text: "answer: d", expected: "D" },
  { text: "Choice: C", expected: "C" },
  { text: "(A)", expected: "A" },
  { text: "**D) the moon**", expected: "D" },
  { text: "B.", expected: "B" },
  { text: "blah blah\nAnswer: B\nmore", expected: "B" },
  { text: "no letter here", expected: null },
  { text: "", expected: null },
  { text: "Answer - C", expected: "C" },
  { text: "Answers: A", expected: "S" },
  { text: "The correct answer is Answer: B because", expected: "B" },
  { text: "**Answer:** A", expected: "A" },
  { text: "Reasoning\n\nAnswer: C\n", expected: "C" },
];
describe("extractMcqAnswer (openbench parity)", () => {
  for (const { text, expected } of OPENBENCH_FIXTURES) {
    it(`extracts ${JSON.stringify(expected)} from ${JSON.stringify(text)}`, () => {
      expect(extractMcqAnswer(text)).toBe(expected);
    });
  }
});
describe("stripMdLatex (openbench parity)", () => {
  it("strips bold markers", () => {
    expect(stripMdLatex("**A**")).toBe("A");
  });
  it("strips boxed latex", () => {
    expect(stripMdLatex("$\\boxed{C}$")).toBe("C");
  });
});
describe("normalizeMcqAnswer (openbench parity)", () => {
  it("maps fullwidth Japanese letters to A-D", () => {
    expect(normalizeMcqAnswer("Ｂ")).toBe("B");
  });
  it("maps fullwidth Japanese letters E-J for 10-option MCQ", () => {
    expect(normalizeMcqAnswer("Ｅ")).toBe("E");
    expect(normalizeMcqAnswer("Ｈ")).toBe("H");
    expect(normalizeMcqAnswer("Ｊ")).toBe("J");
  });
  it("trims and maps Arabic letters", () => {
    expect(normalizeMcqAnswer("ج")).toBe("C");
  });
  it("leaves Latin letters unchanged", () => {
    expect(normalizeMcqAnswer("A")).toBe("A");
  });
});
