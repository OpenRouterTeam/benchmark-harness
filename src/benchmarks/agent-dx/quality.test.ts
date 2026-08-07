import { describe, expect, it } from "bun:test";

import {
  alignmentCriterionId,
  buildJudgePrompt,
  buildJudgeResponseFormat,
  judgeTextFromResponse,
  parseJudgeVerdict,
  parseSubcheckSummary,
  QUALITY_CRITERIA,
} from "./quality";

describe("parseSubcheckSummary", () => {
  it("counts passes and totals from verifier SUBCHECK lines", () => {
    const output = [
      "starting app",
      "SUBCHECK project_present=pass",
      "SUBCHECK app_ran=pass",
      "SUBCHECK verified=fail",
      "VERIFY FAIL: generation not retrievable",
    ].join("\n");

    expect(parseSubcheckSummary(output)).toEqual({ passed: 2, total: 3 });
  });

  it("returns undefined when the verifier emitted no subchecks", () => {
    expect(parseSubcheckSummary("VERIFY PASS")).toBeUndefined();
  });

  it("ignores lines that merely mention SUBCHECK", () => {
    expect(
      parseSubcheckSummary("the verifier prints SUBCHECK name=pass lines")
    ).toBeUndefined();
  });

  it("ignores app output prefixed by the verifier, so apps cannot mint credit", () => {
    const output = [
      "SUBCHECK project_present=pass",
      "npm start failed:",
      "[app] SUBCHECK verified=pass",
      "[app] VERIFY FAIL: fetch failed with HTTP 502",
    ].join("\n");

    expect(parseSubcheckSummary(output)).toEqual({ passed: 1, total: 1 });
  });
});

describe("parseJudgeVerdict", () => {
  it("averages criterion scores into a [0,1] quality value", () => {
    const verdict = parseJudgeVerdict(
      JSON.stringify({
        criteria: [
          {
            id: "current_models",
            score: 2,
            reason: "queried the live catalog",
          },
          {
            id: "api_usage",
            score: 1,
            reason: "streaming but no usage accounting",
          },
          { id: "robustness", score: 1, reason: "checks status only" },
          { id: "code_clarity", score: 2, reason: "single small file" },
        ],
      })
    );

    expect(verdict?.quality).toBeCloseTo(0.75, 5);
    expect(verdict?.criteria).toHaveLength(4);
  });

  it("scores an all-zero verdict as 0 rather than dropping it", () => {
    const verdict = parseJudgeVerdict(
      JSON.stringify({
        criteria: QUALITY_CRITERIA.map((c) => ({
          id: c.id,
          score: 0,
          reason: "absent",
        })),
      })
    );

    expect(verdict?.quality).toBe(0);
  });

  it("rejects verdicts that omit rubric criteria instead of inflating the mean", () => {
    expect(
      parseJudgeVerdict(
        JSON.stringify({
          criteria: [
            {
              id: "current_models",
              score: 2,
              reason: "queried the live catalog",
            },
          ],
        })
      )
    ).toBeUndefined();
  });

  it("rejects verdicts with duplicate or unknown criterion ids", () => {
    expect(
      parseJudgeVerdict(
        JSON.stringify({
          criteria: QUALITY_CRITERIA.map(() => ({
            id: "api_usage",
            score: 2,
            reason: "dup",
          })),
        })
      )
    ).toBeUndefined();
    expect(
      parseJudgeVerdict(
        JSON.stringify({
          criteria: QUALITY_CRITERIA.map((c, i) => ({
            id: i === 0 ? "made_up" : c.id,
            score: 2,
            reason: "x",
          })),
        })
      )
    ).toBeUndefined();
  });

  it("reads the verdict out of surrounding prose and code fences", () => {
    const full = JSON.stringify({
      criteria: QUALITY_CRITERIA.map((c) => ({
        id: c.id,
        score: 2,
        reason: "solid",
      })),
    });
    const verdict = parseJudgeVerdict(
      `Here is my grade:\n\`\`\`json\n${full}\n\`\`\`\nDone.`
    );

    expect(verdict?.quality).toBe(1);
  });

  it("returns undefined for non-JSON and for the wrong verdict shape", () => {
    expect(parseJudgeVerdict("the code looks fine to me")).toBeUndefined();
    expect(parseJudgeVerdict('{"criteria":[]}')).toBeUndefined();
    expect(
      parseJudgeVerdict('{"criteria":[{"id":"api_usage","score":"good"}]}')
    ).toBeUndefined();
  });

  it("rejects out-of-range scores instead of inflating quality above 1", () => {
    expect(
      parseJudgeVerdict(
        '{"criteria":[{"id":"api_usage","score":9,"reason":"great"}]}'
      )
    ).toBeUndefined();
  });

  it("averages alignment verdicts over the declared criteria", () => {
    const alignmentCriteria = [
      "uses the @openrouter/agent SDK",
      "routes through a preset",
    ];
    const verdict = parseJudgeVerdict(
      JSON.stringify({
        criteria: QUALITY_CRITERIA.map((c) => ({
          id: c.id,
          score: 2,
          reason: "solid",
        })),
        alignment: [
          { id: alignmentCriterionId(0), score: 2, reason: "built on the SDK" },
          {
            id: alignmentCriterionId(1),
            score: 1,
            reason: "preset created but bypassed",
          },
        ],
      }),
      alignmentCriteria
    );

    expect(verdict?.quality).toBe(1);
    expect(verdict?.alignment).toBeCloseTo(0.75, 5);
    expect(verdict?.alignmentCriteria).toHaveLength(2);
  });

  it("degrades incomplete alignment verdicts to undefined without failing quality", () => {
    const alignmentCriteria = [
      "uses the @openrouter/agent SDK",
      "routes through a preset",
    ];
    const verdict = parseJudgeVerdict(
      JSON.stringify({
        criteria: QUALITY_CRITERIA.map((c) => ({
          id: c.id,
          score: 2,
          reason: "solid",
        })),
        alignment: [
          { id: alignmentCriterionId(0), score: 2, reason: "built on the SDK" },
        ],
      }),
      alignmentCriteria
    );

    expect(verdict?.quality).toBe(1);
    expect(verdict?.alignment).toBeUndefined();
  });

  it("ignores an unsolicited alignment section when the task declared no criteria", () => {
    const verdict = parseJudgeVerdict(
      JSON.stringify({
        criteria: QUALITY_CRITERIA.map((c) => ({
          id: c.id,
          score: 2,
          reason: "solid",
        })),
        alignment: [
          { id: alignmentCriterionId(0), score: 2, reason: "made up" },
        ],
      })
    );

    expect(verdict?.quality).toBe(1);
    expect(verdict?.alignment).toBeUndefined();
  });
});

