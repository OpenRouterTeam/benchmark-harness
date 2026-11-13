import { describe, expect, it } from "bun:test";

import { fail, runPromise, succeed } from "effect/Effect";

import type { TaskState } from "../../../harness/core";
import {
  initialTaskState,
  MessageRole,
  ScoreValue,
} from "../../../harness/core";
import { ClefError } from "../../../judge/clef";
import { mcqScorer } from "../mcq/scorer";
import type { ChoiceJudge } from "./integrity-rubric";
import { INTEGRITY_QUESTION, withIntegrityRubric } from "./integrity-rubric";

const TARGET = { text: "B" };

function answered(completion: string): TaskState {
  return {
    ...initialTaskState({ id: "s1", input: "Which is it?", target: TARGET }),
    output: {
      completion,
      message: { role: MessageRole.Assistant, content: completion },
    },
  };
}

function judgeReturning(good: number): {
  readonly judge: ChoiceJudge;
  readonly calls: Readonly<Record<string, string>>[];
} {
  const calls: Readonly<Record<string, string>>[] = [];
  const judge: ChoiceJudge = (state) => {
    calls.push(state);
    return succeed({
      coherent_good: good,
      coherent_okay: 0,
      coherent_bad: 1 - good,
    });
  };
  return { judge, calls };
}

const PROSE = "The ester hydrolyses under base.\n\nAnswer: B";

describe("withIntegrityRubric", () => {
  it("keeps a correct answer when the completion reads as intact", async () => {
    const { judge, calls } = judgeReturning(0.9);
    const score = await runPromise(
      withIntegrityRubric(mcqScorer, judge)(answered(PROSE), TARGET)
    );
    expect(score.value).toBe(ScoreValue.Correct);
    expect(calls).toEqual([
      { user_request: "Which is it?", completion: PROSE },
    ]);
  });

  it("marks a correct answer incorrect when the completion is flagged", async () => {
    const { judge } = judgeReturning(0.3);
    const score = await runPromise(
      withIntegrityRubric(mcqScorer, judge)(answered(PROSE), TARGET)
    );
    expect(score.value).toBe(ScoreValue.Incorrect);
    expect(score.answer).toBe("B");
    expect(score.explanation).toContain("flagged as corrupted");
  });

  it("does not flag just under the threshold", async () => {
    const { judge } = judgeReturning(0.43);
    const score = await runPromise(
      withIntegrityRubric(mcqScorer, judge)(answered(PROSE), TARGET)
    );
    expect(score.value).toBe(ScoreValue.Correct);
  });

  it("skips the judge for a bare answer line and for empty output", async () => {
    const { judge, calls } = judgeReturning(0);
    const scorer = withIntegrityRubric(mcqScorer, judge);
    const bare = await runPromise(scorer(answered("Answer: B"), TARGET));
    const empty = await runPromise(scorer(answered("  "), TARGET));
    expect(bare.value).toBe(ScoreValue.Correct);
    expect(empty.value).toBe(ScoreValue.Incorrect);
    expect(calls).toEqual([]);
  });

  it("keeps the base score when the judge fails", async () => {
    const judge: ChoiceJudge = () => fail(new ClefError("Clef HTTP 503"));
    const score = await runPromise(
      withIntegrityRubric(mcqScorer, judge)(answered(PROSE), TARGET)
    );
    expect(score.value).toBe(ScoreValue.Correct);
    expect(score.explanation).toContain(
      "integrity check skipped: Clef HTTP 503"
    );
  });

  it("asks the tuned integrity question", async () => {
    let asked: unknown;
    const judge: ChoiceJudge = (_state, question) => {
      asked = question;
      return succeed({ coherent_good: 1 });
    };
    await runPromise(
      withIntegrityRubric(mcqScorer, judge)(answered(PROSE), TARGET)
    );
    expect(asked).toBe(INTEGRITY_QUESTION);
  });
});
