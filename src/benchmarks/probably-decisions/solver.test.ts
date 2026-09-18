import { describe, expect, test } from "bun:test";

import { provide, runPromise, succeed } from "effect/Effect";
import { mergeAll } from "effect/Layer";

import {
  noopCheckpointLayer,
  noopProgressLayer,
} from "../../../test/helpers/noop-progress-layer";
import type { ModelMessage, ModelOutput } from "../../harness/core";
import { MessageRole, initialTaskState } from "../../harness/core";
import type { GenerateConfig, ModelService } from "../../harness/model";
import type {
  DecisionsChoiceRequest,
  DecisionsService,
} from "../../providers/decisions-client";
import { decisionRecordToSample } from "./dataset";
import {
  chatJudge,
  decisionsJudge,
  judgePrompt,
  parseJudgeCompletion,
} from "./judge";
import { ProbablyRunMetaSchema } from "./schema";
import { makeDecisionSolver } from "./solver";

const INFERENCE: GenerateConfig = { temperature: 0, reasoningEffort: "none" };

const RUN_LAYER = mergeAll(noopProgressLayer, noopCheckpointLayer);

const LEAKED_RECORD = {
  id: "sentinel-aaaaaaaaaaaa",
  domain: "trust_and_safety",
  source: "sentinel_ban_candidates",
  dossier:
    "One long-lived funded account whose key started serving a new client fingerprint from a new ASN overnight, with spend ten times its historical daily rate.",
  evidence: {
    keys: { "t0.key_age_days": 210, "t0.new_client_fingerprints_24h": 1 },
    funding: { "t0.lifetime_funding_usd_bucket": "500-1000" },
  },
  gold: { outcome: "key_revocation", human_decision: "approved" },
} as const;

function output(message: ModelMessage): ModelOutput {
  return { completion: message.content, message, generationTimeMs: 1 };
}

function keywordJudge(hit: string): ModelService {
  return {
    generate: (messages) => {
      const prompt = messages.at(-1)?.content ?? "";
      const labels = prompt
        .split("Descriptions:\n")[1]
        ?.split("\n")
        .filter((line) => /^[A-H]\. /.test(line));
      const count = labels?.length ?? 2;
      const chosen = prompt.includes(hit) ? 0 : count - 1;
      const dist = Object.fromEntries(
        Array.from({ length: count }, (_, i) => [
          "ABCDEFGH"[i] ?? String(i),
          i === chosen ? 0.97 : 0.03 / (count - 1),
        ])
      );
      return succeed(
        output({ role: MessageRole.Assistant, content: JSON.stringify(dist) })
      );
    },
  };
}

describe("probably-decisions judge adapter", () => {
  test("judgePrompt letters every label", () => {
    const prompt = judgePrompt("dossier text", ["first", "second"]);
    expect(prompt).toContain("A. first");
    expect(prompt).toContain("B. second");
  });

  test("parseJudgeCompletion normalises letters, names and fenced JSON", () => {
    expect(parseJudgeCompletion('{"A": 3, "B": 1}', ["x", "y"])).toEqual({
      x: 0.75,
      y: 0.25,
    });
    expect(
      parseJudgeCompletion('```json\n{"x": "0.2", "y": 0.8}\n```', ["x", "y"])
    ).toEqual({ x: 0.2, y: 0.8 });
    expect(parseJudgeCompletion('{"A": 1}', ["x", "y"])).toBeUndefined();
    expect(parseJudgeCompletion("no json", ["x"])).toBeUndefined();
  });

  test("decisionsJudge sends lettered criteria and maps probabilities back to labels", async () => {
    const requests: DecisionsChoiceRequest[] = [];
    const decisions: DecisionsService = {
      choose: (request) => {
        requests.push(request);
        return succeed({
          probabilities: { A: 0.25, B: 0.75 },
          usage: {
            inputTokens: 10,
            outputTokens: 2,
            totalTokens: 12,
            reasoningTokens: 0,
            totalCost: 0.001,
          },
          generationTimeMs: 5,
        });
      },
    };
    const judge = decisionsJudge(decisions, "~typesafe/jev-latest", INFERENCE);
    const call = await runPromise(judge("salt", "dossier text", ["x", "y"]));
    expect(requests).toHaveLength(1);
    expect(requests[0]?.model).toBe("~typesafe/jev-latest");
    expect(requests[0]?.state).toEqual({ dossier: "dossier text" });
    expect(requests[0]?.criteria).toEqual({ A: "x", B: "y" });
    expect(call.probabilities).toEqual({ x: 0.25, y: 0.75 });
    expect(call.usage?.totalCost).toBe(0.001);
    expect(call.completion).toBe('{"A":0.25,"B":0.75}');
  });

  test("decisionsJudge drives the judgment program end to end", async () => {
    const decisions: DecisionsService = {
      choose: (request) => {
        const keys = Object.keys(request.criteria);
        const chosen = JSON.stringify(request.state).includes(
          "new client fingerprint"
        )
          ? 0
          : keys.length - 1;
        return succeed({
          probabilities: Object.fromEntries(
            keys.map((key, i) => [
              key,
              i === chosen ? 0.97 : 0.03 / (keys.length - 1),
            ])
          ),
          usage: {
            inputTokens: 1,
            outputTokens: 1,
            totalTokens: 2,
            reasoningTokens: 0,
            totalCost: 0,
          },
          generationTimeMs: 1,
        });
      },
    };
    const unused: ModelService = {
      generate: () =>
        succeed(output({ role: MessageRole.Assistant, content: "" })),
    };
    const solver = makeDecisionSolver(unused, {
      judge: decisionsJudge(decisions, "~typesafe/jev-latest", INFERENCE),
      mode: "judgment",
      program: "sentinel_case_v1",
      maxResearchSteps: 4,
      inference: INFERENCE,
    });
    const state = await runPromise(
      solver(initialTaskState(decisionRecordToSample(LEAKED_RECORD), 0)).pipe(
        provide(RUN_LAYER)
      )
    );
    const run = ProbablyRunMetaSchema.parse(
      state.sample.metadata?.["probablyRun"]
    );
    expect(state.completed).toBe(true);
    expect(run.failure).toBeNull();
    expect(run.judges.length).toBeGreaterThanOrEqual(1);
    expect(state.output?.completion).toBe("key_revocation");
  });
});