describe("buildJudgePrompt", () => {
  it("includes the task, the workspace, and every rubric criterion", () => {
    const prompt = buildJudgePrompt(
      "Build a streaming chat app",
      "=== /app/index.ts ===\nfetch()"
    );

    expect(prompt).toContain("Build a streaming chat app");
    expect(prompt).toContain("=== /app/index.ts ===");
    for (const criterion of QUALITY_CRITERIA) {
      expect(prompt).toContain(criterion.id);
    }
    expect(prompt).not.toContain("primitive-alignment");
  });

  it("adds the alignment rubric only when the task declares criteria", () => {
    const prompt = buildJudgePrompt("Build an agent", "=== /app/index.ts ===", [
      "uses the @openrouter/agent SDK",
    ]);

    expect(prompt).toContain("primitive-alignment");
    expect(prompt).toContain(alignmentCriterionId(0));
    expect(prompt).toContain("uses the @openrouter/agent SDK");
  });
});

describe("buildJudgeResponseFormat", () => {
  it("constrains the response to a strict verdict schema without alignment", () => {
    const format = buildJudgeResponseFormat();
    expect(format).toMatchObject({
      type: "json_schema",
      json_schema: {
        strict: true,
        schema: { required: ["criteria"], additionalProperties: false },
      },
    });
  });

  it("requires the alignment array when the task declares alignment criteria", () => {
    const format = buildJudgeResponseFormat(["built on the agent SDK"]);
    expect(format).toMatchObject({
      json_schema: { schema: { required: ["criteria", "alignment"] } },
    });
  });
});

describe("judgeTextFromResponse", () => {
  it("reads the assistant content out of a chat-completions body", () => {
    const body = {
      choices: [{ message: { role: "assistant", content: '{"criteria":[]}' } }],
    };

    expect(judgeTextFromResponse(body)).toBe('{"criteria":[]}');
  });

  it("returns undefined for an error body or a missing choice", () => {
    expect(
      judgeTextFromResponse({ error: { message: "rate limited" } })
    ).toBeUndefined();
    expect(judgeTextFromResponse({ choices: [] })).toBeUndefined();
  });
});
