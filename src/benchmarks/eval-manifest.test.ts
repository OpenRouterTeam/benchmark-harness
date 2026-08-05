import { describe, expect, it } from "bun:test";

import { Either } from "../internal/either";
import type { z } from "../internal/zod";
import { parseSchema } from "../internal/zod";
import type { EvalManifestV1 } from "./eval-manifest";
import {
  EVAL_MANIFEST_API_VERSION,
  EvalManifestSchema,
  isDeterministicScoring,
  manifestIdentityDigest,
  validateManifest,
} from "./eval-manifest";

type ManifestInput = z.input<typeof EvalManifestSchema>;

/** A minimal valid spec-bound manifest (the shape rung-1 user evals take). */
function specManifest(overrides: Partial<ManifestInput> = {}): ManifestInput {
  const base: ManifestInput = {
    apiVersion: EVAL_MANIFEST_API_VERSION,
    id: "org_abc/my-eval",
    name: "My Eval",
    version: "1",
    protocolId: "org_abc.my-eval.v1",
    dataset: {
      kind: "inline",
      cases: [{ input: "What is 2+2?", target: "4" }],
    },
    models: [{ role: "candidate" }],
    solver: { kind: "spec", promptTemplate: "Q: {input}\nA:" },
    scorer: { kind: "spec", method: { kind: "exact" } },
    epochs: { default: 1, reducer: "mean" },
    execution: { tier: "trusted-local", drivers: ["process"] },
    reports: [{ key: "score", label: "Score", type: "scalar", goal: "higher" }],
  };
  return { ...base, ...overrides };
}

function parse(value: unknown): EvalManifestV1 {
  const parsed = parseSchema(EvalManifestSchema, value);
  if (Either.isLeft(parsed)) {
    throw new Error(parsed.left.message);
  }
  return parsed.right;
}

describe("EvalManifestSchema", () => {
  it("accepts a minimal spec-bound user eval", () => {
    const manifest = parse(specManifest());
    expect(validateManifest(manifest)).toEqual([]);
    expect(isDeterministicScoring(manifest)).toBe(true);
  });

  it("accepts a builtin-bound first-party benchmark shape (gpqa)", () => {
    const manifest = parse(
      specManifest({
        id: "gpqa_diamond",
        protocolId: "gpqa-diamond.public.v1",
        dataset: {
          kind: "hf",
          dataset: "nmayorga7/gpqa_diamond",
          config: "default",
          split: "train",
          inputField: "Question",
          targetField: "Correct Answer",
        },
        solver: { kind: "builtin", ref: "gpqa-solver" },
        scorer: { kind: "builtin", ref: "mcq" },
        sampling: { temperature: 0.5 },
        epochs: { default: 10, reducer: "mean" },
      })
    );
    expect(validateManifest(manifest)).toEqual([]);
    // builtin ⇒ not provably spec-deterministic
    expect(isDeterministicScoring(manifest)).toBe(false);
  });

  it("accepts a multi-role agentic shape (tau3: user simulator + builtin everything)", () => {
    const manifest = parse(
      specManifest({
        id: "tau3_bench_banking",
        dataset: { kind: "builtin", ref: "tau3-banking-dataset" },
        models: [
          { role: "candidate" },
          { role: "user-simulator", defaultModel: "openai/gpt-5.4-mini" },
        ],
        solver: {
          kind: "builtin",
          ref: "tau3-banking-solver",
          options: { retrieval: "auto" },
        },
        scorer: { kind: "builtin", ref: "tau3-banking-scorer" },
        epochs: { default: 5, reducer: "mean" },
      })
    );
    expect(validateManifest(manifest)).toEqual([]);
  });

  it("accepts a capsule adapter shape and enforces digest pinning", () => {
    const image = `ganler/evalplus@sha256:${"a".repeat(64)}`;
    const manifest = parse(
      specManifest({
        id: "humaneval-plus",
        solver: { kind: "spec" },
        scorer: {
          kind: "capsule",
          image,
          command: ["python", "-m", "evalplus"],
          network: "none",
        },
        execution: { tier: "rootless", drivers: ["docker"] },
      })
    );
    expect(validateManifest(manifest)).toEqual([]);

    const floating = parseSchema(
      EvalManifestSchema,
      specManifest({
        scorer: {
          kind: "capsule",
          image: "ganler/evalplus:latest",
          command: ["x"],
        },
      })
    );
    expect(Either.isLeft(floating)).toBe(true);
  });

  it("rejects unknown apiVersion and missing reports", () => {
    expect(
      Either.isLeft(
        parseSchema(EvalManifestSchema, { ...specManifest(), apiVersion: "v2" })
      )
    ).toBe(true);
    expect(
      Either.isLeft(
        parseSchema(EvalManifestSchema, specManifest({ reports: [] }))
      )
    ).toBe(true);
  });
});