describe("probably-decisions solver", () => {
  test("judgment mode runs the program against the dossier and records judge traces", async () => {
    const judge = keywordJudge("new client fingerprint");
    const solver = makeDecisionSolver(judge, {
      judge: chatJudge(judge, INFERENCE),
      mode: "judgment",
      program: "sentinel_case_v1",
      maxResearchSteps: 4,
      inference: INFERENCE,
    });
    const state = await runPromise(
      solver(initialTaskState(decisionRecordToSample(LEAKED_RECORD), 0)).pipe(
        provide(RUN_LAYER)
      )
    );
    const run = ProbablyRunMetaSchema.parse(
      state.sample.metadata?.["probablyRun"]
    );
    expect(state.completed).toBe(true);
    expect(run.mode).toBe("judgment");
    expect(run.action).toBe("key_revocation");
    expect(run.failure).toBeNull();
    expect(run.judges.length).toBeGreaterThanOrEqual(1);
    expect(run.judges[0]?.probabilities).toBeDefined();
    expect(state.output?.completion).toBe("key_revocation");
    const judgeMessages = state.messages.filter(
      (m) => m.role === MessageRole.User && m.content.includes("Descriptions:")
    );
    expect(judgeMessages).toHaveLength(run.judges.length);
  });

  test("research mode drives read-only tools, then judges the submitted dossier", async () => {
    const calls: string[] = [];
    const researcher: ModelService = {
      generate: (messages) => {
        const step = messages.filter((m) => m.role === MessageRole.Tool).length;
        const call = (name: string, args: Record<string, unknown>) =>
          output({
            role: MessageRole.Assistant,
            content: "",
            toolCalls: [
              {
                id: `call-${step}`,
                type: "function",
                function: { name, arguments: JSON.stringify(args) },
              },
            ],
          });
        calls.push(String(step));
        switch (step) {
          case 0: {
            return succeed(call("list_evidence_sections", {}));
          }
          case 1: {
            return succeed(call("read_evidence_section", { section: "keys" }));
          }
          default: {
            return succeed(
              call("submit_dossier", {
                dossier:
                  "The key is 210 days old and began serving a new client fingerprint from a new ASN overnight while spend jumped tenfold.",
              })
            );
          }
        }
      },
    };
    const solver = makeDecisionSolver(researcher, {
      judge: chatJudge(keywordJudge("new client fingerprint"), INFERENCE),
      mode: "research",
      program: "sentinel_case_v1",
      maxResearchSteps: 6,
      inference: INFERENCE,
    });
    const state = await runPromise(
      solver(initialTaskState(decisionRecordToSample(LEAKED_RECORD), 0)).pipe(
        provide(RUN_LAYER)
      )
    );
    const run = ProbablyRunMetaSchema.parse(
      state.sample.metadata?.["probablyRun"]
    );
    expect(calls).toEqual(["0", "1", "2"]);
    expect(run.mode).toBe("research");
    expect(run.sectionsAvailable).toEqual(["funding", "keys"]);
    expect(run.sectionsRead).toEqual(["keys"]);
    expect(run.researchSteps).toBe(3);
    expect(run.researchDossier).toContain("210 days");
    expect(run.action).toBe("key_revocation");
    expect(
      state.messages.filter((m) => m.role === MessageRole.Tool)
    ).toHaveLength(3);
  });

  test("research mode records a failure when no dossier is submitted", async () => {
    const silent: ModelService = {
      generate: () =>
        succeed(output({ role: MessageRole.Assistant, content: "thinking" })),
    };
    const solver = makeDecisionSolver(silent, {
      judge: chatJudge(silent, INFERENCE),
      mode: "research",
      program: "sentinel_case_v1",
      maxResearchSteps: 2,
      inference: INFERENCE,
    });
    const state = await runPromise(
      solver(initialTaskState(decisionRecordToSample(LEAKED_RECORD), 0)).pipe(
        provide(RUN_LAYER)
      )
    );
    const run = ProbablyRunMetaSchema.parse(
      state.sample.metadata?.["probablyRun"]
    );
    expect(run.action).toBeNull();
    expect(run.judges).toEqual([]);
    expect(run.failure).toMatch(/No dossier submitted/);
  });
});
