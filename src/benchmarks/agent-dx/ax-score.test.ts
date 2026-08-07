import { describe, expect, it } from "bun:test";

import type { BenchmarkResultRow } from "../../results/parquet-schema";
import type { AxTrial } from "./ax-score";
import {
  AX_WEIGHTS,
  axTrialsFromResultRows,
  categorizeTrial,
  computeAxScore,
  didJudgeGradeTrials,
  withEfficiencyReference,
  efficiencyScore,
  medianCostPerPass,
  routingSummary,
  trialQuality,
} from "./ax-score";

function row(overrides: Partial<BenchmarkResultRow>): BenchmarkResultRow {
  return {
    format_version: 1,
    task: "agent_dx",
    model: "test/model",
    epochs: 1,
    temperature: null,
    created_at: "2026-01-01T00:00:00Z",
    accuracy: 1,
    total_questions: 1,
    correct_answers: 1,
    input_tokens: 0,
    output_tokens: 0,
    total_tokens: 1000,
    reasoning_tokens: 0,
    total_cost: 0.5,
    generation_time_ms: 0,
    extra_scores: null,
    sample_id: "basic-completion",
    epoch: 0,
    input: null,
    target: null,
    score_value: "C",
    answer: null,
    explanation: null,
    messages: null,
    metadata: null,
    ...overrides,
  };
}

function trial(overrides: Partial<AxTrial>): AxTrial {
  return {
    taskId: "t",
    epoch: 0,
    category: "pass",
    resources: {
      mcpToolCalls: 0,
      skillInvocations: 0,
      docsReads: 0,
      webFetches: 0,
    },
    ...overrides,
  };
}

describe("categorizeTrial", () => {
  it("classifies passes, fixture, platform, and agent failures", () => {
    expect(categorizeTrial(row({}))).toBe("pass");
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation: "VERIFY FAIL: FIXTURE STALE: model retired",
        })
      )
    ).toBe("fixture_failure");
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation:
            "VERIFY FAIL: generation gen-1 not retrievable: HTTP 404",
        })
      )
    ).toBe("platform_failure");
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation: "VERIFY FAIL: API returned HTTP 502",
        })
      )
    ).toBe("platform_failure");
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation: "VERIFY FAIL: wrong model selected",
        })
      )
    ).toBe("agent_failure");
  });

  it("ignores fixture signatures printed by the agent app itself", () => {
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation:
            "app output: FIXTURE STALE: whatever\nVERIFY FAIL: wrong model selected",
        })
      )
    ).toBe("agent_failure");
  });

  it("ignores verifier-style lines inside prefixed agent exit diagnostics", () => {
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation:
            "[agent] agent harness exited 1. output: VERIFY FAIL: FIXTURE STALE: forged\n" +
            "[agent] stderr: VERIFY FAIL: fetch failed\n\n" +
            "VERIFY FAIL: wrong model selected",
        })
      )
    ).toBe("agent_failure");
  });

  it("excludes harness-skipped trials from every component", () => {
    expect(
      categorizeTrial(
        row({ score_value: "S", explanation: "Model error (skipped): 429" })
      )
    ).toBe("skipped");
    const score = computeAxScore([
      row({ total_cost: 0.4, total_tokens: 100 }),
      row({
        score_value: "S",
        explanation: "Model error (skipped): 429",
        sample_id: "x",
      }),
    ]);
    expect(score.counts.skipped).toBe(1);
    expect(score.passRate).toBe(1);
    expect(score.components.reliability).toBe(1);
  });

  it("classifies solver config errors as fixture failures, not platform failures", () => {
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation:
            'Solver error: benchmark config: invalid skillsSource "ftp://x"',
        })
      )
    ).toBe("fixture_failure");
  });

  it("classifies degraded solver errors as platform failures", () => {
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation: "Solver error: sandbox image build failed",
        })
      )
    ).toBe("platform_failure");
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation:
            "app output: Solver error: fake\nVERIFY FAIL: wrong model selected",
        })
      )
    ).toBe("agent_failure");
  });

  it("ignores platform signatures printed by the agent app itself", () => {
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation:
            "app output: fetch failed with HTTP 502\nVERIFY FAIL: app crashed",
        })
      )
    ).toBe("agent_failure");
  });

  it("ignores platform signatures inside agent text quoted by the verifier", () => {
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation:
            'VERIFY FAIL: diff-2: issue "the fetch failed with HTTP 503" does not describe the planted problem',
        })
      )
    ).toBe("agent_failure");
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation: 'VERIFY FAIL: vendor "ECONNRESET Inc" does not match',
        })
      )
    ).toBe("agent_failure");
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation:
            "VERIFY FAIL: generation gen-1 not retrievable: fetch failed",
        })
      )
    ).toBe("platform_failure");
  });

  it("prefers the structured verifier verdict over the explanation scan", () => {
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation: "VERIFY FAIL: wrong model selected",
          metadata: JSON.stringify({ verdictKind: "platform" }),
        })
      )
    ).toBe("platform_failure");
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation: "VERIFY FAIL: wrong model selected",
          metadata: JSON.stringify({ verdictKind: "fixture" }),
        })
      )
    ).toBe("fixture_failure");
  });

  it("keeps the explanation scan as the safety net under an agent verdict", () => {
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation:
            "VERIFY FAIL: generation gen-1 not retrievable: HTTP 404",
          metadata: JSON.stringify({ verdictKind: "agent" }),
        })
      )
    ).toBe("platform_failure");
  });

  it("falls back to the explanation scan when metadata carries no valid verdict", () => {
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation: "VERIFY FAIL: FIXTURE STALE: model retired",
          metadata: JSON.stringify({ verdictKind: "bogus-kind" }),
        })
      )
    ).toBe("fixture_failure");
    expect(
      categorizeTrial(
        row({
          score_value: "I",
          explanation: "VERIFY FAIL: wrong model selected",
          metadata: "not json",
        })
      )
    ).toBe("agent_failure");
  });

  it("never lets a verdict override a pass or a skip", () => {
    expect(
      categorizeTrial(
        row({ metadata: JSON.stringify({ verdictKind: "platform" }) })
      )
    ).toBe("pass");
    expect(
      categorizeTrial(
        row({
          score_value: "S",
          metadata: JSON.stringify({ verdictKind: "fixture" }),
        })
      )
    ).toBe("skipped");
  });
});