describe("validateManifest", () => {
  it("requires a candidate role and unique roles", () => {
    const noCandidate = parse(specManifest({ models: [{ role: "judge" }] }));
    expect(validateManifest(noCandidate).map((i) => i.code)).toContain(
      "missing-candidate"
    );

    const duplicate = parse(
      specManifest({ models: [{ role: "candidate" }, { role: "candidate" }] })
    );
    expect(validateManifest(duplicate).map((i) => i.code)).toContain(
      "duplicate-role"
    );
  });

  it("spec-judge scoring requires a declared judge role", () => {
    const judged = parse(
      specManifest({
        scorer: { kind: "spec-judge", promptRef: "prompts/judge-v1" },
      })
    );
    expect(validateManifest(judged).map((i) => i.code)).toContain(
      "judge-role-missing"
    );

    const fixed = parse(
      specManifest({
        models: [
          { role: "candidate" },
          { role: "judge", defaultModel: "openai/gpt-5.4" },
        ],
        scorer: { kind: "spec-judge", promptRef: "prompts/judge-v1" },
      })
    );
    expect(validateManifest(fixed)).toEqual([]);
    expect(isDeterministicScoring(fixed)).toBe(false);
  });

  it("enforces pass@k coherence", () => {
    const noK = parse(
      specManifest({ epochs: { default: 5, reducer: "pass@k" } })
    );
    expect(validateManifest(noK).map((i) => i.code)).toContain("k-missing");

    const bigK = parse(
      specManifest({ epochs: { default: 3, reducer: "pass@k", k: 5 } })
    );
    expect(validateManifest(bigK).map((i) => i.code)).toContain(
      "k-exceeds-epochs"
    );
  });

  it("capsule bindings require rootless tier and a container driver", () => {
    const image = `x/y@sha256:${"b".repeat(64)}`;
    const wrongTier = parse(
      specManifest({
        scorer: { kind: "capsule", image, command: ["run"] },
        execution: { tier: "trusted-local", drivers: ["process"] },
      })
    );
    const codes = validateManifest(wrongTier).map((i) => i.code);
    expect(codes).toContain("capsule-needs-rootless");
    expect(codes).toContain("capsule-needs-container-driver");
  });
});

describe("manifestIdentityDigest", () => {
  it("is stable across metadata-only changes", () => {
    const a = parse(specManifest());
    const b = parse(specManifest({ name: "Renamed — display only" }));
    expect(manifestIdentityDigest(a)).toBe(manifestIdentityDigest(b));
    expect(manifestIdentityDigest(a)).toHaveLength(32);
  });

  it("changes when any identity-bearing field changes", () => {
    const base = manifestIdentityDigest(parse(specManifest()));
    const changed: Partial<ManifestInput>[] = [
      { version: "2" },
      { protocolId: "other.v1" },
      { sampling: { temperature: 0.7 } },
      { epochs: { default: 3, reducer: "mean" } },
      { solver: { kind: "spec", promptTemplate: "different: {input}" } },
      { scorer: { kind: "spec", method: { kind: "contains" } } },
      { mediaRecipe: { imageDetail: "high" } },
      { dataset: { kind: "inline", cases: [{ input: "other", target: "x" }] } },
    ];
    for (const override of changed) {
      expect(manifestIdentityDigest(parse(specManifest(override)))).not.toBe(
        base
      );
    }
  });

  it("is insensitive to model-role declaration order", () => {
    const roles = [
      { role: "candidate" as const },
      { role: "user-simulator" as const, defaultModel: "openai/gpt-5.4-mini" },
    ];
    const a = parse(specManifest({ models: roles }));
    const b = parse(specManifest({ models: [...roles].reverse() }));
    expect(manifestIdentityDigest(a)).toBe(manifestIdentityDigest(b));
  });

  it("ignores provenance and reports (metadata, not identity)", () => {
    const a = parse(specManifest());
    const b = parse(
      specManifest({
        provenance: { supportStatus: "ready", comparabilityNotes: "anything" },
        reports: [{ key: "other", label: "Other", type: "table" }],
      })
    );
    expect(manifestIdentityDigest(a)).toBe(manifestIdentityDigest(b));
  });
});
