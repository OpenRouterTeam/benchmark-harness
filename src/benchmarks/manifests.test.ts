import { describe, expect, it } from "bun:test";

import { assertRight } from "../internal/testing";
import { parseSchema } from "../internal/zod";
import {
  GPQA_META,
  TAU3_BENCH_BANKING_META,
  getBenchmarkMeta,
} from "./benchmark-meta";
import {
  EvalManifestSchema,
  manifestIdentityDigest,
  validateManifest,
} from "./eval-manifest";
import { GPQA_DATASET, GPQA_OPTION_FIELDS } from "./gpqa";
import {
  FIRST_PARTY_MANIFESTS,
  GPQA_MANIFEST,
  TAU3_BENCH_BANKING_MANIFEST,
} from "./manifests";
import { getBenchmark } from "./registry";

describe("first-party manifests", () => {
  it("every manifest parses, validates, and has a stable identity digest", () => {
    for (const manifest of Object.values(FIRST_PARTY_MANIFESTS)) {
      assertRight(parseSchema(EvalManifestSchema, manifest));
      expect(validateManifest(manifest)).toEqual([]);
      expect(manifestIdentityDigest(manifest)).toHaveLength(32);
    }
  });

  /*
   * Equality guards: until meta/constants are *derived from* manifests, the
   * two sources must agree. A drift in either direction fails here, which is
   * the M2 contract — manifests cannot silently diverge from the running
   * benchmark definitions.
   */
  it("gpqa manifest agrees with its meta and registry entry", () => {
    expect(GPQA_MANIFEST.id).toBe(GPQA_META.id);
    expect(GPQA_MANIFEST.epochs.default).toBe(GPQA_META.defaultEpochs);
    expect(GPQA_MANIFEST.sampling?.temperature).toBe(GPQA_META.temperature);
    expect(getBenchmark(GPQA_MANIFEST.id)?.defaultEpochs).toBe(
      GPQA_MANIFEST.epochs.default
    );
    expect(getBenchmark(GPQA_MANIFEST.id)?.temperature).toBe(
      GPQA_MANIFEST.sampling?.temperature
    );
  });

  it("gpqa manifest dataset reference and record fields match the running dataset module", () => {
    /* inputField/targetField on a builtin-bound dataset are documentation for
     * tooling — this guard keeps the documentation from silently lying about
     * the record shape the builtin solver actually reads. */
    const dataset = GPQA_MANIFEST.dataset;
    expect(dataset.kind).toBe("hf");
    if (dataset.kind !== "hf") {
      throw new Error("unreachable");
    }
    expect(dataset.dataset).toBe(GPQA_DATASET.dataset);
    expect(dataset.config).toBe(GPQA_DATASET.config);
    expect(dataset.split).toBe(GPQA_DATASET.split);
    expect(dataset.revision).toBe(GPQA_DATASET.revision);
    expect(dataset.revision).toMatch(/^[0-9a-f]{40}$/);
    /* The fields named by the manifest must be fields the solver reads. */
    expect(dataset.inputField).toBe("Question");
    expect(dataset.targetField).toBe(GPQA_OPTION_FIELDS[0]);
  });

  it("gpqa protocol id is the openrouter series, not the simple-evals reference protocol", () => {
    expect(GPQA_MANIFEST.protocolId).toBe("gpqa-diamond.openrouter.v1");
    expect(GPQA_MANIFEST.epochs.default).toBe(10);
  });

  it("tau3 manifest agrees with its meta, registry entry, and user-simulator default", () => {
    expect(TAU3_BENCH_BANKING_MANIFEST.id).toBe(TAU3_BENCH_BANKING_META.id);
    expect(TAU3_BENCH_BANKING_MANIFEST.epochs.default).toBe(
      TAU3_BENCH_BANKING_META.defaultEpochs
    );
    expect(getBenchmark(TAU3_BENCH_BANKING_MANIFEST.id)?.defaultEpochs).toBe(
      TAU3_BENCH_BANKING_MANIFEST.epochs.default
    );
    const simulator = TAU3_BENCH_BANKING_MANIFEST.models.find(
      (m) => m.role === "user-simulator"
    );
    expect(simulator?.defaultModel).toBe(TAU3_BENCH_BANKING_META.userModel);
  });

  it("manifest ids resolve in the meta registry (no orphan manifests)", () => {
    for (const id of Object.keys(FIRST_PARTY_MANIFESTS)) {
      expect(getBenchmarkMeta(id)).toBeDefined();
      expect(getBenchmark(id)).toBeDefined();
    }
  });

  it("identity digests are distinct across benchmarks", () => {
    const digests = Object.values(FIRST_PARTY_MANIFESTS).map((m) =>
      manifestIdentityDigest(m)
    );
    expect(new Set(digests).size).toBe(digests.length);
  });
});