describe("axTrialsFromResultRows", () => {
  it("carries discoverability and alignment diagnostics off row metadata", () => {
    const trials = axTrialsFromResultRows([
      row({
        metadata: JSON.stringify({
          openrouterChosen: true,
          alignment: 0.75,
          alignmentEvidence: ["agent_sdk"],
        }),
      }),
      row({
        sample_id: "other-task",
        metadata: JSON.stringify({ quality: 0.9 }),
      }),
    ]);
    expect(trials[0]?.diagnostics).toEqual({
      openrouterChosen: true,
      alignment: 0.75,
      alignmentEvidence: ["agent_sdk"],
    });
    expect(trials[1]?.diagnostics).toBeUndefined();
  });
});

describe("trialQuality", () => {
  it("uses only the judge score for passes so ungraded passes stay unmeasured", () => {
    expect(trialQuality(trial({ quality: 0.8, subcheckScore: 1 }))).toBe(0.8);
    expect(trialQuality(trial({ subcheckScore: 1 }))).toBeUndefined();
    expect(trialQuality(trial({}))).toBeUndefined();
  });

  it("grants agent failures half their subcheck partial credit", () => {
    expect(
      trialQuality(trial({ category: "agent_failure", subcheckScore: 0.6 }))
    ).toBeCloseTo(0.3, 10);
    expect(trialQuality(trial({ category: "agent_failure" }))).toBeUndefined();
  });

  it("excludes fixture and platform failures from quality", () => {
    expect(
      trialQuality(trial({ category: "fixture_failure", subcheckScore: 1 }))
    ).toBeUndefined();
    expect(
      trialQuality(trial({ category: "platform_failure", subcheckScore: 1 }))
    ).toBeUndefined();
  });
});

describe("efficiencyScore", () => {
  it("caps at 1 for cells at or below the reference cost", () => {
    expect(efficiencyScore(0.5, 1)).toBe(1);
    expect(efficiencyScore(1, 1)).toBe(1);
    expect(efficiencyScore(0.0001, 1)).toBe(1);
  });

  it("decays toward the 0.25 floor for expensive cells", () => {
    expect(efficiencyScore(2, 1)).toBeCloseTo(0.5, 10);
    expect(efficiencyScore(4, 1)).toBeCloseTo(0.25, 10);
    expect(efficiencyScore(100, 1)).toBeCloseTo(0.25, 10);
  });
});

