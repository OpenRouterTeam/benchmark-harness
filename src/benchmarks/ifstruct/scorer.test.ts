import { describe, expect, it } from "bun:test";

import { runSync } from "effect/Effect";

import type { ModelOutput, Sample, TaskState } from "../../harness/core";
import { MessageRole, ScoreValue } from "../../harness/core";
import type { IfStructRequirements } from "./schema";
import { OutputFormat } from "./schema";
import { ifStructScorer } from "./scorer";

const REQUIREMENTS: IfStructRequirements = {
  jsonSchema: {
    type: "array",
    items: {
      type: "object",
      properties: { id: { type: "string" } },
      required: ["id"],
    },
    minItems: 1,
    maxItems: 1,
  },
  topLevelCount: 1,
  topLevelKey: null,
  requireWrapperKey: false,
  requireCodeBlock: false,
  requireNoCommentary: false,
  outputFormat: OutputFormat.Json,
};

function makeOutput(completion: string): ModelOutput {
  return {
    completion,
    message: { role: MessageRole.Assistant, content: completion },
  };
}

function makeState(
  completion: string,
  metadata: Sample["metadata"]
): TaskState {
  return {
    sample: {
      id: "ifstruct-1",
      input: "prompt",
      target: { text: "json" },
      metadata,
    },
    messages: [],
    output: makeOutput(completion),
    completed: true,
  };
}

const TARGET = { text: "json" };
describe("ifStructScorer", () => {
  it("scores a schema-conformant completion Correct", () => {
    const score = runSync(
      ifStructScorer(makeState('[{"id": "1"}]', { ...REQUIREMENTS }), TARGET)
    );
    expect(score.value).toBe(ScoreValue.Correct);
    expect(score.answer).toBe('[{"id": "1"}]');
    expect(score.explanation).toContain("Passed all ifstruct checks");
  });
  it("scores a schema-violating completion Incorrect and surfaces the error", () => {
    const score = runSync(
      ifStructScorer(makeState('[{"id": 1}]', { ...REQUIREMENTS }), TARGET)
    );
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.explanation).toContain("Failed ifstruct checks");
  });
  it("scores Incorrect when the sample metadata is missing requirements", () => {
    const score = runSync(
      ifStructScorer(makeState('[{"id": "1"}]', undefined), TARGET)
    );
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.explanation).toContain("requirements missing/invalid");
  });
});