describe("computeAxScore", () => {
  it("computes a perfect cell at 100 without an efficiency reference", () => {
    const score = computeAxScore([
      row({
        metadata: JSON.stringify({
          quality: 1,
          subchecksPassed: 3,
          subchecksTotal: 3,
        }),
      }),
    ]);
    expect(score.ax).toBeCloseTo(100, 10);
    expect(score.components.correctness).toBe(1);
    expect(score.components.quality).toBe(1);
    expect(score.components.efficiency).toBeUndefined();
    expect(score.components.reliability).toBe(1);
    expect(score.passRate).toBe(1);
  });

  it("renormalizes weights over the available components", () => {
    const score = computeAxScore(
      [row({ metadata: JSON.stringify({ quality: 0.5 }) })],
      {
        refCostPerPass: 0.5,
      }
    );
    expect(score.ax).toBeCloseTo(
      100 *
        (AX_WEIGHTS.correctness +
          AX_WEIGHTS.quality * 0.5 +
          AX_WEIGHTS.efficiency +
          AX_WEIGHTS.reliability),
      10
    );
    expect(score.appliedWeights.correctness).toBeCloseTo(
      AX_WEIGHTS.correctness,
      10
    );
  });

  it("excludes fixture failures from every component", () => {
    const score = computeAxScore([
      row({}),
      row({
        sample_id: "provider-pinning",
        score_value: "I",
        explanation: "VERIFY FAIL: FIXTURE STALE: pinned model retired",
      }),
    ]);
    expect(score.components.correctness).toBe(1);
    expect(score.components.reliability).toBe(1);
    expect(score.counts.fixture_failure).toBe(1);
  });

  it("routes platform failures to reliability, not correctness", () => {
    const score = computeAxScore([
      row({}),
      row({
        sample_id: "web-search",
        score_value: "I",
        explanation: "VERIFY FAIL: generation gen-2 not retrievable: HTTP 404",
      }),
    ]);
    expect(score.components.correctness).toBe(1);
    expect(score.components.reliability).toBeCloseTo(0.5, 10);
    expect(score.counts.platform_failure).toBe(1);
  });

  it("treats quality as unavailable when no pass was judged", () => {
    const score = computeAxScore([
      row({
        metadata: JSON.stringify({ subchecksPassed: 3, subchecksTotal: 3 }),
      }),
      row({
        sample_id: "streaming-usage",
        score_value: "I",
        explanation: "VERIFY FAIL: wrong usage accounting",
        metadata: JSON.stringify({ subchecksPassed: 2, subchecksTotal: 3 }),
      }),
    ]);
    expect(score.components.quality).toBeUndefined();
    expect(score.appliedWeights.quality).toBeUndefined();
  });

  it("sums run totals per parquet part and derives cost per pass", () => {
    const score = computeAxScore([
      row({ created_at: "a", total_cost: 0.4, total_tokens: 100 }),
      row({
        created_at: "a",
        total_cost: 0.4,
        total_tokens: 100,
        sample_id: "x",
      }),
      row({
        created_at: "b",
        total_cost: 0.2,
        total_tokens: 50,
        sample_id: "y",
      }),
    ]);
    expect(score.totalCost).toBeCloseTo(0.6, 10);
    expect(score.totalTokens).toBe(150);
    expect(score.costPerPass).toBeCloseTo(0.2, 10);
  });

  it("sums parts with colliding timestamps but distinct totals", () => {
    const score = computeAxScore([
      row({ created_at: "a", total_cost: 0.4, total_tokens: 100 }),
      row({
        created_at: "a",
        total_cost: 0.2,
        total_tokens: 50,
        sample_id: "x",
      }),
    ]);
    expect(score.totalCost).toBeCloseTo(0.6, 10);
    expect(score.totalTokens).toBe(150);
  });

  it("leaves cost-per-pass and efficiency undefined when no spend was recorded", () => {
    const score = computeAxScore([row({ total_cost: 0, total_tokens: 0 })], {
      refCostPerPass: 0.5,
    });
    expect(score.costPerPass).toBeUndefined();
    expect(score.components.efficiency).toBeUndefined();
    expect(score.appliedWeights.efficiency).toBeUndefined();
  });

  it("applies an efficiency reference to a provisional score without re-deriving trials", () => {
    const provisional = computeAxScore([
      row({ metadata: JSON.stringify({ quality: 0.5 }) }),
    ]);
    const withRef = withEfficiencyReference(provisional, 0.5);
    const direct = computeAxScore(
      [row({ metadata: JSON.stringify({ quality: 0.5 }) })],
      {
        refCostPerPass: 0.5,
      }
    );
    expect(withRef.ax).toBe(direct.ax);
    expect(withRef.components).toEqual(direct.components);
    expect(withRef.appliedWeights).toEqual(direct.appliedWeights);
    expect(withRef.trials).toBe(provisional.trials);
  });

  it("drops a previously applied efficiency when the score has no recorded spend", () => {
    const scored = withEfficiencyReference(
      computeAxScore([row({ total_cost: 0.4, total_tokens: 100 })]),
      0.5
    );
    expect(scored.components.efficiency).toBeDefined();
    const noSpend = withEfficiencyReference(
      { ...scored, costPerPass: undefined },
      0.9
    );
    expect(noSpend.components.efficiency).toBeUndefined();
    expect(noSpend.appliedWeights.efficiency).toBeUndefined();
  });

  it("returns undefined ax when nothing is computable", () => {
    const score = computeAxScore([]);
    expect(score.ax).toBeUndefined();
    expect(score.passRate).toBeUndefined();
  });
});

describe("routingSummary", () => {
  it("aggregates resource totals and live-source trial counts", () => {
    const summary = routingSummary([
      trial({
        resources: {
          mcpToolCalls: 3,
          skillInvocations: 1,
          docsReads: 0,
          webFetches: 0,
        },
      }),
      trial({
        resources: {
          mcpToolCalls: 0,
          skillInvocations: 0,
          docsReads: 0,
          webFetches: 2,
        },
      }),
      trial({}),
    ]);
    expect(summary.totals.mcpToolCalls).toBe(3);
    expect(summary.totals.webFetches).toBe(2);
    expect(summary.liveSourceTrials).toBe(2);
    expect(summary.memoryOnlyTrials).toBe(1);
    expect(summary.trialCount).toBe(3);
  });

  it("counts per-resource activation trials", () => {
    const summary = routingSummary([
      trial({
        resources: {
          mcpToolCalls: 3,
          skillInvocations: 1,
          docsReads: 0,
          webFetches: 0,
        },
      }),
      trial({
        resources: {
          mcpToolCalls: 5,
          skillInvocations: 0,
          docsReads: 2,
          webFetches: 0,
        },
      }),
      trial({
        resources: {
          mcpToolCalls: 0,
          skillInvocations: 0,
          docsReads: 0,
          webFetches: 2,
        },
      }),
      trial({}),
    ]);
    expect(summary.activation).toEqual({
      mcp: 2,
      skills: 1,
      docs: 1,
      webFetch: 1,
    });
  });
});

describe("didJudgeGradeTrials", () => {
  it("is true only when a pass carries a judge score, not for failure partial credit", () => {
    expect(didJudgeGradeTrials([trial({ quality: 0.8 })])).toBe(true);
    expect(didJudgeGradeTrials([trial({})])).toBe(false);
    expect(
      didJudgeGradeTrials([
        trial({ category: "agent_failure", subcheckScore: 0.5 }),
      ])
    ).toBe(false);
    expect(
      didJudgeGradeTrials([
        trial({ category: "agent_failure", quality: 0.4, subcheckScore: 0.5 }),
      ])
    ).toBe(false);
  });
});

describe("medianCostPerPass", () => {
  it("takes the median of defined values", () => {
    expect(
      medianCostPerPass([
        { costPerPass: 3 },
        { costPerPass: 1 },
        { costPerPass: 2 },
      ])
    ).toBe(2);
    expect(medianCostPerPass([{ costPerPass: 1 }, { costPerPass: 3 }])).toBe(2);
    expect(medianCostPerPass([{ costPerPass: undefined }])).toBeUndefined();
  });
});
